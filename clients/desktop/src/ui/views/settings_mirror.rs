//! Settings, for admins: the mirror of the server's files on this computer (see `mirror`),
//! and moving the folder everything's kept in (see `root`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::mirror::in_app::{Command, Opening};
use crate::mirror::index::{Count, RemovableGroup};
use crate::mirror::service::State as Service;
use crate::mirror::{Doing, Snapshot};
use crate::root::MovePlan;
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{thousands, widgets, Shell};

#[derive(Default)]
pub struct MirrorSection {
    /// A removal asked for, waiting for a yes: its group's (state, volume).
    confirm: Option<(Option<String>, Option<String>)>,
    /// A folder chosen to look in.
    folder_chosen: Arc<Mutex<Vec<PathBuf>>>,
    /// The server folder for the folder being added (suggested, editable).
    server_folder: String,
    /// The mirror is paused (shown in the status line).
    paused: bool,
    /// The server was asked for its list since the page opened (to show sizes while
    /// mirroring is off).
    refreshed: bool,
    service: Arc<Mutex<Option<Service>>>,
    service_busy: Arc<AtomicBool>,
    service_said: Arc<Mutex<Option<String>>>,
    // Moving the root.
    move_chosen: Arc<Mutex<Vec<PathBuf>>>,
    move_plan: Arc<Mutex<Option<Result<MovePlan, String>>>>,
    moving: Option<Moving>,
}

struct Moving {
    copied: Arc<AtomicU64>,
    of: u64,
    /// Set when it's over: None if it worked (the app restarts), else why not.
    done: Arc<Mutex<Option<Result<(), String>>>>,
}

fn files(count: Count) -> String {
    format!("{} files · {}", thousands(count.blobs as usize), crate::mirror::cli::bytes(count.bytes))
}

fn bytes(n: u64) -> String {
    crate::mirror::cli::bytes(n)
}

impl MirrorSection {
    pub fn opened(&mut self, shell: &Shell) {
        self.refreshed = false;
        self.ask_service(shell);
    }

    fn ask_service(&self, shell: &Shell) {
        let (slot, ctx) = (self.service.clone(), shell.ctx.clone());
        shell.runtime.spawn_blocking(move || {
            *slot.lock().unwrap() = Some(crate::mirror::service::state());
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let opening = shell.mirror.opening();
        let status = shell.mirror.status();
        let snapshot = shell.mirror.snapshot(shell.runtime.handle(), Duration::from_secs(2), &shell.ctx);
        let snapshot = match snapshot {
            Some(Ok(snapshot)) => Some(snapshot),
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't read the mirror: {err}")).color(P.danger));
                None
            }
            None => None,
        };
        // Moving along: shown as it goes.
        if status.as_ref().is_some_and(|s| !matches!(s.doing, Doing::Idle | Doing::Waiting)) {
            ui.ctx().request_repaint_after(Duration::from_millis(400));
        }

        self.paused = shell.mirror.paused.load(std::sync::atomic::Ordering::Relaxed);
        self.status_line(ui, opening.as_ref(), status.as_ref(), snapshot.as_ref());
        let (Some(Opening::Open), Some(snapshot)) = (opening, snapshot) else { return };
        // Mirroring off: the server's list asked for once, so there's something to choose from.
        if !snapshot.config.enabled && !self.refreshed {
            self.refreshed = true;
            shell.mirror.send(Command::Refresh);
        }
        ui.add_space(8.0);

        let mut config = snapshot.config.clone();
        let mut changed = false;
        changed |= ui.checkbox(&mut config.enabled, "Keep a copy of the server's files on this computer").changed();
        egui::Grid::new("mirror").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new("Volumes").color(P.muted));
            ui.vertical(|ui| {
                if snapshot.server_volumes.is_empty() {
                    ui.label(RichText::new("Asking the server…").color(P.faint));
                }
                for volume in &snapshot.server_volumes {
                    let total = snapshot.volumes.iter().find(|v| v.name == volume.name).cloned().unwrap_or_default();
                    let mut on = config.volumes.contains(&volume.name);
                    let name = if volume.default { format!("{} (new files go here)", volume.name) } else { volume.name.clone() };
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, name).changed() {
                            if on {
                                config.volumes.push(volume.name.clone());
                            } else {
                                config.volumes.retain(|v| v != &volume.name);
                            }
                            changed = true;
                        }
                        ui.label(RichText::new(files(total.kept)).color(P.faint));
                    });
                }
            });
            ui.end_row();

            ui.label(RichText::new("Previous files").color(P.muted));
            ui.horizontal(|ui| {
                let previous = snapshot.volumes.iter().filter(|v| config.volumes.contains(&v.name)).fold(Count::default(), |sum, v| Count {
                    blobs: sum.blobs + v.previous.blobs,
                    bytes: sum.bytes + v.previous.bytes,
                });
                changed |= ui.checkbox(&mut config.previous, "Also tracks' earlier files (replaced, kept on the server)").changed();
                ui.label(RichText::new(files(previous)).color(P.faint));
            });
            ui.end_row();

            ui.label(RichText::new("Look in").color(P.muted));
            ui.vertical(|ui| {
                let mut gone = None;
                for (i, look) in config.look_in.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(RichText::new(look.folder.display().to_string()).monospace().small()).selectable(true));
                        ui.label(RichText::new(format!("has the server's {}", look.server_folder)).color(P.faint).small());
                        if ui.small_button("✕").on_hover_text("Don't look here").clicked() {
                            gone = Some(i);
                        }
                    });
                }
                if let Some(i) = gone {
                    config.look_in.remove(i);
                    changed = true;
                }
                if let Some(folder) = self.folder_chosen.lock().unwrap().pop() {
                    self.server_folder.clear();
                    shell.mirror.detect(shell.runtime.handle(), folder, &shell.ctx);
                }
                if let Some(look) = self.adding(ui, shell) {
                    config.look_in.retain(|l| l.folder != look.folder);
                    config.look_in.push(look);
                    changed = true;
                }
                if shell.mirror.detected.lock().unwrap().is_none() && ui.small_button("Add a folder…").clicked() {
                    crate::ui::files::choose_folder(shell, "A folder with the files the server imported", &self.folder_chosen);
                }
                changed |= ui
                    .checkbox(&mut config.check_hashes, "Hash files from these folders before taking them in (slow; else their path and size are trusted)")
                    .changed();
            });
            ui.end_row();

            let plan = &snapshot.plan;
            ui.label(RichText::new("Here").color(P.muted));
            ui.label(format!("{} of {} wanted", files(plan.had), thousands(plan.wanted.blobs as usize)));
            ui.end_row();
            if plan.unchecked.blobs > 0 {
                ui.label(RichText::new("Not hashed yet").color(P.muted));
                ui.label(format!("{}: taken in by path and size. “Hash every file again” checks them.", files(plan.unchecked)));
                ui.end_row();
            }
            if plan.missing.blobs > 0 {
                ui.label(RichText::new("To fetch").color(P.muted));
                let unknown = if plan.missing_unsized > 0 { format!(" (and {} of unknown size)", thousands(plan.missing_unsized as usize)) } else { String::new() };
                ui.label(format!("{}{unknown}", files(plan.missing)));
                ui.end_row();
            }
            if let Some(free) = plan.free_bytes {
                ui.label(RichText::new("Free space").color(P.muted));
                if plan.missing.bytes > free {
                    ui.label(RichText::new(format!("{}: not enough for what's to fetch", bytes(free))).color(P.danger));
                } else {
                    ui.label(bytes(free));
                }
                ui.end_row();
            }
            ui.label(RichText::new("Where").color(P.muted));
            ui.add(egui::Label::new(RichText::new(shell.data_dir.join("blobs").display().to_string()).monospace().small()).selectable(true));
            ui.end_row();
        });
        if changed
            && let Err(err) = shell.mirror.set_config(&config)
        {
            shell.controller.report(err);
        }

        ui.add_space(6.0);
        let busy = status.as_ref().is_some_and(|s| !matches!(s.doing, Doing::Idle | Doing::Waiting));
        ui.horizontal(|ui| {
            if busy && widgets::chip(ui, "Stop", false) {
                shell.mirror.send(Command::Pause);
            }
            if config.enabled && !busy && widgets::chip(ui, "Sync now", false) {
                shell.mirror.send(Command::Restart);
            }
            if snapshot.plan.had.blobs > 0 && widgets::chip(ui, "Hash every file again", false) {
                *shell.mirror.outcome.lock().unwrap() = None;
                shell.mirror.send(Command::Rehash);
            }
        });
        if let Some(outcome) = shell.mirror.outcome.lock().unwrap().clone() {
            ui.label(RichText::new(outcome).color(P.muted));
        }

        if !snapshot.removable.is_empty() {
            ui.add_space(10.0);
            ui.label(RichText::new("Nothing wants these anymore").font(font(14.0, medium())));
            for group in &snapshot.removable {
                self.removable(ui, shell, group, &config);
            }
            hint(ui, "Nothing is removed from here because the server let it go: they stay until you remove them.");
        }

        if let Some(status) = &status
            && !status.problems.is_empty()
        {
            ui.add_space(10.0);
            ui.label(RichText::new("Problems").font(font(14.0, medium())));
            let shown = 5.min(status.problems.len());
            for problem in &status.problems[status.problems.len() - shown..] {
                ui.label(RichText::new(problem).color(P.danger).small());
            }
            if status.problems.len() > shown {
                ui.label(RichText::new(format!("and {} before", status.problems.len() - shown)).color(P.faint).small());
            }
        }

        ui.add_space(10.0);
        self.service(ui, shell);
        hint(
            ui,
            "Downloads are hashed as they come in, and only kept if they're what the server says. Files in the folders to look in are found by the path the server imported them from and their size, and taken in as reflinks where the disk can (no extra space), else copied. Your own files are never changed.",
        );
    }

    /// The folder being added: its server folder, worked out and shown to change or
    /// accept. The folder to add, once accepted.
    fn adding(&mut self, ui: &mut egui::Ui, shell: &mut Shell) -> Option<crate::mirror::index::LookIn> {
        let detected = shell.mirror.detected.lock().unwrap().clone();
        let (folder, found) = detected?;
        ui.add_space(4.0);
        ui.label(RichText::new(folder.display().to_string()).monospace().small());
        let mut add = None;
        match found {
            None => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Matching its files to the server's…").color(P.faint));
                    if ui.small_button("Cancel").clicked() {
                        shell.mirror.cancel_detect();
                    }
                });
                ui.ctx().request_repaint_after(Duration::from_millis(300));
                return None;
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't look: {err}")).color(P.danger));
            }
            Some(Ok(found)) => {
                if self.server_folder.is_empty()
                    && let Some(server_folder) = &found.server_folder
                {
                    self.server_folder = server_folder.clone();
                }
                let said = match &found.server_folder {
                    Some(server_folder) => format!(
                        "{} of its {} files are the server's, at the same paths under {server_folder}.",
                        thousands(found.matched as usize),
                        thousands(found.files as usize)
                    ),
                    None => format!("None of its {} files are the server's by path and size: say which folder of the server's it is.", thousands(found.files as usize)),
                };
                ui.label(RichText::new(said).color(P.muted));
            }
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("The server's folder").color(P.muted));
            ui.add(egui::TextEdit::singleline(&mut self.server_folder).desired_width(320.0).font(egui::TextStyle::Monospace));
            if !self.server_folder.trim().is_empty() && ui.button("Add").clicked() {
                add = Some(crate::mirror::index::LookIn { folder: folder.clone(), server_folder: self.server_folder.trim().to_string() });
            }
            if ui.button("Cancel").clicked() {
                shell.mirror.cancel_detect();
            }
        });
        if add.is_some() {
            shell.mirror.cancel_detect();
            self.server_folder.clear();
        }
        add
    }

    fn status_line(&self, ui: &mut egui::Ui, opening: Option<&Opening>, status: Option<&crate::mirror::Status>, snapshot: Option<&Snapshot>) {
        let (text, color) = match opening {
            None | Some(Opening::NotYet) => ("Opening…".to_string(), P.faint),
            Some(Opening::Waiting(why)) => (why.clone(), P.faint),
            Some(Opening::Failed(err)) => (format!("Couldn't open the mirror: {err}"), P.danger),
            Some(Opening::Open) => {
                let Some(status) = status else { return };
                let of = |what: &str| format!("{what}: {} of {}", thousands(status.done as usize), thousands(status.of as usize));
                match status.doing {
                    Doing::Idle | Doing::Waiting => match snapshot {
                        Some(s) if !s.config.enabled => ("Off.".to_string(), P.muted),
                        Some(_) if self.paused => ("Stopped. “Sync now” goes on.".to_string(), P.muted),
                        Some(s) if s.plan.missing.blobs == 0 => {
                            let heard = s.synced_at.map(|at| format!(" Heard from the server {}.", ago(at))).unwrap_or_default();
                            (format!("Up to date: {} here. New files come in as the server gets them.{heard}", files(s.plan.had)), P.text)
                        }
                        Some(s) => (format!("{} still to fetch.", files(s.plan.missing)), P.text),
                        None => (String::new(), P.faint),
                    },
                    Doing::Checking => (of("Checking the files here"), P.text),
                    Doing::Listing => ("Asking the server what it has…".to_string(), P.text),
                    Doing::LookingIn => (of("Looking in your folders"), P.text),
                    Doing::Downloading => (of("Downloading"), P.text),
                    Doing::Rehashing => (of("Hashing every file again"), P.text),
                    Doing::Removing => (of("Removing"), P.text),
                }
            }
        };
        ui.label(RichText::new(text).color(color));
        if let Some(status) = status
            && matches!(status.doing, Doing::Checking | Doing::LookingIn | Doing::Downloading | Doing::Rehashing)
        {
            let done = if status.of_bytes > 0 { status.bytes as f32 / status.of_bytes as f32 } else { status.done as f32 / status.of.max(1) as f32 };
            let text = if status.of_bytes > 0 { format!("{} of {}", bytes(status.bytes), bytes(status.of_bytes)) } else { String::new() };
            ui.add(egui::ProgressBar::new(done.clamp(0.0, 1.0)).desired_width(420.0).text(text));
            if let Some(current) = &status.current {
                ui.label(RichText::new(current).color(P.faint).small());
            }
        }
    }

    fn removable(&mut self, ui: &mut egui::Ui, shell: &mut Shell, group: &RemovableGroup, config: &crate::mirror::index::Config) {
        let on = |v: &Option<String>| v.as_deref().map(|v| format!(" on {v}")).unwrap_or_default();
        let why = match group.state.as_deref() {
            None => "no mirror of a server lists them".to_string(),
            Some("gone") => "gone from the server: these may be the only copies left".to_string(),
            Some("trashed") => "in the server's trash".to_string(),
            Some("other") => "nothing on the server uses them".to_string(),
            Some("previous") if config.enabled && group.volume.as_ref().is_some_and(|v| config.volumes.contains(v)) => {
                "tracks' earlier files (not mirrored)".to_string()
            }
            Some(_) if !config.enabled => "mirroring is off".to_string(),
            Some(_) => format!("not mirrored{}", on(&group.volume)),
        };
        let key = (group.state.clone(), group.volume.clone());
        ui.horizontal(|ui| {
            ui.label(format!("{}: {}", why, files(group.count)));
            if self.confirm.as_ref() != Some(&key) && ui.small_button("Remove…").clicked() {
                self.confirm = Some(key.clone());
            }
        });
        if self.confirm.as_ref() == Some(&key) {
            let warning = if group.state.as_deref() == Some("gone") { " The server doesn't have them anymore: they can't be fetched again." } else { "" };
            ui.label(RichText::new(format!("Remove these {} from this computer?{warning}", files(group.count))).color(P.danger));
            ui.horizontal(|ui| {
                if ui.button("Remove").clicked() {
                    *shell.mirror.outcome.lock().unwrap() = None;
                    shell.mirror.send(Command::Remove(key.0.clone(), key.1.clone()));
                    self.confirm = None;
                }
                if ui.button("Keep them").clicked() {
                    self.confirm = None;
                }
            });
        }
    }

    fn service(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let state = self.service.lock().unwrap().clone();
        let busy = self.service_busy.load(Relaxed);
        let (text, action) = match state {
            None => ("Checking for the mirror service…".to_string(), None),
            Some(Service::Unsupported) => ("A mirror service can't be set up on this system: the mirror runs while the app is open.".to_string(), None),
            Some(Service::NotInstalled) => (
                "The mirror runs while the app is open. The mirror service keeps it up to date while the app is closed too.".to_string(),
                Some(("Set up the mirror service", true)),
            ),
            Some(Service::Installed { running }) => (
                format!(
                    "The mirror service is set up ({}): it keeps the mirror up to date while the app is closed, and steps aside while it's open.",
                    if running { "running" } else { "not running now" }
                ),
                Some(("Remove the mirror service", false)),
            ),
        };
        ui.label(RichText::new(text).color(P.muted));
        if let Some((label, install)) = action
            && !busy
            && ui.small_button(label).clicked()
        {
            self.service_busy.store(true, Relaxed);
            let (busy, said, slot, ctx) = (self.service_busy.clone(), self.service_said.clone(), self.service.clone(), shell.ctx.clone());
            shell.runtime.spawn_blocking(move || {
                let result = if install { crate::mirror::service::install() } else { crate::mirror::service::remove() };
                *said.lock().unwrap() = result.err().map(|e| format!("{e:#}"));
                *slot.lock().unwrap() = Some(crate::mirror::service::state());
                busy.store(false, Relaxed);
                ctx.request_repaint();
            });
        }
        if let Some(said) = self.service_said.lock().unwrap().clone() {
            ui.label(RichText::new(said).color(P.danger).small());
        }
        if cfg!(target_os = "linux") && matches!(state, Some(Service::Installed { .. })) {
            hint(ui, "A user service runs while you're logged in; `loginctl enable-linger` keeps it running after you log out.");
        }
    }

    /// Moving the folder everything's kept in: choose where, see what would move, move,
    /// and the app starts again from there.
    pub fn show_move(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if std::env::var_os("ACETATE_ROOT").is_some_and(|r| !r.is_empty()) {
            hint(ui, "ACETATE_ROOT chooses this folder: change that to move it.");
            return;
        }
        if let Some(moving) = &self.moving {
            let done = moving.done.lock().unwrap().clone();
            match done {
                None => {
                    let copied = moving.copied.load(Relaxed);
                    ui.add(egui::ProgressBar::new(copied as f32 / moving.of.max(1) as f32).desired_width(420.0).text(format!(
                        "Moving… {} of {}",
                        bytes(copied),
                        bytes(moving.of)
                    )));
                    ui.ctx().request_repaint_after(Duration::from_millis(300));
                    return;
                }
                Some(Ok(())) => {
                    // Started again from the new folder: nothing more is written to the old one.
                    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn()) {
                        Ok(_) => std::process::exit(0),
                        Err(err) => {
                            ui.label(RichText::new(format!("Moved. Couldn't start the app again ({err}): close it and open it again.")).color(P.danger));
                            return;
                        }
                    }
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(format!("The move stopped: {err}")).color(P.danger));
                    hint(ui, "What was moved is in the new folder, the rest still here; the app keeps using this folder. Moving again goes on from there.");
                    if ui.small_button("OK").clicked() {
                        self.moving = None;
                    }
                    return;
                }
            }
        }

        // A folder chosen: the plan worked out in the background (measuring can take a moment).
        if let Some(chosen) = self.move_chosen.lock().unwrap().pop() {
            // Into the folder chosen if it's empty, else into a folder of its own in it.
            let empty = std::fs::read_dir(&chosen).is_ok_and(|mut d| d.next().is_none());
            let to = if empty { chosen } else { chosen.join("acetate") };
            let (from, slot, ctx) = (shell.data_dir.clone(), self.move_plan.clone(), shell.ctx.clone());
            *slot.lock().unwrap() = None;
            shell.runtime.spawn_blocking(move || {
                *slot.lock().unwrap() = Some(crate::root::plan_move(&from, &to).map_err(|e| format!("{e:#}")));
                ctx.request_repaint();
            });
            *self.move_plan.lock().unwrap() = Some(Err(String::new()));
        }
        let plan = self.move_plan.lock().unwrap().clone();
        match plan {
            None => {
                if ui.small_button("Move this folder…").clicked() {
                    crate::ui::files::choose_folder(shell, "Where to keep Acetate's folder", &self.move_chosen);
                }
            }
            Some(Err(err)) if err.is_empty() => {
                ui.label(RichText::new("Measuring what's to move…").color(P.faint));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Can't move it there: {err}")).color(P.danger));
                if ui.small_button("OK").clicked() {
                    *self.move_plan.lock().unwrap() = None;
                }
            }
            Some(Ok(plan)) => {
                ui.label(format!(
                    "Move {} ({} files, {}) to {}?",
                    plan.entries.iter().map(|e| e.name.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", "),
                    thousands(plan.files() as usize),
                    bytes(plan.bytes()),
                    plan.to.display()
                ));
                if plan.same_filesystem {
                    hint(ui, "The same disk: it's moved at once.");
                } else {
                    let free = plan.free_bytes.map(|f| format!(" ({} free there)", bytes(f))).unwrap_or_default();
                    hint(ui, &format!("Another disk{free}: everything is copied and each file's hash checked, then removed here. The app starts again when it's done."));
                }
                let short = !plan.same_filesystem && plan.free_bytes.is_some_and(|f| f < plan.bytes());
                if short {
                    ui.label(RichText::new("There isn't enough free space there.").color(P.danger));
                }
                ui.horizontal(|ui| {
                    if !short && ui.button("Move and start again").clicked() {
                        self.start_move(shell, plan.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        *self.move_plan.lock().unwrap() = None;
                    }
                });
            }
        }
    }

    fn start_move(&mut self, shell: &mut Shell, plan: MovePlan) {
        // The mirror lets go of its files first.
        shell.mirror.shut_down(shell.runtime.handle());
        let moving = Moving { copied: Arc::default(), of: plan.bytes(), done: Arc::default() };
        let (copied, done, ctx) = (moving.copied.clone(), moving.done.clone(), shell.ctx.clone());
        std::thread::spawn(move || {
            let result = crate::root::move_root(&plan, &|n| copied.store(n, Relaxed)).map_err(|e| format!("{e:#}"));
            *done.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        self.moving = Some(moving);
        *self.move_plan.lock().unwrap() = None;
    }
}

/// "just now", "5 minutes ago", "3 hours ago" (`at`: seconds since 1970).
fn ago(at: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(at);
    match widgets::how_long((now - at).max(0) as u64) {
        Some(span) => format!("{span} ago"),
        None => "just now".to_string(),
    }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(font(12.5, medium())).color(P.faint));
}
