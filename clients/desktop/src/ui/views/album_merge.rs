//! Merging two albums that are the same (split by ingest because the album artists or the
//! date differ, say), from the album page in edit mode; and undoing a merge.
//!
//! Merging: the two side by side, the one on the left merged into the one on the right
//! (`s` swaps). What happens is said before anything does: the left one's tracks (each
//! with a revision, their disc and track numbers kept), covers, sources and share links
//! move to the right one, which keeps its own title, kind, date and credits; the left one
//! then only redirects. Tracks with the same title on both are pointed out: the album
//! will have both copies, to merge afterwards. Ctrl+Enter merges
//! (`POST /api/albums/{left}/merge {into: right}`).
//!
//! Undoing: the server's preview first (`GET /api/album-merges/{id}/undo`, which changes
//! nothing), all of it: what goes back, what stays and why, what's in the way. Only then
//! the button (`POST` the same).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

/// What happened, for the album page.
pub enum Outcome {
    /// Merged: go to the album that stays.
    Merged { kept: Uuid, gone: Uuid },
    Closed,
}

pub struct AlbumMergeView {
    /// Merged away.
    pub from: Uuid,
    /// Stays.
    pub into: Uuid,
    reason: String,
    merging: bool,
    error: Option<String>,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
}

impl AlbumMergeView {
    pub fn new(from: Uuid, into: Uuid) -> AlbumMergeView {
        AlbumMergeView { from, into, reason: String::new(), merging: false, error: None, answer: Arc::default() }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog) -> Option<Outcome> {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.merging = false;
            match result {
                Ok(()) => return Some(Outcome::Merged { kept: self.into, gone: self.from }),
                Err(err) => self.error = Some(err),
            }
        }
        let (Some(from), Some(into)) = (catalog.album(self.from), catalog.album(self.into)) else {
            ui.label(RichText::new("One of the two albums isn't in the library (anymore).").color(P.muted));
            return ui.button("Close").clicked().then_some(Outcome::Closed);
        };

        // ---- Keys ----
        if !shell.palette_open {
            let ctx = ui.ctx().clone();
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                self.merge(shell);
            }
            if !ctx.egui_wants_keyboard_input() {
                let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
                if key(Key::S) {
                    std::mem::swap(&mut self.from, &mut self.into);
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                } else if key(Key::Escape) {
                    return Some(Outcome::Closed);
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Merge albums").font(font(20.0, bold())));
            ui.label(RichText::new("the left one is merged into the right one, which stays").color(P.muted));
            if ui.small_button("Swap  (s)").clicked() {
                std::mem::swap(&mut self.from, &mut self.into);
            }
        });
        ui.add_space(8.0);
        let width = ((ui.available_width() - 16.0) / 2.0).max(220.0);
        ui.horizontal_top(|ui| {
            for (album, label, color) in [(from, "Merged away", P.muted), (into, "Stays", P.accent)] {
                egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(RichText::new(label).font(font(12.0, semibold())).color(color));
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
                        shell.cover(ui, album.cover(), album.id, rect, 6, 0.1);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&album.title).font(font(16.0, semibold())));
                            ui.label(RichText::new(&album.artist_display).color(P.muted));
                            let mut facts = vec![album.kind.clone()];
                            if let Some(date) = &album.release_date {
                                facts.push(date.clone());
                            }
                            let n = catalog.album_tracks(album.id).len();
                            facts.push(if n == 1 { "1 track".into() } else { format!("{n} tracks") });
                            ui.label(RichText::new(facts.join(" · ")).color(P.faint));
                        });
                    });
                });
            }
        });

        // ---- What happens ----
        ui.add_space(8.0);
        let moving = catalog.album_tracks(from.id).len();
        ui.label(
            RichText::new(format!(
                "“{}”'s {} (each with a revision, their disc and track numbers kept), covers, sources and share links move to “{}”, which keeps its own title, kind, date and credits. “{}” then only leads to it. Undo is on “{}”'s page.",
                from.title,
                if moving == 1 { "track".to_string() } else { format!("{moving} tracks") },
                into.title,
                from.title,
                into.title
            ))
            .color(P.text),
        );
        // Same titles on both: those will be possible duplicates, to merge afterwards.
        let staying: Vec<String> = catalog.album_tracks(into.id).iter().map(|&i| catalog.tracks[i as usize].title.to_lowercase()).collect();
        let doubled: Vec<&str> = catalog
            .album_tracks(from.id)
            .iter()
            .map(|&i| catalog.tracks[i as usize].title.as_str())
            .filter(|t| staying.contains(&t.to_lowercase()))
            .collect();
        if !doubled.is_empty() {
            ui.label(
                RichText::new(format!(
                    "On both: {}. After the merge the album has both copies of each: merge those tracks afterwards (from a track's page in edit mode, or Ctrl+M on Review where they're flagged as possible duplicates).",
                    doubled.join(", ")
                ))
                .color(P.muted),
            );
        }

        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional)").desired_width(420.0));
        let mut outcome = None;
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.merging, egui::Button::new("Merge  (Ctrl+Enter)")).clicked() {
                self.merge(shell);
            }
            if ui.button("Close  (Esc)").clicked() {
                outcome = Some(Outcome::Closed);
            }
            if self.merging {
                ui.label(RichText::new("Merging…").color(P.muted));
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(P.danger));
        }
        outcome
    }

    fn merge(&mut self, shell: &Shell) {
        if self.merging {
            return;
        }
        self.merging = true;
        self.error = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let (from, body) = (self.from, serde_json::json!({ "into": self.into, "reason": self.reason.trim() }));
        shell.runtime.spawn(async move {
            let result = api
                .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/albums/{from}/merge"), Some(body))
                .await
                .map(|_| ())
                .map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// What undoing an album merge would do (or did), as the server says it.
#[derive(Debug, Clone, Deserialize)]
pub struct UndoReport {
    pub from_album_id: Uuid,
    pub from_title: String,
    pub into_title: String,
    /// Why it can't be undone (empty: it can).
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub tracks: Vec<TrackUndo>,
    #[serde(default)]
    pub hidden_tracks_back: usize,
    #[serde(default)]
    pub hidden_tracks_staying: usize,
    #[serde(default)]
    pub goes_back: Vec<String>,
    #[serde(default)]
    pub stays: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackUndo {
    pub title: String,
    pub goes_back: bool,
    pub detail: String,
}

/// Undoing merges into an album: "Undo…" fetches the preview, shows it whole, and only
/// then offers to do it.
#[derive(Default)]
pub struct UndoPanel {
    /// The merge being looked at, and its preview (None: loading).
    open: Option<(Uuid, Option<Result<UndoReport, String>>)>,
    doing: bool,
    answer: Arc<Mutex<Option<(Uuid, bool, Result<UndoReport, String>)>>>,
    /// A finished undo: the album that came back.
    done: Option<(Uuid, String)>,
}

impl UndoPanel {
    /// The merges into album `album`, each with Undo… (`can`: allowed to).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, album: Uuid, merges: &[crate::details::AlbumMerge], can: bool) {
        if let Some((merge, done, result)) = self.answer.lock().unwrap().take() {
            self.doing = false;
            match (done, result) {
                (true, Ok(report)) => {
                    self.open = None;
                    self.done = Some((report.from_album_id, report.from_title.clone()));
                    shell.album_details.refresh(album);
                    shell.act(Action::RefreshLibrary);
                }
                (_, result) => {
                    if self.open.as_ref().is_some_and(|(m, _)| *m == merge) {
                        self.open = Some((merge, Some(result)));
                    }
                }
            }
        }
        for m in merges {
            let title = m.from_title.clone().unwrap_or_else(|| "(an album you can't see)".into());
            let by = m.merged_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
            let why = Some(m.reason.trim()).filter(|r| !r.is_empty()).map(|r| format!(" ({r})")).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{title} · {}{by}{why}", m.merged_at.get(..10).unwrap_or(&m.merged_at)));
                if m.undone_at.is_some() {
                    ui.label(RichText::new("· undone").color(P.faint));
                    if widgets::link(ui, "its page", font(13.0, egui::FontFamily::Proportional), P.muted) {
                        shell.act(Action::Go(Page::Album(m.from_album_id)));
                    }
                } else if can && self.open.as_ref().is_none_or(|(o, _)| *o != m.id) && ui.small_button("Undo…").clicked() {
                    self.open = Some((m.id, None));
                    self.done = None;
                    self.ask(shell, m.id, false);
                }
            });
            if let Some((open, preview)) = &self.open
                && *open == m.id
            {
                match preview {
                    None => {
                        ui.label(RichText::new("Asking the server what undoing it would do…").color(P.faint));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(format!("Couldn't get the preview: {err}")).color(P.danger));
                    }
                    Some(Ok(report)) => {
                        let report = report.clone();
                        if self.preview(ui, shell, m.id, &report) {
                            self.open = None;
                        }
                    }
                }
            }
        }
        if let Some((back, title)) = self.done.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Undone: “{title}” is an album of its own again.")).color(P.accent));
                if widgets::link(ui, "Go to it", font(13.0, egui::FontFamily::Proportional), P.text) {
                    shell.act(Action::Go(Page::Album(back)));
                }
            });
        }
    }

    /// The preview, all of it, and the button. True: cancelled.
    fn preview(&mut self, ui: &mut egui::Ui, shell: &Shell, merge: Uuid, r: &UndoReport) -> bool {
        let mut cancelled = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.label(RichText::new(format!("Undoing gives “{}” back as its own album, out of “{}”:", r.from_title, r.into_title)).color(P.text));
            if !r.blockers.is_empty() {
                ui.label(RichText::new("It can't be undone:").color(P.danger));
                for b in &r.blockers {
                    ui.label(RichText::new(format!("• {b}")).color(P.danger));
                }
            }
            for t in &r.tracks {
                let (mark, color) = if t.goes_back { ("goes back", P.text) } else { ("stays", P.muted) };
                ui.label(RichText::new(format!("• {} — {mark}: {}", t.title, t.detail)).color(color));
            }
            if r.hidden_tracks_back + r.hidden_tracks_staying > 0 {
                ui.label(RichText::new(format!("Tracks you can't see: {} go back, {} stay.", r.hidden_tracks_back, r.hidden_tracks_staying)).color(P.muted));
            }
            for g in &r.goes_back {
                ui.label(RichText::new(format!("Goes back: {g}")).color(P.text));
            }
            for s in &r.stays {
                ui.label(RichText::new(format!("Stays: {s}")).color(P.muted));
            }
            ui.horizontal(|ui| {
                if r.blockers.is_empty() {
                    if self.doing {
                        ui.label(RichText::new("Undoing…").color(P.muted));
                    } else if ui.button("Undo the merge").clicked() {
                        self.doing = true;
                        self.ask(shell, merge, true);
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        cancelled
    }

    /// The preview (`doit` false) or the undo itself.
    fn ask(&self, shell: &Shell, merge: Uuid, doit: bool) {
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let path = format!("/api/album-merges/{merge}/undo");
            let result = if doit {
                api.send_json::<UndoReport>(reqwest::Method::POST, &path, Some(serde_json::json!({ "reason": "" }))).await
            } else {
                api.send_json::<UndoReport>(reqwest::Method::GET, &path, None).await
            };
            *answer.lock().unwrap() = Some((merge, doit, result.map_err(|err| err.message)));
            ctx.request_repaint();
        });
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_ALBUM_FROM` and `ACETATE_TEST_ALBUM_INTO`. Merges the
/// first into the second, reads the undo preview, and undoes it: a throwaway database.
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;

    #[test]
    #[ignore]
    fn merging_and_undoing_albums() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_ALBUM_FROM", "ACETATE_TEST_ALBUM_INTO"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(from), Ok(into)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-album-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let body = serde_json::json!({ "into": into, "reason": "album merge test" });
            let merged: serde_json::Value = api.send_json(reqwest::Method::POST, &format!("/api/albums/{from}/merge"), Some(body)).await.unwrap();
            let merge = merged["merge_id"].as_str().unwrap().to_string();
            // The kept album lists it, as the page reads it.
            let kept: crate::details::AlbumDetail = api.send_json(reqwest::Method::GET, &format!("/api/albums/{into}"), None).await.unwrap();
            let entry = kept.merges.iter().find(|m| m.id.to_string() == merge).expect("listed on the kept album");
            assert_eq!(entry.from_album_id.to_string(), from);
            // The preview changes nothing, and reads.
            let path = format!("/api/album-merges/{merge}/undo");
            let preview: UndoReport = api.send_json(reqwest::Method::GET, &path, None).await.unwrap();
            assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
            let preview_again: UndoReport = api.send_json(reqwest::Method::GET, &path, None).await.unwrap();
            assert_eq!(preview.goes_back, preview_again.goes_back, "a preview changes nothing");
            // Undone: the album is back as itself.
            let done: UndoReport = api.send_json(reqwest::Method::POST, &path, Some(serde_json::json!({ "reason": "" }))).await.unwrap();
            assert_eq!(done.from_album_id.to_string(), from);
            let back: serde_json::Value = api.send_json(reqwest::Method::GET, &format!("/api/albums/{from}"), None).await.unwrap();
            assert_eq!(back["id"].as_str().unwrap(), from, "answers as itself again");
        });
    }
}
