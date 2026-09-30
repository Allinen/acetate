//! Extras (vocals, stems, samples, clips, project files: not tracks) on a track's, album's
//! or artist's page: each with its files (audio plays, anything can be saved), who sees
//! it, and a link to its own page. In edit mode: "Attach extra…" (a name, a kind, files
//! chosen, pasted or dropped; seen like the track it's attached to), linking one that's
//! there already, adding files, unlinking, taking one off (kept: "Taken off" brings it
//! back). See the server's `db::extras`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::details::{Extra, ExtraFile};
use crate::playback::{AudioSource, QueueItem};
use crate::ui::edit::inline_search::{Choice, InlineSearch};
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

/// The kinds offered (any other can be typed).
pub const KINDS: &[&str] = &["vocals", "instrumental", "stems", "sample", "clip", "video", "project", "other"];

/// What extras are shown on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Owner {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Owner {
    /// As the server names a link.
    pub fn link(self) -> serde_json::Value {
        match self {
            Owner::Track(id) => serde_json::json!({ "kind": "track", "id": id }),
            Owner::Album(id) => serde_json::json!({ "kind": "album", "id": id }),
            Owner::Artist(id) => serde_json::json!({ "kind": "artist", "id": id }),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Owner::Track(_) => "track",
            Owner::Album(_) => "album",
            Owner::Artist(_) => "artist",
        }
    }

    /// Its details fetched again (its extras are in them).
    pub fn refresh(self, shell: &mut Shell) {
        match self {
            Owner::Track(id) => shell.track_details.refresh(id),
            Owner::Album(id) => shell.album_details.refresh(id),
            Owner::Artist(id) => shell.artist_details.refresh(id),
        }
    }
}

/// Plays one of an extra's files (not counted as a play: it isn't a track).
pub fn play_file(shell: &Shell, extra: &str, file: Uuid, name: &str, duration_ms: Option<i64>) {
    let item = QueueItem {
        track_id: file,
        file_id: file,
        source: AudioSource::Extra,
        title: extra.to_string(),
        artist: name.to_string(),
        album: None,
        duration_ms,
        cover: None,
    };
    let controller = shell.controller.clone();
    shell.runtime.spawn(async move { controller.play(vec![item], 0).await });
}

/// Saves one of an extra's files where the user chooses (the system's save dialog).
/// `said`: what happened, when it's done.
pub fn save_file(shell: &Shell, file: Uuid, name: &str, said: &Arc<Mutex<Option<Result<String, String>>>>) {
    let (api, ctx, said, name) = (shell.api.clone(), shell.ctx.clone(), said.clone(), name.to_string());
    shell.runtime.spawn(async move {
        let Some(to) = rfd::AsyncFileDialog::new().set_file_name(&name).save_file().await else { return };
        let path = to.path().to_path_buf();
        let result = async {
            let response = api.stream_from(&format!("/api/extra-files/{file}/stream"), 0).await.map_err(|e| e.message)?;
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            tokio::fs::write(&path, &bytes).await.map_err(|e| e.to_string())?;
            Ok(format!("Saved to {}", path.display()))
        }
        .await;
        *said.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
}

/// A file's line: ▶ (audio) or its thumbnail and "Watch" (video), its name, what it is,
/// and "Save…".
pub fn file_line(ui: &mut egui::Ui, shell: &mut Shell, extra: &Extra, f: &ExtraFile, said: &Arc<Mutex<Option<Result<String, String>>>>) {
    ui.horizontal(|ui| {
        if f.is_video() {
            let (rect, response) = ui.allocate_exact_size(egui::Vec2::new(96.0, 54.0), egui::Sense::click());
            shell.cover(ui, f.thumb(), f.id, rect, 4, 0.0);
            let watch = ui.button("▶ Watch").clicked();
            if watch || response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                shell.watch(f.id, &extra.name);
            }
        } else if (f.duration_ms.is_some() || f.codec.is_some()) && ui.small_button("▶").on_hover_text("Play it").clicked() {
            play_file(shell, &extra.name, f.id, &f.name, f.duration_ms);
        }
        ui.label(RichText::new(&f.name).font(font(13.5, medium())));
        let mut facts = Vec::new();
        if let (Some(w), Some(h)) = (f.width, f.height) {
            let fps = f.frame_rate.map(|r| format!(" {r:.0} fps")).unwrap_or_default();
            facts.push(format!("{w}×{h}{fps}"));
        }
        if let Some(codec) = &f.video_codec {
            facts.push(codec.to_uppercase());
        }
        if let Some(codec) = &f.codec {
            facts.push(codec.to_uppercase());
        }
        if let (Some(rate), Some(depth)) = (f.sample_rate, f.bit_depth) {
            facts.push(format!("{} kHz {depth}-bit", rate as f32 / 1000.0));
        }
        if let Some(ms) = f.duration_ms {
            facts.push(widgets::length(Some(ms)));
        }
        facts.push(format!("{:.1} MB", f.size_bytes as f64 / 1_000_000.0));
        ui.label(RichText::new(facts.join(" · ")).color(P.faint).small());
        if ui.small_button("Save…").on_hover_text("Save a copy of the file").clicked() {
            save_file(shell, f.id, &f.name, said);
        }
    });
}

/// "Seen by everyone" or who sees it.
pub fn access_text(extra: &Extra) -> String {
    if !extra.is_restricted {
        return "seen by everyone".into();
    }
    let groups: Vec<&str> = extra.groups.iter().map(|g| g.name.as_str()).collect();
    if groups.is_empty() { "restricted: admins only".into() } else { format!("restricted: {}", groups.join(", ")) }
}

/// Who sees a new extra.
#[derive(Debug, Clone, PartialEq)]
enum Seen {
    /// Like the track it's attached to (the server's default).
    LikeTrack,
    Everyone,
    /// Admins and these groups.
    Groups(Vec<Uuid>),
}

/// A new extra being made here.
struct Attaching {
    name: String,
    kind: String,
    description: String,
    source: String,
    files: Vec<PathBuf>,
    seen: Seen,
}

/// A group, to choose who sees an extra.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GroupChoice {
    pub id: Uuid,
    pub name: String,
}

/// The groups (`GET /api/groups`, `manage_access`), asked once.
pub type Groups = Arc<Mutex<Option<Result<Vec<GroupChoice>, String>>>>;

pub fn ask_groups(shell: &Shell, into: &Groups) {
    if into.lock().unwrap().is_some() {
        return;
    }
    *into.lock().unwrap() = Some(Ok(Vec::new()));
    let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), into.clone());
    shell.runtime.spawn(async move {
        let result = api.send_json::<Vec<GroupChoice>>(reqwest::Method::GET, "/api/groups", None).await.map_err(|e| e.message);
        *into.lock().unwrap() = Some(result);
        ctx.request_repaint();
    });
}

/// An extra's description and source, as a page shows them (links clickable).
pub fn about(ui: &mut egui::Ui, description: &str, source: &str, lines: Option<usize>) {
    if !source.trim().is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("From").color(P.faint).small());
            crate::ui::edit::notes::text_line(ui, source.trim());
        });
    }
    let all: Vec<&str> = description.lines().collect();
    let shown = lines.map(|n| n.min(all.len())).unwrap_or(all.len());
    for line in &all[..shown] {
        crate::ui::edit::notes::text_line(ui, line);
    }
    if shown < all.len() {
        ui.label(RichText::new("…").color(P.faint));
    }
}

#[derive(Default)]
pub struct ExtrasView {
    owner: Option<Owner>,
    attaching: Option<Attaching>,
    /// Linking one that's there (found by typing).
    linking: Option<InlineSearch>,
    /// Files chosen in the dialog: for the new extra, or (Some) for this one.
    chosen: Arc<Mutex<Vec<PathBuf>>>,
    chosen_for: Option<Uuid>,
    sending: bool,
    progress: Arc<AtomicUsize>,
    uploading: usize,
    answer: Arc<Mutex<Option<Result<String, String>>>>,
    groups: Groups,
    /// A file saved (or not): said, nothing else happens.
    saved: Arc<Mutex<Option<Result<String, String>>>>,
    result: Option<Result<String, String>>,
}

impl ExtrasView {
    /// Whether there's anything to show (outside edit mode: extras that aren't taken off).
    pub fn worth_showing(shell: &Shell, extras: &[Extra]) -> bool {
        (shell.edit_mode && shell.account.can("edit_metadata")) || extras.iter().any(|e| !e.hidden)
    }

    /// The extras linked to `owner`. `restricted`: the owner is (a new extra starts so).
    /// `take_files`: dropped or pasted files go to the extra being attached here.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, extras: &[Extra], restricted: bool, take_files: bool) {
        if self.owner != Some(owner) {
            *self = ExtrasView { owner: Some(owner), ..Default::default() };
        }
        let answer = self.answer.lock().unwrap().take();
        if let Some(result) = answer {
            self.sending = false;
            if result.is_ok() {
                self.attaching = None;
            }
            owner.refresh(shell);
            shell.act(Action::RefreshLibrary);
            self.result = Some(result);
        }
        let saved = self.saved.lock().unwrap().take();
        if let Some(result) = saved {
            self.result = Some(result);
        }
        let can = shell.edit_mode && shell.account.can("edit_metadata");
        // Files for the extra being attached, or for one getting more.
        let chosen = std::mem::take(&mut *self.chosen.lock().unwrap());
        if !chosen.is_empty() {
            match (self.chosen_for.take(), self.attaching.as_mut()) {
                (Some(extra), _) => self.upload(shell, extra, chosen, "Files added."),
                (None, Some(a)) => a.files.extend(chosen),
                (None, None) => {}
            }
        }
        if can && take_files && self.attaching.is_some() && !shell.palette_open && !shell.modal_open {
            let given = crate::ui::files::given(ui.ctx());
            // Images are covers' and text files notes' (on the same page); the rest is audio,
            // archives, project files.
            let mine = given.into_iter().filter(|p| {
                let ext = crate::ui::files::extension(p);
                !crate::ui::files::IMAGES.contains(&ext.as_str()) && !["txt", "md", "text", "nfo"].contains(&ext.as_str())
            });
            if let Some(a) = self.attaching.as_mut() {
                a.files.extend(mine);
            }
        }

        let said = self.saved.clone();
        let mut take_off: Option<(Uuid, bool)> = None;
        let mut unlink: Option<Uuid> = None;
        let mut add_files: Option<Uuid> = None;
        for extra in extras.iter().filter(|e| !e.hidden) {
            egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if widgets::link(ui, &extra.name, font(15.0, semibold()), P.text) {
                        shell.act(Action::Go(Page::Extra(extra.id)));
                    }
                    ui.label(RichText::new(&extra.kind).color(P.accent).small());
                    if extra.is_restricted {
                        ui.label(RichText::new(format!("🔒 {}", access_text(extra))).color(P.muted).small());
                    }
                });
                about(ui, &extra.description, &extra.source, Some(3));
                let shown: Vec<&ExtraFile> = extra.files.iter().filter(|f| !f.hidden).collect();
                if shown.is_empty() {
                    ui.label(RichText::new("No files yet.").color(P.faint));
                }
                for f in shown {
                    file_line(ui, shell, extra, f, &said);
                }
                if can {
                    ui.horizontal(|ui| {
                        if ui.small_button("Add files…").clicked() {
                            add_files = Some(extra.id);
                        }
                        let tip = "Only here: the extra stays, on its own page and wherever else it's linked (a track's single goes with it)";
                        if ui.small_button(format!("Not on this {}", owner.name())).on_hover_text(tip).clicked() {
                            unlink = Some(extra.id);
                        }
                        if ui.small_button("Take off everywhere").on_hover_text("Hidden from every page it's on (kept, not deleted: “Taken off” brings it back)").clicked() {
                            take_off = Some((extra.id, true));
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
        if let Some(extra) = add_files {
            self.chosen_for = Some(extra);
            crate::ui::files::choose(shell, "Files to add", ("Any file", &[]), true, &self.chosen);
        }
        if !can {
            self.result_line(ui);
            return;
        }

        // ---- Edit mode: attach a new one, link one that's there, what was taken off ----
        self.attach_form(ui, shell, owner, restricted);
        if let Some(search) = self.linking.as_mut() {
            let catalog = shell.catalog.value.clone();
            let hit = search.show(ui, shell, catalog.as_deref(), egui::Id::new("extras-link"), "an extra that's there: type its name", &[]);
            if let (Some((Choice::Hit(i), _)), Some(c)) = (hit, catalog.as_deref())
                && let Some(e) = c.extras.get(i as usize)
            {
                let body = serde_json::json!({ "link": owner.link() });
                self.linking = None;
                self.send(shell, format!("/api/extras/{}/links", e.id), body, format!("Linked “{}”.", e.name));
            }
        }
        ui.horizontal_wrapped(|ui| {
            if self.attaching.is_none() && ui.button("Attach extra…").on_hover_text("Vocals, stems, a sample…: files that aren't a track").clicked() {
                self.attaching = Some(Attaching {
                    name: String::new(),
                    kind: "vocals".into(),
                    description: String::new(),
                    source: String::new(),
                    files: Vec::new(),
                    seen: Seen::LikeTrack,
                });
            }
            if self.linking.is_none() && ui.button("Link an existing extra…").clicked() {
                self.linking = Some(InlineSearch::new(Part::Extras));
            }
        });
        let hidden: Vec<&Extra> = extras.iter().filter(|e| e.hidden).collect();
        if !hidden.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("Taken off ({})", hidden.len())).color(P.muted)).id_salt(("extras-hidden", owner.link().to_string())).show(ui, |ui| {
                for e in hidden {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&e.name).color(P.muted));
                        if ui.small_button("Bring back").clicked() {
                            take_off = Some((e.id, false));
                        }
                    });
                }
            });
        }
        if let Some((extra, hidden)) = take_off {
            let text = if hidden { "Taken off (kept: “Taken off” brings it back)." } else { "Brought back." };
            self.send(shell, format!("/api/extras/{extra}/hidden"), serde_json::json!({ "hidden": hidden }), text.into());
        }
        if let Some(extra) = unlink {
            self.send(shell, format!("/api/extras/{extra}/links"), serde_json::json!({ "link": owner.link(), "undo": true }), "Unlinked.".into());
        }
        self.result_line(ui);
    }

    fn result_line(&self, ui: &mut egui::Ui) {
        if self.sending && self.uploading > 0 {
            ui.label(RichText::new(format!("Uploading {} of {}…", self.progress.load(Ordering::Relaxed) + 1, self.uploading)).color(P.muted));
        }
        match &self.result {
            Some(Ok(text)) => {
                ui.label(RichText::new(text.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }
    }

    /// "Attach extra…": its name, kind, files, and who'll see it.
    fn attach_form(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, restricted: bool) {
        let Some(a) = self.attaching.as_mut() else { return };
        let mut make = false;
        let mut cancel = false;
        let mut choose = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.label(RichText::new("A new extra").font(font(14.0, semibold())));
            ui.horizontal(|ui| {
                ui.label("Name");
                let hint = a.files.first().and_then(|f| f.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "e.g. “Glass Harbor (vocals)”".into());
                ui.add(egui::TextEdit::singleline(&mut a.name).hint_text(hint).desired_width(360.0));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Kind");
                for k in KINDS {
                    if widgets::chip(ui, k, a.kind == *k) {
                        a.kind = k.to_string();
                    }
                }
                ui.add(egui::TextEdit::singleline(&mut a.kind).desired_width(120.0));
            });
            ui.horizontal_wrapped(|ui| {
                choose = ui.button("Choose files…").clicked();
                ui.label(RichText::new("or paste copied files (Ctrl+V), or drop them on the window").color(P.faint).small());
            });
            let mut drop_file = None;
            for (n, f) in a.files.iter().enumerate() {
                ui.horizontal(|ui| {
                    let size = std::fs::metadata(f).map(|m| format!("{:.1} MB", m.len() as f64 / 1_000_000.0)).unwrap_or_else(|_| "can't be read".into());
                    ui.label(format!("{}  ·  {size}", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                    if ui.small_button("×").clicked() {
                        drop_file = Some(n);
                    }
                });
            }
            if let Some(n) = drop_file {
                a.files.remove(n);
            }
            ui.add(egui::TextEdit::multiline(&mut a.description).hint_text("Description (optional)").desired_rows(2).desired_width(f32::INFINITY));
            ui.add(egui::TextEdit::singleline(&mut a.source).hint_text("Source (optional): a link, or where it came from").desired_width(f32::INFINITY));
            // Who sees it: decided before it's made (nothing to leak in between).
            let like = if restricted { format!("like this {} (restricted to its groups)", owner.name()) } else { format!("like this {} (everyone)", owner.name()) };
            if shell.account.can("manage_access") {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Seen by");
                    if widgets::chip(ui, &like, a.seen == Seen::LikeTrack) {
                        a.seen = Seen::LikeTrack;
                    }
                    if widgets::chip(ui, "everyone", a.seen == Seen::Everyone) {
                        a.seen = Seen::Everyone;
                    }
                    if widgets::chip(ui, "only some groups…", matches!(a.seen, Seen::Groups(_))) && !matches!(a.seen, Seen::Groups(_)) {
                        a.seen = Seen::Groups(Vec::new());
                        ask_groups(shell, &self.groups);
                    }
                });
                if let Seen::Groups(chosen) = &mut a.seen {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("admins, and").color(P.faint));
                        match &*self.groups.lock().unwrap() {
                            Some(Ok(all)) if all.is_empty() => {
                                ui.label(RichText::new("loading the groups…").color(P.muted));
                            }
                            Some(Ok(all)) => {
                                for g in all.iter().filter(|g| g.name != "admin") {
                                    let on = chosen.contains(&g.id);
                                    if widgets::chip(ui, &g.name, on) {
                                        if on {
                                            chosen.retain(|x| *x != g.id);
                                        } else {
                                            chosen.push(g.id);
                                        }
                                    }
                                }
                            }
                            Some(Err(err)) => {
                                ui.label(RichText::new(err.as_str()).color(P.danger));
                            }
                            None => {}
                        }
                    });
                }
            } else {
                ui.label(RichText::new(format!("Seen {like}.")).color(P.muted).small());
            }
            ui.horizontal(|ui| {
                let name_ok = !a.name.trim().is_empty() || !a.files.is_empty();
                make = ui.add_enabled(!self.sending && name_ok, egui::Button::new("Attach")).clicked();
                cancel = ui.add_enabled(!self.sending, egui::Button::new("Cancel")).clicked();
            });
        });
        ui.add_space(8.0);
        if choose {
            self.chosen_for = None;
            crate::ui::files::choose(shell, "Files for the extra", ("Any file", &[]), true, &self.chosen);
        }
        if cancel {
            self.attaching = None;
        }
        if make && let Some(a) = self.attaching.as_ref() {
            let name = if a.name.trim().is_empty() { a.files.first().and_then(|f| f.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default() } else { a.name.trim().to_string() };
            let mut body = serde_json::json!({
                "name": name, "kind": a.kind.trim(), "links": [owner.link()],
                "description": a.description.trim(), "source": a.source.trim(),
            });
            match &a.seen {
                Seen::LikeTrack => {}
                Seen::Everyone => body["access"] = serde_json::json!({ "restricted": false }),
                Seen::Groups(groups) => body["access"] = serde_json::json!({ "restricted": true, "groups": groups }),
            }
            let files = a.files.clone();
            self.sending = true;
            self.result = None;
            self.uploading = files.len();
            self.progress.store(0, Ordering::Relaxed);
            let (api, ctx, answer, progress) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone(), self.progress.clone());
            shell.runtime.spawn(async move {
                let result = async {
                    let made: serde_json::Value = api.send_json(reqwest::Method::POST, "/api/extras", Some(body)).await.map_err(|e| e.message)?;
                    let id = made["id"].as_str().unwrap_or_default().to_string();
                    upload_all(&api, &id, &files, &progress, &ctx).await?;
                    Ok(format!("Attached “{name}”."))
                }
                .await;
                *answer.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        }
    }

    /// More files for an extra there already.
    fn upload(&mut self, shell: &Shell, extra: Uuid, files: Vec<PathBuf>, text: &str) {
        self.sending = true;
        self.result = None;
        self.uploading = files.len();
        self.progress.store(0, Ordering::Relaxed);
        let (api, ctx, answer, progress, text) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone(), self.progress.clone(), text.to_string());
        shell.runtime.spawn(async move {
            let result = upload_all(&api, &extra.to_string(), &files, &progress, &ctx).await.map(|_| text);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    fn send(&mut self, shell: &Shell, path: String, body: serde_json::Value, text: String) {
        self.sending = true;
        self.result = None;
        self.uploading = 0;
        let (api, ctx, answer) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<serde_json::Value>(reqwest::Method::POST, &path, Some(body)).await.map(|_| text).map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// Uploads files to an extra, one request each (each within the server's size limit).
pub async fn upload_all(api: &crate::api::Api, extra: &str, files: &[PathBuf], progress: &AtomicUsize, ctx: &egui::Context) -> Result<(), String> {
    for (n, path) in files.iter().enumerate() {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let bytes = tokio::fs::read(path).await.map_err(|e| format!("Couldn't read “{name}”: {e}"))?;
        api.upload::<serde_json::Value>(&format!("/api/extras/{extra}/files"), bytes, &name).await.map_err(|e| format!("“{name}”: {}", e.message))?;
        progress.store(n + 1, Ordering::Relaxed);
        ctx.request_repaint();
    }
    Ok(())
}

/// How many files, in words.
pub fn files_text(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{} files", thousands(n)) }
}
