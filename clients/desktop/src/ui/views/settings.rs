//! Settings: the sound output and volume, the account, what the server stores (admins),
//! and the library kept on this computer (how much, where, and checking it against the
//! server).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Shell};

#[derive(Default)]
pub struct SettingsPage {
    /// The sound outputs there are, listed in the background when the page opens (it can
    /// take a moment).
    outputs: Arc<Mutex<Option<Vec<String>>>>,
    /// The qualities the server makes copies at (`GET /api/qualities`), the default first.
    qualities: Arc<Mutex<Option<Vec<u32>>>>,
    /// The size of the kept covers (small, standard, and an earlier version's no longer
    /// used), measured in the background: (files, bytes) each.
    covers_size: Arc<Mutex<Option<[(usize, u64); 3]>>>,
    /// Which app opens acetate:// links (the system's answer), asked in the background.
    links_handler: Arc<Mutex<Option<crate::links::Handler>>>,
    /// What the server stores on each volume (`GET /api/storage`, admins), asked when the
    /// page opens.
    storage: Arc<Mutex<Option<Result<Vec<StorageVolume>, String>>>>,
}

#[derive(Clone, serde::Deserialize)]
struct StorageVolume {
    name: String,
    default: bool,
    parts: Vec<StoragePart>,
}

#[derive(Clone, serde::Deserialize)]
struct StoragePart {
    kind: String,
    blobs: i64,
    bytes: i64,
    unmeasured: i64,
}

/// `12.3 MB`, `1.2 GB`, `3.4 TB`.
fn size(bytes: i64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e12 {
        format!("{:.1} TB", bytes / 1e12)
    } else if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.1} MB", bytes / 1e6)
    }
}

impl SettingsPage {
    pub fn opened(&mut self, shell: &Shell) {
        if shell.account.can("admin") {
            let (api, storage, ctx) = (shell.api.clone(), self.storage.clone(), shell.ctx.clone());
            shell.runtime.spawn(async move {
                let answer = api.send_json::<Vec<StorageVolume>>(reqwest::Method::GET, "/api/storage", None).await.map_err(|e| e.message);
                *storage.lock().unwrap() = Some(answer);
                ctx.request_repaint();
            });
        }
        let outputs = self.outputs.clone();
        shell.runtime.spawn_blocking(move || *outputs.lock().unwrap() = Some(crate::playback::engine::outputs()));
        let (qualities, api, ctx) = (self.qualities.clone(), shell.api.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            #[derive(serde::Deserialize)]
            struct Qualities {
                kbps: Vec<u32>,
            }
            if let Ok(q) = api.send_json::<Qualities>(reqwest::Method::GET, "/api/qualities", None).await {
                *qualities.lock().unwrap() = Some(q.kbps);
                ctx.request_repaint();
            }
        });
        let handler = self.links_handler.clone();
        shell.runtime.spawn_blocking(move || *handler.lock().unwrap() = Some(crate::links::handler()));
        let (size, dir) = (self.covers_size.clone(), shell.cache_dir.clone());
        shell.runtime.spawn_blocking(move || {
            let measure = |name: &str| {
                let files: Vec<u64> = std::fs::read_dir(dir.join(name))
                    .map(|entries| entries.filter_map(|e| e.ok()?.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).collect())
                    .unwrap_or_default();
                (files.len(), files.iter().sum())
            };
            *size.lock().unwrap() = Some([measure("covers-small"), measure("covers-standard"), measure("covers")]);
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
        ui.label(RichText::new("Settings").font(font(30.0, bold())));
        ui.add_space(12.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Sound");
            egui::Grid::new("sound").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Output").color(P.muted));
                let outputs = self.outputs.lock().unwrap().clone();
                let current = shell.settings.output.clone();
                let shown = current.clone().unwrap_or_else(|| "The system's default".into());
                egui::ComboBox::from_id_salt("output").selected_text(shown).width(320.0).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "The system's default").clicked() {
                        shell.act(Action::SetOutput(None));
                    }
                    match &outputs {
                        None => {
                            ui.label(RichText::new("Looking for outputs…").color(P.faint));
                        }
                        Some(names) => {
                            for name in names {
                                if ui.selectable_label(current.as_deref() == Some(name.as_str()), name).clicked() {
                                    shell.act(Action::SetOutput(Some(name.clone())));
                                }
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Volume").color(P.muted));
                ui.horizontal(|ui| {
                    let mut volume = player.volume;
                    let slider = egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false);
                    if ui.add_sized([240.0, 20.0], slider).changed() {
                        shell.act(Action::SetVolume(volume));
                    }
                    ui.label(RichText::new(format!("{}%", (player.volume * 100.0).round())).color(P.muted));
                    let mut muted = player.muted;
                    if ui.checkbox(&mut muted, "Muted").changed() {
                        shell.act(Action::ToggleMute);
                    }
                });
                ui.end_row();
            });
            egui::Grid::new("quality").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Streaming quality").color(P.muted));
                let current = shell.settings.stream_kbps;
                let qualities = self.qualities.lock().unwrap().clone();
                let shown = match current {
                    None => "Automatic".to_string(),
                    Some(k) => format!("{k} kbps copy"),
                };
                egui::ComboBox::from_id_salt("stream-quality").selected_text(shown).width(320.0).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "Automatic: the original if you may have it, else the server's usual copy").clicked() {
                        shell.act(Action::SetStreamQuality(None));
                    }
                    for k in qualities.iter().flatten() {
                        if ui.selectable_label(current == Some(*k), format!("{k} kbps copy")).clicked() {
                            shell.act(Action::SetStreamQuality(Some(*k)));
                        }
                    }
                });
                ui.end_row();
            });
            hint(ui, "Copies are Opus, made by the server (and kept there). A lower quality downloads less; it applies from the next track.");
            hint(ui, "9 and 0 turn it down and up, m mutes, anywhere outside a text field.");

            section(ui, "Account");
            let session = shell.api.session();
            egui::Grid::new("account").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                if let Some(session) = &session {
                    ui.label(RichText::new("Server").color(P.muted));
                    ui.label(&session.server);
                    ui.end_row();
                    ui.label(RichText::new("Logged in as").color(P.muted));
                    ui.label(&session.username);
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            if widgets::chip(ui, "Log out", false) {
                shell.act(Action::LogOut);
            }

            if shell.account.can("admin") {
                section(ui, "Stored on the server");
                match self.storage.lock().unwrap().clone() {
                    None => {
                        ui.label(RichText::new("Adding up…").color(P.faint));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
                    }
                    Some(Ok(volumes)) => {
                        egui::Grid::new("storage").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                            for volume in &volumes {
                                let total: i64 = volume.parts.iter().map(|p| p.bytes).sum();
                                let name = if volume.default { format!("{} (new files go here)", volume.name) } else { volume.name.clone() };
                                ui.label(RichText::new(name).color(P.muted));
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(size(total)).font(font(14.0, medium())));
                                    for part in &volume.parts {
                                        let unmeasured = if part.unmeasured > 0 { format!(" ({} not measured yet)", thousands(part.unmeasured as usize)) } else { String::new() };
                                        ui.label(RichText::new(format!("{}: {} · {}{unmeasured}", part.kind, size(part.bytes), thousands(part.blobs as usize))).color(P.faint));
                                    }
                                });
                                ui.end_row();
                            }
                        });
                        hint(ui, "Audio counts tracks' current and previous files; “other” is what nothing refers to anymore. Sizes the server never recorded (older covers) are filled in by acetate measure-covers.");
                    }
                }
            }

            section(ui, "Memory");
            self.memory(ui, shell);

            section(ui, "Library on this computer");
            let catalog = shell.catalog.value.clone();
            egui::Grid::new("library").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                if let Some(c) = &catalog {
                    ui.label(RichText::new("Kept here").color(P.muted));
                    ui.label(format!(
                        "{} tracks · {} albums · {} artists · {} playlists",
                        thousands(c.tracks.len()),
                        thousands(c.albums.len()),
                        thousands(c.artists.len()),
                        thousands(c.playlists.len())
                    ));
                    ui.end_row();
                }
                ui.label(RichText::new("State").color(P.muted));
                ui.label(shell.library_status());
                ui.end_row();
                ui.label(RichText::new("Covers kept").color(P.muted));
                let measured = *self.covers_size.lock().unwrap();
                let mb = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1_000_000.0);
                match measured {
                    Some([small, standard, _]) => ui.label(format!(
                        "{} small ({}) · {} standard ({})",
                        thousands(small.0),
                        mb(small.1),
                        thousands(standard.0),
                        mb(standard.1)
                    )),
                    None => ui.label(RichText::new("Measuring…").color(P.faint)),
                };
                ui.end_row();
                // Copies an earlier version made (one size, scaled here), no longer used.
                if let Some([_, _, (count, bytes)]) = measured
                    && count > 0
                {
                    ui.label(RichText::new("No longer used").color(P.muted));
                    ui.label(format!(
                        "{} covers from an earlier version ({}), in {}: safe to delete",
                        thousands(count),
                        mb(bytes),
                        shell.cache_dir.join("covers").display()
                    ));
                    ui.end_row();
                }
                ui.label(RichText::new("Where").color(P.muted));
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(shell.data_dir.display().to_string()).monospace().small()).selectable(true));
                    ui.add(egui::Label::new(RichText::new(shell.cache_dir.display().to_string()).monospace().small()).selectable(true));
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            if widgets::chip(ui, "Check for changes now  F5", false) {
                shell.act(Action::RefreshLibrary);
            }
            hint(ui, "The library shown is the copy kept here: it's there at once, and the server is asked for changes in the background (the server's copy wins).");

            section(ui, "Links");
            use crate::links::Handler;
            let how = if crate::links::CAN_REGISTER { "“Open links here” sets that up" } else { "clients/desktop/install.sh sets that up" };
            let handler = self.links_handler.lock().unwrap().clone();
            match &handler {
                Some(Handler::Here) => {
                    ui.label(RichText::new("Links to tracks, albums and artists open here.").color(P.text));
                }
                Some(Handler::Other(other)) => {
                    ui.label(RichText::new(format!("Links to tracks, albums and artists open in {other}, not here: {how}.")).color(P.danger));
                }
                Some(Handler::Nobody) => {
                    ui.label(RichText::new(format!("Links to tracks, albums and artists don't open here yet: {how}.")).color(P.muted));
                }
                None => {
                    ui.label(RichText::new("Checking…").color(P.faint));
                }
            }
            if crate::links::CAN_REGISTER && matches!(handler, Some(Handler::Other(_) | Handler::Nobody)) && ui.small_button("Open links here").clicked() {
                match crate::links::register() {
                    Ok(()) => *self.links_handler.lock().unwrap() = Some(crate::links::handler()),
                    Err(err) => shell.controller.report(err),
                }
            }
            hint(ui, "Ctrl+L (or “Share” on a page) copies a link to the track, album or artist shown, for someone with an account here; Ctrl+Shift+L (or Shift+click “Share”) opens a popup to make a share link anyone can open: its expiry, and whether it plays the original file or a lower-quality copy. Opened by someone logged in, either one goes to the app (or the web UI, if the app doesn't take it).");

            section(ui, "Keys");
            if widgets::chip(ui, "Show every key  ?", false) {
                shell.act(Action::ShowKeys);
            }
            ui.add_space(24.0);
            ui.label(RichText::new(format!("Acetate {}", env!("CARGO_PKG_VERSION"))).color(P.faint).small());
        });
    }
}

impl SettingsPage {
    /// What the app holds: in all (measured), and its big parts.
    fn memory(&mut self, ui: &mut egui::Ui, shell: &Shell) {
        let mb = |bytes: u64| format!("{} MB", thousands((bytes / 1_000_000) as usize));
        egui::Grid::new("memory").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            if let Some((now, peak)) = crate::memory::resident() {
                ui.label(RichText::new("In use").color(P.muted));
                ui.label(format!("{} (at most {} since it started)", mb(now), mb(peak)));
                ui.end_row();
            }
            if let Some(c) = &shell.catalog.value {
                // Measured once on a real library: about 2.2 kB a track with everything.
                ui.label(RichText::new("The library").color(P.muted));
                ui.label(format!("about {} ({} tracks; twice that for a moment while it's refreshed)", mb(c.tracks.len() as u64 * 2_230), thousands(c.tracks.len())));
                ui.end_row();
            }
            let (covers, cover_bytes) = shell.covers.textures();
            ui.label(RichText::new("Covers shown").color(P.muted));
            ui.label(format!("{} ({} images, let go when not shown for a while)", mb(cover_bytes as u64), thousands(covers)));
            ui.end_row();
            ui.label(RichText::new("Playing").color(P.muted));
            let held = crate::playback::stream::HELD.load(std::sync::atomic::Ordering::Relaxed);
            ui.label(format!("{} (what plays and what's next, each file whole)", mb(held)));
            ui.end_row();
        });
        if ui.small_button("Give freed memory back now").clicked() {
            crate::memory::trim();
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        hint(ui, "Freed memory is given back to the system after the library is refreshed, when covers are let go, and once a minute.");
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).font(font(18.0, bold())));
    ui.add_space(8.0);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(font(12.5, medium())).color(P.faint));
}
