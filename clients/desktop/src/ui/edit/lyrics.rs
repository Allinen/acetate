//! A track's lyrics, in edit mode: its entries (a track may have several: languages,
//! official or transcribed, synced or plain), one open at a time in a form: the text,
//! "official" and the language. Saving writes a revision (`PUT /api/lyrics/{id}`, with the
//! revision the form was filled from: someone else's save in between is refused, not
//! undone); "Add lyrics…" makes a new entry (`POST /api/tracks/{id}/lyrics`). Entries are
//! never deleted, only edited. Synced lyrics are LRC text ("[01:23.45] a line"): the
//! server tells synced from plain by the text itself.
//!
//! Each entry's history (with `view_history`): its revisions, newest first, each one's
//! text a click away, and Restore (with `restore_revision`; asks first) writes a new
//! revision with that one's text.
//!
//! Ctrl+Enter saves while the cursor is in one of its fields, or while timing (the
//! track's form has it otherwise); Discard puts back what's saved. "Sync…" times the
//! lines while listening, in a popup (see `lyrics_sync`); the text follows what it does.

use super::lyrics_sync::Sync;

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::ApiError;
use crate::catalog::Catalog;
use crate::details::{Lyrics, LyricsRevision, TrackDetail};
use crate::playback::PlayerView;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{widgets, Shell};

const BODY_ID: &str = "lyrics-body";
const LANGUAGE_ID: &str = "lyrics-language";
const REASON_ID: &str = "lyrics-reason";

/// Which entry the form is for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Open {
    Entry(Uuid),
    New,
}

/// What the form edits (as typed).
#[derive(Debug, Clone, PartialEq, Default)]
struct Fields {
    body: String,
    official: bool,
    language: String,
}

impl Fields {
    fn of(l: &Lyrics) -> Fields {
        Fields { body: l.body.clone(), official: l.is_official, language: l.language.clone().unwrap_or_default() }
    }
}

/// The server's answer to a save: the entry and its new revision.
struct Saved {
    entry: Uuid,
    revision: Uuid,
}

#[derive(Default)]
pub struct LyricsEditor {
    track: Option<Uuid>,
    open: Option<Open>,
    fields: Fields,
    /// As filled (what "changed" is measured against), and the revision it's from.
    was: Fields,
    base: Option<Uuid>,
    reason: String,
    /// Ctrl+Enter was pressed in one of the fields (taken before the track's form sees it).
    save_asked: bool,
    saving: bool,
    answer: Arc<Mutex<Option<Result<Saved, ApiError>>>>,
    error: Option<String>,
    /// Saved (shown for a few seconds).
    saved_at: Option<std::time::Instant>,
    /// The history: open or not, the revision whose text is shown, "Restore…" asked, and
    /// a restore on its way.
    history_open: bool,
    showing: Option<Uuid>,
    asked_restore: Option<Uuid>,
    restoring: bool,
    restore_answer: Arc<Mutex<Option<Result<(), String>>>>,
    /// Timing the lines (in place of the text field), if on.
    sync: Option<Sync>,
}

/// An entry's name: official or not, the language, synced or not.
fn name(l: &Lyrics, n: usize) -> String {
    let mut parts = vec![if l.is_official { "Official".to_string() } else { format!("Lyrics {}", n + 1) }];
    if let Some(language) = l.language.as_deref().filter(|s| !s.is_empty()) {
        parts.push(language.to_string());
    }
    if l.is_synced {
        parts.push("synced".into());
    }
    parts.join(" · ")
}

impl LyricsEditor {
    pub fn dirty(&self) -> bool {
        self.open.is_some() && self.fields != self.was
    }

    /// Ctrl+Enter in one of the fields, or while timing (outside any field): a save of
    /// these lyrics. Called before the track's form is shown, so it gets there first.
    pub fn take_keys(&mut self, ctx: &egui::Context) {
        let in_field = [BODY_ID, LANGUAGE_ID, REASON_ID, super::lyrics_sync::TEXT_ID, super::lyrics_sync::LANGUAGE_ID].iter().any(|id| ctx.memory(|m| m.has_focus(egui::Id::new(*id))));
        let ours = in_field || (self.sync.is_some() && !ctx.egui_wants_keyboard_input());
        if ours && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            self.save_asked = true;
        }
    }

    fn fill(&mut self, open: Open, entry: Option<&Lyrics>) {
        self.open = Some(open);
        // A new entry is in English unless said otherwise.
        self.fields = entry.map(Fields::of).unwrap_or_else(|| Fields { language: "en".into(), ..Fields::default() });
        self.was = self.fields.clone();
        self.base = entry.map(|l| l.revision_id);
        self.error = None;
        self.showing = None;
        self.asked_restore = None;
        self.resync();
    }

    /// The sync editor, if it's on, starts again from the text.
    fn resync(&mut self) {
        if self.sync.is_some() {
            self.sync = Some(Sync::new(&self.fields.body));
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, d: &TrackDetail, player: &PlayerView) {
        if self.track != Some(d.id) {
            *self = LyricsEditor { track: Some(d.id), ..Default::default() };
        }
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.saving = false;
            match result {
                Ok(saved) => {
                    // What was sent is what's there now (the details catch up).
                    self.open = Some(Open::Entry(saved.entry));
                    self.base = Some(saved.revision);
                    self.was = self.fields.clone();
                    self.reason.clear();
                    self.saved_at = Some(std::time::Instant::now());
                    shell.track_details.refresh(d.id);
                }
                Err(err) => self.error = Some(err.message),
            }
        }
        if let Some(result) = self.restore_answer.lock().unwrap().take() {
            self.restoring = false;
            match result {
                Ok(()) => shell.track_details.refresh(d.id),
                Err(err) => self.error = Some(format!("Couldn't restore it: {err}")),
            }
        }

        // The entry open: the first to start with; a form without changes follows the
        // details (a save of ours come back, or someone else's).
        let entry = |id: Uuid| d.lyrics.iter().find(|l| l.id == id);
        match self.open {
            None => {
                if let Some(first) = d.lyrics.first() {
                    self.fill(Open::Entry(first.id), Some(first));
                }
            }
            Some(Open::Entry(id)) => {
                if let Some(l) = entry(id)
                    && Some(l.revision_id) != self.base
                    && !self.saving
                    && !self.dirty()
                {
                    self.fill(Open::Entry(id), Some(l));
                }
            }
            Some(Open::New) => {}
        }
        let dirty = self.dirty();
        if dirty {
            shell.form_dirty = true;
        }

        // ---- The entries ----
        ui.horizontal_wrapped(|ui| {
            for (n, l) in d.lyrics.iter().enumerate() {
                let current = self.open == Some(Open::Entry(l.id));
                ui.add_enabled_ui(current || !dirty, |ui| {
                    if widgets::chip(ui, &name(l, n), current) && !current {
                        self.fill(Open::Entry(l.id), Some(l));
                    }
                });
            }
            let adding = self.open == Some(Open::New);
            let add = ui.add_enabled(!adding && !dirty, egui::Button::new("Add lyrics…").small());
            if add.on_disabled_hover_text("Save or discard your changes first").clicked() {
                self.fill(Open::New, None);
                shell.ctx.memory_mut(|m| m.request_focus(egui::Id::new(BODY_ID)));
            }
        });
        if d.lyrics.is_empty() && self.open.is_none() {
            ui.label(RichText::new("No lyrics yet.").color(P.muted));
            return;
        }
        let Some(open) = self.open else { return };
        let current = match open {
            Open::Entry(id) => entry(id),
            Open::New => None,
        };
        if let Some(l) = current
            && dirty
            && Some(l.revision_id) != self.base
            && !self.saving
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Someone changed these lyrics since you started.").color(P.danger));
                if ui.small_button("Load theirs (drops your changes)").clicked() {
                    self.fill(open, Some(l));
                }
            });
        }

        // ---- The form ----
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fields.official, "Official").on_hover_text("From the artist or label, rather than transcribed by someone");
            ui.label(RichText::new("Language").color(P.faint));
            ui.add(egui::TextEdit::singleline(&mut self.fields.language).id(egui::Id::new(LANGUAGE_ID)).hint_text("en, ja… (optional)").desired_width(110.0));
            if let Some(l) = current
                && !l.source.is_empty()
            {
                ui.label(RichText::new(format!("· from {}", if l.source == "embedded" { "the file" } else { &l.source })).color(P.faint));
            }
            let sync = ui.button("Sync…").on_hover_text("Time the lines while listening (Enter as each one starts), and give lines their own language");
            if sync.clicked() && self.sync.is_none() {
                self.sync = Some(Sync::new(&self.fields.body));
            }
        });
        if self.sync.is_some() {
            self.sync_popup(shell, catalog, d, player);
        }
        // (While timing, the text follows what the popup does.)
        ui.add_enabled(
            self.sync.is_none(),
            egui::TextEdit::multiline(&mut self.fields.body)
                .id(egui::Id::new(BODY_ID))
                .font(egui::TextStyle::Monospace)
                .hint_text("The lyrics. Synced: a time in front of each line, “[01:23.45] a line”; a line in another language, “[lang:ja]” after it.")
                .desired_rows(14)
                .desired_width(f32::INFINITY),
        );
        ui.add(egui::TextEdit::singleline(&mut self.reason).id(egui::Id::new(REASON_ID)).hint_text("Why (optional: kept with the change)").desired_width(420.0));
        let can_save = dirty && !self.saving && !self.fields.body.trim().is_empty();
        ui.horizontal(|ui| {
            let text = if open == Open::New { "Add them  (Ctrl+Enter)" } else { "Save the lyrics  (Ctrl+Enter)" };
            let save = ui.add_enabled(can_save, egui::Button::new(text)).clicked() || std::mem::take(&mut self.save_asked);
            if save && can_save {
                self.save(shell, d.id, open);
            }
            if open == Open::New {
                if ui.button("Cancel").clicked() {
                    self.open = None;
                    self.fields = Fields::default();
                    self.was = Fields::default();
                }
            } else if ui.add_enabled(dirty && !self.saving, egui::Button::new("Discard")).clicked() {
                self.fields = self.was.clone();
                self.resync();
            }
            if self.saving {
                ui.label(RichText::new("Saving…").color(P.muted));
            } else if self.saved_at.is_some_and(|at| at.elapsed().as_secs() < 5) && !dirty {
                ui.label(RichText::new("Saved").color(P.accent));
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            } else if dirty && self.fields.body.trim().is_empty() {
                ui.label(RichText::new("Empty lyrics can't be saved (entries aren't removed: a wrong one is corrected).").color(P.faint).small());
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }

        // ---- Its history ----
        if let Some(l) = current
            && let Some(revisions) = l.revisions.as_ref().filter(|r| r.len() > 1)
        {
            ui.add_space(6.0);
            let text = format!("{} History of these lyrics ({})", if self.history_open { "▾" } else { "▸" }, revisions.len());
            if ui.add(egui::Button::new(RichText::new(text).color(P.muted)).frame(false)).clicked() {
                self.history_open = !self.history_open;
            }
            if self.history_open {
                self.history(ui, shell, revisions);
            }
        }
    }

    /// Timing the lines, in a popup over the page (Esc closes it; the changes stay in the
    /// form, to save there or here with Ctrl+Enter).
    fn sync_popup(&mut self, shell: &mut Shell, catalog: &Catalog, d: &TrackDetail, player: &PlayerView) {
        let Some(sync) = self.sync.as_mut() else { return };
        shell.modal_open = true;
        let language = Some(self.fields.language.trim()).filter(|l| !l.is_empty()).unwrap_or("?").to_string();
        let status = if self.saving {
            ("Saving…".to_string(), P.muted)
        } else if let Some(err) = &self.error {
            (err.clone(), P.danger)
        } else if self.fields != self.was {
            ("Not saved yet (Ctrl+Enter saves)".to_string(), P.muted)
        } else if self.saved_at.is_some_and(|at| at.elapsed().as_secs() < 5) {
            ("Saved".to_string(), P.accent)
        } else {
            (String::new(), P.muted)
        };
        let mut changed = false;
        let ctx = shell.ctx.clone();
        let title = format!("Timing “{}”", d.title);
        let closed = crate::ui::modal::show(&ctx, "lyrics-sync", &title, egui::Vec2::new(1100.0, 900.0), |ui| {
            if !status.0.is_empty() {
                ui.label(RichText::new(&status.0).color(status.1));
            }
            changed = sync.show(ui, shell, catalog, d, player, &language);
        });
        if changed {
            self.fields.body = sync.text();
        }
        if closed {
            self.sync = None;
        }
    }

    /// The revisions, newest first: who, when, why, the text a click away, Restore.
    fn history(&mut self, ui: &mut egui::Ui, shell: &Shell, revisions: &[LyricsRevision]) {
        let can_restore = shell.account.can("restore_revision");
        for (n, r) in revisions.iter().enumerate() {
            let who = r.author.clone().unwrap_or_else(|| "the system".into());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(r.created_at.get(..16).unwrap_or(&r.created_at).replace('T', " ")).font(font(12.5, semibold())));
                ui.label(RichText::new(format!("{} by {who}", r.source)).color(P.muted));
                let showing = self.showing == Some(r.id);
                if ui.small_button(if showing { "Hide" } else { "Show" }).clicked() {
                    self.showing = if showing { None } else { Some(r.id) };
                }
                if n == 0 {
                    ui.label(RichText::new("· now").color(P.accent));
                } else if can_restore && !self.restoring && self.asked_restore != Some(r.id) && ui.small_button("Restore…").clicked() {
                    self.asked_restore = Some(r.id);
                }
            });
            if !r.reason.is_empty() {
                ui.label(RichText::new(format!("“{}”", r.reason)).color(P.muted).italics());
            }
            if self.showing == Some(r.id) {
                let body = r.data.get("body").and_then(Value::as_str).unwrap_or("");
                egui::Frame::NONE.fill(P.surface).corner_radius(6).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(body).monospace().color(P.text)).wrap());
                });
            }
            if self.asked_restore == Some(r.id) {
                ui.label(RichText::new("Restoring writes a new revision with this one's text; the ones after it stay in the history.").color(P.text));
                ui.horizontal(|ui| {
                    if ui.button("Restore it").clicked() {
                        self.asked_restore = None;
                        self.restore(shell, r.id);
                    }
                    if ui.button("Cancel").clicked() {
                        self.asked_restore = None;
                    }
                });
            }
            ui.add_space(4.0);
        }
        if self.restoring {
            ui.label(RichText::new("Restoring…").color(P.muted));
        }
    }

    fn save(&mut self, shell: &Shell, track: Uuid, open: Open) {
        #[derive(serde::Deserialize)]
        struct Answer {
            /// Only when made.
            id: Option<Uuid>,
            revision_id: Uuid,
        }
        let language = Some(self.fields.language.trim()).filter(|l| !l.is_empty());
        let mut body = json!({
            "body": self.fields.body,
            "is_official": self.fields.official,
            "language": language,
            "reason": self.reason.trim(),
        });
        let (method, path) = match open {
            Open::New => (reqwest::Method::POST, format!("/api/tracks/{track}/lyrics")),
            Open::Entry(id) => {
                body["base_revision_id"] = json!(self.base);
                (reqwest::Method::PUT, format!("/api/lyrics/{id}"))
            }
        };
        self.saving = true;
        self.error = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Answer>(method, &path, Some(body)).await.and_then(|a| {
                let entry = match open {
                    Open::Entry(id) => id,
                    Open::New => a.id.ok_or_else(|| ApiError { status: None, code: "invalid".into(), message: "The server didn't say which entry it made.".into() })?,
                };
                Ok(Saved { entry, revision: a.revision_id })
            });
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    fn restore(&mut self, shell: &Shell, revision: Uuid) {
        self.restoring = true;
        self.error = None;
        let (api, answer, ctx) = (shell.api.clone(), self.restore_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let path = format!("/api/lyrics-revisions/{revision}/restore");
            let result = api.send_json::<Value>(reqwest::Method::POST, &path, Some(json!({ "reason": "" }))).await.map(|_| ()).map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Api;

    fn entry(official: bool, language: Option<&str>, synced: bool) -> Lyrics {
        serde_json::from_value(json!({
            "id": Uuid::from_u128(1), "is_official": official, "is_synced": synced, "language": language,
            "body": "", "revision_id": Uuid::from_u128(2),
        }))
        .unwrap()
    }

    #[test]
    fn entries_are_named_by_what_they_are() {
        assert_eq!(name(&entry(true, Some("en"), true), 0), "Official · en · synced");
        assert_eq!(name(&entry(false, None, false), 1), "Lyrics 2");
        assert_eq!(name(&entry(false, Some(""), false), 0), "Lyrics 1");
    }

    /// Against a running server (a throwaway database: it adds a lyrics entry to the
    /// track, which stays, as entries do):
    /// `ACETATE_TEST_SERVER=http://127.0.0.1:3002 ACETATE_TEST_TOKEN=… ACETATE_TEST_TRACK=<id>
    /// cargo test lyrics -- --ignored`
    #[test]
    #[ignore]
    fn lyrics_added_edited_refused_when_stale_and_restored() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACK"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(track)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-lyrics-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (api, track) = (&api, track.as_str());
            let detail = || async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap() };
            #[derive(serde::Deserialize)]
            struct Answer {
                id: Option<Uuid>,
                revision_id: Uuid,
            }

            // Added, as the form sends it.
            let text = format!("[00:01.00] test line {}\n[00:03.00] and another", std::process::id());
            let body = json!({ "body": text, "is_official": false, "language": "en", "reason": "lyrics test" });
            let made: Answer = api.send_json(reqwest::Method::POST, &format!("/api/tracks/{track}/lyrics"), Some(body)).await.unwrap();
            let id = made.id.expect("a new entry's id");
            let d = detail().await;
            let l = d.lyrics.iter().find(|l| l.id == id).unwrap();
            assert!(l.is_synced && l.language.as_deref() == Some("en") && l.revision_id == made.revision_id);

            // Edited from its revision; again from the same (now old) one: refused.
            let edit = |base: Uuid, body: &str| json!({ "base_revision_id": base, "body": body, "is_official": true, "language": null, "reason": "" });
            let edited: Answer = api.send_json(reqwest::Method::PUT, &format!("/api/lyrics/{id}"), Some(edit(made.revision_id, "plain now"))).await.unwrap();
            let stale = api.send_json::<Answer>(reqwest::Method::PUT, &format!("/api/lyrics/{id}"), Some(edit(made.revision_id, "late"))).await.err().expect("a stale save is refused");
            assert_eq!(stale.status, Some(409));
            let d = detail().await;
            let l = d.lyrics.iter().find(|l| l.id == id).unwrap();
            assert_eq!((l.body.as_str(), l.is_official, l.is_synced, l.revision_id), ("plain now", true, false, edited.revision_id));

            // The first one restored: its text again, as a new revision.
            let first = l.revisions.as_ref().expect("history, for this user").last().unwrap().id;
            api.send_json::<Value>(reqwest::Method::POST, &format!("/api/lyrics-revisions/{first}/restore"), Some(json!({ "reason": "" }))).await.unwrap();
            let d = detail().await;
            let l = d.lyrics.iter().find(|l| l.id == id).unwrap();
            assert_eq!(l.body, text);
            assert_eq!(l.revisions.as_ref().unwrap().len(), 3);
        });
    }
}
