//! Review's "Didn't import": files the server refused (missing tags…) or failed on, kept
//! with their audio (`GET /api/ingest-problems`). Left, the list; right, the chosen file:
//! where it came from, why, its tags, a play button, and what to do with it:
//!
//! * **add it to a track that's there** (`replace_file`): the track found by typing (the
//!   file's name is typed in to start with), then "make it this track's file" (its file
//!   becomes an older one) or "keep it as an older file of this track";
//! * **import it as a new track**, with the title and artists its tags lack typed in;
//! * **resolve it** with a note (nothing to import).
//!
//! A video (a music video that didn't import as a track) is kept as one: an extra of kind
//! "video", linked to the track chosen or to nothing yet; or every video shown at once.
//!
//! Adding or importing can say the file is an original file (an og file, named as it was
//! uploaded): on to start with for WAV files, which are nearly always the artist's own.
//! Review's filter works here too: words in the name, the path (folders), the reason or
//! the tags.
//!
//! Done ones leave the list at once (the server resolves them too).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::playback::{AudioSource, QueueItem};
use crate::ui::edit::inline_search::{Choice, InlineSearch};
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

const PAGE: usize = 500;
const ROW: f32 = 46.0;
const LIST_WIDTH: f32 = 380.0;

/// A file that didn't import.
#[derive(Debug, Clone, Deserialize)]
pub struct Problem {
    pub id: Uuid,
    /// "refused" (the file's fault: missing tags…) or "failed" (an error on our side).
    pub kind: String,
    pub size_bytes: i64,
    pub uploaded_filename: String,
    /// Where it was imported from, if known.
    pub path: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub embedded_tags: Option<serde_json::Value>,
    pub tag_title: Option<String>,
    #[serde(default)]
    pub tag_artists: Vec<String>,
    pub attempts: i32,
    pub created_by: Option<String>,
    pub last_attempt_at: String,
}

/// What an action on a file answered: (the file, what to say, the track it went to).
type Done = (Uuid, Result<(String, Option<Uuid>), String>);

#[derive(Default)]
pub struct NotImported {
    pub list: Vec<Problem>,
    pub loading: bool,
    pub error: Option<String>,
    asked: u64,
    arrived: Arc<Mutex<Option<(u64, Result<Vec<Problem>, String>)>>>,
    selected: Option<Uuid>,
    /// The track to add it to: found by typing, then chosen (a place in the catalog).
    search: Option<InlineSearch>,
    target: Option<u32>,
    reason: String,
    title: String,
    artists: String,
    /// Made an extra of the chosen track instead: its kind (guessed from the file's name).
    extra_kind: String,
    /// It's an original file (an og file), with this name: on to start with for WAVs.
    is_og: bool,
    og_name: String,
    /// The form was filled for this file (moving to another fills it again).
    filled_for: Option<Uuid>,
    sending: bool,
    done: Arc<Mutex<Option<Done>>>,
    /// The last thing done (or why not), with the track it went to.
    notice: Option<(Result<String, String>, Option<Uuid>)>,
    /// Videos being kept as music videos, many at once: how many done, of how many.
    batch: Option<(Arc<std::sync::atomic::AtomicUsize>, usize)>,
    batch_done: Arc<Mutex<Option<String>>>,
}

impl NotImported {
    /// Fetches every open entry in the background.
    pub fn refresh(&mut self, shell: &Shell) {
        self.asked += 1;
        self.loading = true;
        let (api, arrived, ctx, number) = (shell.api.clone(), self.arrived.clone(), shell.ctx.clone(), self.asked);
        shell.runtime.spawn(async move {
            let result = async {
                let mut all: Vec<Problem> = Vec::new();
                loop {
                    let before = all.last().map(|p| format!("&before={}", p.id)).unwrap_or_default();
                    let page: Vec<Problem> = api.send_json(reqwest::Method::GET, &format!("/api/ingest-problems?limit={PAGE}{before}"), None).await?;
                    let done = page.len() < PAGE;
                    all.extend(page);
                    if done {
                        return Ok(all);
                    }
                }
            }
            .await
            .map_err(|err: crate::api::ApiError| err.message);
            *arrived.lock().unwrap() = Some((number, result));
            ctx.request_repaint();
        });
    }

    /// Takes in what arrived. Call once per frame.
    pub fn poll(&mut self, shell: &mut Shell) {
        let arrived = self.arrived.lock().unwrap().take();
        if let Some((number, result)) = arrived
            && number == self.asked
        {
            self.loading = false;
            match result {
                Ok(list) => {
                    self.list = list;
                    self.error = None;
                }
                Err(err) => self.error = Some(err),
            }
        }
        let done = self.done.lock().unwrap().take();
        if let Some((problem, result)) = done {
            self.sending = false;
            match result {
                Ok((text, track)) => {
                    // Gone from the list at once; on to the one in its place.
                    let at = self.list.iter().position(|p| p.id == problem);
                    self.list.retain(|p| p.id != problem);
                    if let Some(at) = at {
                        self.selected = self.list.get(at.min(self.list.len().saturating_sub(1))).map(|p| p.id);
                    }
                    if let Some(track) = track {
                        shell.track_details.refresh(track);
                    }
                    shell.act(Action::RefreshLibrary);
                    self.notice = Some((Ok(text), track));
                }
                Err(err) => self.notice = Some((Err(err), None)),
            }
        }
    }

    /// How many match `filter` (words anywhere in a file's name, path, reason or tags).
    pub fn matching(&self, filter: &str) -> usize {
        let words = words(filter);
        self.list.iter().filter(|p| matches(p, &words)).count()
    }

    /// The list (those matching `filter`) and the chosen file, in `rect`.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, filter: &str) {
        let words = words(filter);
        let shown: Vec<usize> = (0..self.list.len()).filter(|&n| matches(&self.list[n], &words)).collect();
        if self.selected.is_none_or(|id| !shown.iter().any(|&n| self.list[n].id == id)) {
            self.selected = shown.first().map(|&n| self.list[n].id);
        }
        // Ctrl+↓/↑: the next or previous file (anywhere, as on the rest of Review).
        let (next, previous) = ui.ctx().input_mut(|i| (i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::COMMAND, egui::Key::ArrowUp)));
        if (next || previous) && !shell.modal_open {
            let at = self.selected.and_then(|id| shown.iter().position(|&n| self.list[n].id == id)).unwrap_or(0);
            let to = if next { (at + 1).min(shown.len().saturating_sub(1)) } else { at.saturating_sub(1) };
            self.selected = shown.get(to).map(|&n| self.list[n].id);
        }
        // Videos among those shown: kept as music videos, all at once.
        let videos: Vec<(Uuid, String)> = shown.iter().map(|&n| &self.list[n]).filter(|p| is_video(&p.uploaded_filename)).map(|p| (p.id, searchable_name(&p.uploaded_filename))).collect();
        let batch_done = self.batch_done.lock().unwrap().take();
        if let Some(text) = batch_done {
            self.batch = None;
            self.notice = Some((Ok(text), None));
            shell.act(Action::RefreshLibrary);
            self.refresh(shell);
        }
        if !videos.is_empty() || self.batch.is_some() {
            ui.horizontal(|ui| match &self.batch {
                Some((done, of)) => {
                    ui.label(RichText::new(format!("Keeping videos as music videos… {} of {of}", done.load(std::sync::atomic::Ordering::Relaxed))).color(P.muted));
                }
                None => {
                    let tip = "Each an extra of kind “video”, linked to nothing yet (Review: “Unlinked extras”), with its thumbnail";
                    if ui.button(format!("Keep the {} videos shown as music videos", videos.len())).on_hover_text(tip).clicked() {
                        self.keep_videos(shell, videos.clone());
                    }
                }
            });
            ui.add_space(6.0);
        }
        let rest = ui.available_rect_before_wrap();
        let list_rect = egui::Rect::from_min_size(rest.min, Vec2::new(LIST_WIDTH, rest.height()));
        let detail_rect = egui::Rect::from_min_max(egui::pos2(list_rect.right() + 24.0, rest.top()), rest.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        self.list(&mut list_ui, &shown);
        let mut detail_ui = ui.new_child(egui::UiBuilder::new().max_rect(detail_rect));
        self.detail(&mut detail_ui, shell, catalog);
        ui.allocate_rect(rest, Sense::hover());
    }

    fn list(&mut self, ui: &mut egui::Ui, shown: &[usize]) {
        if shown.is_empty() {
            let text = if self.loading {
                "Loading…".to_string()
            } else if let Some(err) = &self.error {
                format!("Couldn't load them: {err}")
            } else if !self.list.is_empty() {
                "Nothing matches the filter.".to_string()
            } else {
                "Every file imported.".to_string()
            };
            ui.label(RichText::new(text).color(P.muted));
            return;
        }
        egui::ScrollArea::vertical().id_salt("not-imported-list").auto_shrink([false, false]).show_rows(ui, ROW, shown.len(), |ui, range| {
            for n in range {
                let p = &self.list[shown[n]];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                let chosen = self.selected == Some(p.id);
                if chosen || response.hovered() {
                    ui.painter().rect_filled(rect, 8, if chosen { P.accent_soft } else { P.surface });
                }
                let painter = ui.painter();
                widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(10.0, 6.0), Vec2::new(rect.width() - 20.0, 18.0)), &p.uploaded_filename, font(13.5, medium()), P.text);
                let sub = p.path.as_deref().map(folder).filter(|f| !f.is_empty()).map(|f| format!("{} · {f}", p.reason)).unwrap_or_else(|| p.reason.clone());
                widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(10.0, 25.0), Vec2::new(rect.width() - 20.0, 16.0)), &sub, font(12.0, medium()), P.muted);
                if response.clicked() {
                    self.selected = Some(p.id);
                }
            }
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        let Some(p) = self.selected.and_then(|id| self.list.iter().find(|p| p.id == id)).cloned() else {
            self.notice_line(ui, shell);
            return;
        };
        // A new file chosen: its name typed in to find its track, its tags (or name) in the
        // import fields.
        if self.filled_for != Some(p.id) {
            self.filled_for = Some(p.id);
            let name = searchable_name(&p.uploaded_filename);
            let mut search = InlineSearch::new(Part::Tracks);
            search.set_query(&name, shell);
            self.search = Some(search);
            self.target = None;
            self.reason.clear();
            self.title = p.tag_title.clone().unwrap_or_else(|| name.clone());
            self.artists = p.tag_artists.join("; ");
            self.is_og = is_wav(&p.uploaded_filename);
            self.extra_kind = guess_kind(&p.uploaded_filename).to_string();
            self.og_name = p.uploaded_filename.clone();
        }
        egui::ScrollArea::vertical().id_salt(("not-imported-detail", p.id)).auto_shrink([false, false]).show(ui, |ui| {
            self.notice_line(ui, shell);
            // What it is, where it came from, why it didn't import.
            ui.horizontal(|ui| {
                if play(ui, "not-imported-play", 34.0) {
                    let item = QueueItem {
                        track_id: p.id,
                        file_id: p.id,
                        source: AudioSource::NotImported,
                        title: p.uploaded_filename.clone(),
                        artist: if p.tag_artists.is_empty() { "didn't import".into() } else { p.tag_artists.join(", ") },
                        album: None,
                        duration_ms: None,
                        cover: None,
                    };
                    let controller = shell.controller.clone();
                    shell.runtime.spawn(async move { controller.play(vec![item], 0).await });
                }
                ui.label(RichText::new(&p.uploaded_filename).font(font(22.0, bold())));
            });
            let mut facts = vec![p.kind.clone(), format!("{:.1} MB", p.size_bytes as f64 / 1_000_000.0), p.last_attempt_at.get(..10).unwrap_or(&p.last_attempt_at).to_string()];
            if p.attempts > 1 {
                facts.push(format!("tried {} times", p.attempts));
            }
            if let Some(by) = &p.created_by {
                facts.push(by.clone());
            }
            ui.label(RichText::new(facts.join("  ·  ")).color(P.muted));
            if let Some(path) = &p.path {
                ui.add(egui::Label::new(RichText::new(path).monospace().color(P.faint)).selectable(true));
            }
            ui.label(RichText::new(&p.reason).color(P.danger));
            tags(ui, &p);
            ui.add_space(12.0);
            // For either way in (added to a track, or imported).
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.is_og, "It's an original file (og file)").on_hover_text("The file as the artist made and named it, not a rip or a download. On to start with for WAV files");
                if self.is_og {
                    ui.label(RichText::new("named").color(P.muted));
                    ui.add(egui::TextEdit::singleline(&mut self.og_name).desired_width(360.0));
                }
            });
            ui.add_space(12.0);

            // A video: kept as a music video (linked to the track chosen below, if one is).
            if is_video(&p.uploaded_filename) {
                let target = self.target.and_then(|i| catalog.tracks.get(i as usize)).map(|t| (t.id, t.title.clone()));
                egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.label(RichText::new("It's a video").font(font(15.0, semibold())));
                    let whose = match &target {
                        Some((_, title)) => format!("It's kept as a music video of “{title}” (the track chosen below)."),
                        None => "It's kept as a music video linked to nothing yet (choose its track below first, or link it later: Review, “Unlinked extras”).".into(),
                    };
                    ui.label(RichText::new(whose).color(P.muted));
                    if ui.add_enabled(!self.sending, egui::Button::new("Keep it as a music video")).clicked() {
                        let links: Vec<serde_json::Value> = target.iter().map(|(id, _)| serde_json::json!({ "kind": "track", "id": id })).collect();
                        let body = serde_json::json!({ "name": searchable_name(&p.uploaded_filename), "kind": "video", "links": links, "reason": self.reason.trim() });
                        self.send(shell, p.id, format!("/api/ingest-problems/{}/extra", p.id), body, target.as_ref().map(|t| t.0), "Kept as a music video.".into(), true);
                    }
                });
                ui.add_space(14.0);
            }
            if shell.account.can("replace_file") {
                self.add_to_track(ui, shell, catalog, &p);
                ui.add_space(14.0);
            }
            self.import(ui, shell, &p);
            ui.add_space(14.0);
            ui.label(RichText::new("Or: nothing to import").font(font(15.0, semibold())));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("what you did (optional): not music, a duplicate…").desired_width(360.0));
                if ui.add_enabled(!self.sending, egui::Button::new("Resolve")).clicked() {
                    let body = serde_json::json!({ "resolution": self.reason.trim() });
                    self.send(shell, p.id, format!("/api/ingest-problems/{}/resolve", p.id), body, None, "Resolved.".into(), false);
                }
            });
        });
    }

    /// The last thing done: what, with a link to the track it went to.
    fn notice_line(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let Some((result, track)) = &self.notice else { return };
        ui.horizontal(|ui| {
            match result {
                Ok(text) => ui.label(RichText::new(text.as_str()).color(P.accent)),
                Err(err) => ui.label(RichText::new(err.as_str()).color(P.danger)),
            };
            if let Some(track) = track
                && widgets::link(ui, "open the track", font(13.0, medium()), P.muted)
            {
                shell.act(Action::Go(Page::Track(*track)));
            }
        });
        ui.add_space(8.0);
    }

    /// "Add it to a track that's there": the track, then as its file or an older one.
    fn add_to_track(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, p: &Problem) {
        ui.label(RichText::new("Add it to a track that's there").font(font(15.0, semibold())));
        ui.label(RichText::new("The same song already in the library (a better copy without tags, an older version…). Found by its name; type to find another.").color(P.muted));
        if let Some(search) = self.search.as_mut() {
            let hit = search.show(ui, shell, Some(catalog), egui::Id::new("not-imported-track"), "the track: type its title or artist", &[]);
            if let Some((Choice::Hit(i), _)) = hit {
                self.target = Some(i);
            }
        }
        let Some(i) = self.target else { return };
        let t = &catalog.tracks[i as usize];
        ui.add_space(6.0);
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if play(ui, "not-imported-play-track", 28.0) {
                    shell.act(Action::Play { tracks: vec![i], start: 0 });
                }
                ui.vertical(|ui| {
                    if widgets::link(ui, &t.title, font(15.0, semibold()), P.text) {
                        shell.act(Action::Go(Page::Track(t.id)));
                    }
                    let album = catalog.main_album(t).map(|a| format!(" · {}", a.title)).unwrap_or_default();
                    let codec = t.codec.as_deref().map(|c| format!(" · {c}")).unwrap_or_default();
                    ui.label(RichText::new(format!("{}{album}{codec}", t.artist_display)).color(P.muted));
                });
            });
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with the file)").desired_width(420.0));
            ui.horizontal(|ui| {
                let current = ui
                    .add_enabled(!self.sending, egui::Button::new("Make it this track's file"))
                    .on_hover_text("This file plays from now on; the track's file now is kept as an older one (nothing is deleted)")
                    .clicked();
                let older = ui
                    .add_enabled(!self.sending, egui::Button::new("Keep it as an older file of this track"))
                    .on_hover_text("The track keeps playing its file; this one is kept with it, as an earlier file")
                    .clicked();
                if current || older {
                    let add_as = if current { "current_file" } else { "previous_file" };
                    let body = serde_json::json!({ "track_id": t.id, "add_as": add_as, "reason": self.reason.trim(), "original_filename": self.og() });
                    let text = if current { format!("“{}” plays this file now (its old file is kept as an older one).", t.title) } else { format!("Kept as an older file of “{}”.", t.title) };
                    self.send(shell, p.id, format!("/api/ingest-problems/{}/add-to-track", p.id), body, Some(t.id), text, true);
                }
            });
            // Not the song itself but something of it (vocals, a stem…): an extra of it.
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Or an extra of it:").color(P.muted));
                for k in crate::ui::edit::extras::KINDS {
                    if widgets::chip(ui, k, self.extra_kind == *k) {
                        self.extra_kind = k.to_string();
                    }
                }
                let make = ui
                    .add_enabled(!self.sending, egui::Button::new("Make it an extra of this track"))
                    .on_hover_text("Kept with the track (vocals, a stem…), not a track of its own; seen like the track")
                    .clicked();
                if make {
                    let name = searchable_name(&p.uploaded_filename);
                    let body = serde_json::json!({ "name": name, "kind": self.extra_kind, "links": [{ "kind": "track", "id": t.id }], "reason": self.reason.trim() });
                    let text = format!("Made an extra ({}) of “{}”.", self.extra_kind, t.title);
                    self.send(shell, p.id, format!("/api/ingest-problems/{}/extra", p.id), body, Some(t.id), text, true);
                }
            });
        });
    }

    /// "Import it as a new track": what its tags lack, typed in.
    fn import(&mut self, ui: &mut egui::Ui, shell: &mut Shell, p: &Problem) {
        ui.label(RichText::new("Import it as a new track").font(font(15.0, semibold())));
        ui.label(RichText::new("With what its tags lack typed in (the file isn't changed; the track's first revision says what was typed).").color(P.muted));
        egui::Grid::new(("not-imported-import", p.id)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Title");
            ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(420.0));
            ui.end_row();
            ui.label("Artists");
            ui.add(egui::TextEdit::singleline(&mut self.artists).hint_text("separated by ;").desired_width(420.0));
            ui.end_row();
        });
        let ready = !self.title.trim().is_empty() && !self.artists.trim().is_empty();
        if ui.add_enabled(!self.sending && ready, egui::Button::new("Import")).clicked() {
            // Only what differs from the tags counts as given by hand.
            let title = self.title.trim();
            let title = (Some(title) != p.tag_title.as_deref()).then(|| title.to_string());
            let artists = self.artists.trim();
            let artists: Vec<String> = if artists != p.tag_artists.join("; ") { vec![artists.to_string()] } else { Vec::new() };
            let body = serde_json::json!({ "title": title, "artists": artists, "original_filename": self.og() });
            self.send(shell, p.id, format!("/api/ingest-problems/{}/import", p.id), body, None, "Imported.".into(), true);
        }
    }

    /// Videos kept as music videos, one after another (each gets its facts and thumbnail
    /// on the server): extras of kind "video", linked to nothing yet.
    fn keep_videos(&mut self, shell: &Shell, videos: Vec<(Uuid, String)>) {
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.batch = Some((done.clone(), videos.len()));
        let (api, ctx, answer) = (shell.api.clone(), shell.ctx.clone(), self.batch_done.clone());
        shell.runtime.spawn(async move {
            let mut failed = Vec::new();
            for (id, name) in &videos {
                let body = serde_json::json!({ "name": name, "kind": "video", "reason": "a music video that didn't import" });
                if let Err(err) = api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/ingest-problems/{id}/extra"), Some(body)).await {
                    failed.push(format!("{name}: {}", err.message));
                }
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ctx.request_repaint();
            }
            let kept = videos.len() - failed.len();
            let mut text = format!("{kept} videos kept as music videos (Review: “Unlinked extras” to link them).");
            if !failed.is_empty() {
                text.push_str(&format!(" Not these: {}", failed.join("; ")));
            }
            *answer.lock().unwrap() = Some(text);
            ctx.request_repaint();
        });
    }

    /// The original filename to send: the name, if it's said to be an original file.
    fn og(&self) -> Option<String> {
        Some(self.og_name.trim().to_string()).filter(|n| self.is_og && !n.is_empty())
    }

    /// Sends an action on `problem`; `track`: the track it goes to (known, or from the
    /// answer's `track_id`). `answers`: the server answers with JSON (resolving doesn't).
    #[allow(clippy::too_many_arguments)]
    fn send(&mut self, shell: &Shell, problem: Uuid, path: String, body: serde_json::Value, track: Option<Uuid>, text: String, answers: bool) {
        self.sending = true;
        self.notice = None;
        let (api, ctx, done) = (shell.api.clone(), shell.ctx.clone(), self.done.clone());
        shell.runtime.spawn(async move {
            let answer = if answers {
                api.send_json::<serde_json::Value>(reqwest::Method::POST, &path, Some(body)).await.map(Some)
            } else {
                api.send(reqwest::Method::POST, &path, Some(body)).await.map(|_| None)
            };
            let result = answer
                .map(|answer| {
                    let track = track.or_else(|| answer.as_ref()?.get("track_id")?.as_str()?.parse().ok());
                    (text, track)
                })
                .map_err(|e| e.message);
            *done.lock().unwrap() = Some((problem, result));
            ctx.request_repaint();
        });
    }
}

/// A round play button, `size` across, in the layout.
fn play(ui: &mut egui::Ui, id: &str, size: f32) -> bool {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    widgets::icon_button(ui, id, rect.center(), size / 2.0, widgets::Icon::Play, true)
}

/// The file's tags, as it had them (folded).
fn tags(ui: &mut egui::Ui, p: &Problem) {
    let Some(formats) = p.embedded_tags.as_ref().and_then(|t| t.get("tags")).and_then(|t| t.as_array()).filter(|t| !t.is_empty()) else {
        ui.label(RichText::new("No tags in the file.").color(P.faint));
        return;
    };
    egui::CollapsingHeader::new(RichText::new("Tags in the file").color(P.muted)).id_salt(("not-imported-tags", p.id)).show(ui, |ui| {
        for format in formats {
            ui.label(RichText::new(format.get("format").and_then(|f| f.as_str()).unwrap_or("?")).font(font(12.5, semibold())).color(P.faint));
            for item in format.get("items").and_then(|i| i.as_array()).into_iter().flatten() {
                let key = item.get("key").and_then(|k| k.as_str()).unwrap_or("");
                let value = match item.get("value") {
                    Some(serde_json::Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                ui.add(egui::Label::new(RichText::new(format!("{key}: {value}")).monospace().small()).selectable(true));
            }
        }
    });
}

/// An extra's kind, guessed from a file's name ("… (vocals).wav" → vocals).
fn guess_kind(filename: &str) -> &'static str {
    let name = filename.to_lowercase();
    if ["acapella", "a cappella", "acappella", "vocal", "vox"].iter().any(|w| name.contains(w)) {
        "vocals"
    } else if ["instrumental", "inst.", "(inst)", " inst "].iter().any(|w| name.contains(w)) {
        "instrumental"
    } else if name.contains("stem") {
        "stems"
    } else if name.contains("sample") {
        "sample"
    } else {
        "other"
    }
}

/// A video file (by its name).
fn is_video(filename: &str) -> bool {
    std::path::Path::new(filename).extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "mp4" | "m4v" | "mkv" | "webm" | "mov" | "avi"))
}

/// A WAV file (by its name): nearly always an artist's own file, not a rip.
fn is_wav(filename: &str) -> bool {
    std::path::Path::new(filename).extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "wav" | "wave"))
}

/// A filter's words, lowercase.
fn words(filter: &str) -> Vec<String> {
    filter.to_lowercase().split_whitespace().map(str::to_string).collect()
}

/// Whether every word is in the file's name, path, reason or tags' title and artists.
fn matches(p: &Problem, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let text = format!(
        "{}\n{}\n{}\n{}\n{}",
        p.uploaded_filename,
        p.path.as_deref().unwrap_or(""),
        p.reason,
        p.tag_title.as_deref().unwrap_or(""),
        p.tag_artists.join("\n")
    )
    .to_lowercase();
    words.iter().all(|w| text.contains(w.as_str()))
}

/// A file's name as something to search for: no extension, underscores as spaces.
fn searchable_name(filename: &str) -> String {
    let stem = std::path::Path::new(filename).file_stem().and_then(|s| s.to_str()).unwrap_or(filename);
    stem.replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The folder of a path ("/music/a/b.flac" → "a").
fn folder(path: &str) -> &str {
    let dir = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    dir.rsplit_once('/').map(|(_, last)| last).unwrap_or(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_to_search_for() {
        assert_eq!(searchable_name("glass_harbor_MASTER_v7.wav"), "glass harbor MASTER v7");
        assert_eq!(searchable_name("STFU (Prod5.10).mp3"), "STFU (Prod5.10)");
        assert_eq!(searchable_name("no extension"), "no extension");
        assert_eq!(folder("/music/some artist/song.flac"), "some artist");
        assert!(is_wav("MASTER.WAV") && is_wav("a.wav") && !is_wav("a.flac") && !is_wav("wav"));
        assert!(is_video("Clip (Official Video).MP4") && is_video("a.mkv") && !is_video("a.m4a"));
        assert_eq!(guess_kind("Glass Harbor (Acapella).wav"), "vocals");
        assert_eq!(guess_kind("song_VOX_dry.wav"), "vocals");
        assert_eq!(guess_kind("song (Instrumental).flac"), "instrumental");
        assert_eq!(guess_kind("drums stem.wav"), "stems");
        assert_eq!(guess_kind("bounce v3.wav"), "other");
    }

    #[test]
    fn the_filter_looks_in_the_name_path_reason_and_tags() {
        let p = Problem {
            id: Uuid::nil(),
            kind: "refused".into(),
            size_bytes: 1,
            uploaded_filename: "bounce v3.wav".into(),
            path: Some("/music/Verdigrix/unreleased/bounce v3.wav".into()),
            reason: "missing required tags: title, artist".into(),
            embedded_tags: None,
            tag_title: None,
            tag_artists: vec!["Mira Vale".into()],
            attempts: 1,
            created_by: None,
            last_attempt_at: String::new(),
        };
        assert!(matches(&p, &words("verdigrix unreleased")));
        assert!(matches(&p, &words("BOUNCE mira")));
        assert!(matches(&p, &words("missing")));
        assert!(!matches(&p, &words("verdigrix demo")));
        assert!(matches(&p, &words("")));
    }

    #[test]
    fn entries_from_the_server_parse() {
        let p: Problem = serde_json::from_value(serde_json::json!({
            "id": "01a0e7f5-8c03-7670-a987-3780c26c309d", "kind": "refused", "sha256": "ab", "size_bytes": 5,
            "uploaded_filename": "a.wav", "path": null, "reason": "missing required tags: title, artist",
            "description": null, "embedded_tags": null, "tag_title": null, "tag_artists": [], "attempts": 1,
            "created_at": "2026-09-29T21:27:27Z", "created_by": "admin", "last_attempt_at": "2026-09-29T21:27:27Z",
            "resolved_at": null, "resolved_by": null, "resolution": "", "track_id": null
        }))
        .unwrap();
        assert_eq!(p.uploaded_filename, "a.wav");
    }
}
