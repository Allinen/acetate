//! Settings, Vocabulary (admins): the words the library knows (statuses, credit roles,
//! album kinds, relation kinds, kinds of review flag and of extra). The ones the app is
//! built around are locked: they can be described, not renamed or removed. Any other can be
//! renamed (everything that uses it follows), described, ordered and, when nothing uses it,
//! removed. New ones are added at the bottom of each list.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use serde::Deserialize;

use super::settings::{hint, section};
use crate::ui::task::Task;
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Shell};

const LISTS: [(&str, &str, &str); 7] = [
    ("status", "Statuses", "where a track stands: released, demo…"),
    ("credit_role", "Credit roles", "how an artist is on a track or album: primary, featured, producer…"),
    ("album_kind", "Album kinds", "album, single, EP…"),
    ("relation_kind", "Relations", "how one track relates to another: remix, instrumental…; each reads both ways"),
    ("review_kind", "Review flags", "the kinds of flag in Review (the automatic ones are the server's own checks)"),
    ("extra_kind", "Extra kinds", "vocals, stems, a video…"),
    ("version_word", "Version words", "words in a title that make it another version of a song, so “Song” and “Song (Instrumental)” aren't taken for duplicates; the mark is what versions have in common (remastered, remaster)"),
];

#[derive(Debug, Clone, Deserialize)]
struct Word {
    list: String,
    word: String,
    label: String,
    description: String,
    locked: bool,
    inverse: Option<String>,
    data: serde_json::Value,
    uses: i64,
}

#[derive(Default)]
struct Draft {
    rename: String,
    label: String,
    description: String,
    template: String,
    mark: String,
}

#[derive(Default)]
pub struct VocabularyTab {
    words: Task<Result<Vec<Word>, String>>,
    loaded: Option<Result<Vec<Word>, String>>,
    list: usize,
    /// What's being changed in each row (by list and word).
    drafts: HashMap<(String, String), Draft>,
    adding: Draft,
    adding_inverse: String,
    adding_inverse_template: String,
    adding_substring: bool,
    action: Task<Result<String, String>>,
    said: Option<Result<String, String>>,
}

impl VocabularyTab {
    pub fn opened(&mut self, shell: &Shell) {
        self.words.start_with(shell, |api| async move { api.send_json::<Vec<Word>>(reqwest::Method::GET, "/api/vocabulary", None).await.map_err(|e| e.message) });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(words) = self.words.poll() {
            self.loaded = Some(words);
        }
        if let Some(done) = self.action.poll() {
            if done.is_ok() {
                self.drafts.clear();
                self.adding = Draft::default();
                self.adding_inverse.clear();
                self.adding_inverse_template.clear();
                self.adding_substring = false;
                self.opened(shell);
                // The forms everywhere use the new words.
                shell.account.refresh_vocab(&shell.api, shell.runtime.handle(), &shell.waker);
            }
            self.said = Some(done);
        }
        ui.horizontal_wrapped(|ui| {
            for (n, (_, name, _)) in LISTS.iter().enumerate() {
                if widgets::chip(ui, name, self.list == n) {
                    self.list = n;
                }
            }
        });
        let (list, _, about) = LISTS[self.list];
        hint(ui, about);
        match &self.said {
            Some(Ok(text)) => {
                ui.label(RichText::new(text.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }
        let words = match &self.loaded {
            None => {
                ui.label(RichText::new("Loading…").color(P.faint));
                return;
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
                return;
            }
            Some(Ok(words)) => words.iter().filter(|w| w.list == list).cloned().collect::<Vec<_>>(),
        };
        section(ui, "Words");
        let busy = self.action.is_running();
        for w in &words {
            self.row(ui, shell, w, busy);
            ui.separator();
        }
        section(ui, "Add one");
        self.add_form(ui, shell, list, busy);
    }

    fn row(&mut self, ui: &mut egui::Ui, shell: &Shell, w: &Word, busy: bool) {
        // (Relations are pairs: the second of a pair is changed from the first.)
        let draft = self.drafts.entry((w.list.clone(), w.word.clone())).or_insert_with(|| Draft {
            rename: w.word.clone(),
            label: w.label.clone(),
            description: w.description.clone(),
            template: w.data.get("template").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            mark: w.data.get("mark").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        });
        let path = format!("/api/vocabulary/{}/{}", w.list, w.word);
        ui.horizontal_wrapped(|ui| {
            if w.locked {
                ui.label(RichText::new(format!("{}  (locked)", w.word)).strong());
            } else {
                ui.add(egui::TextEdit::singleline(&mut draft.rename).desired_width(180.0));
            }
            if w.list == "review_kind" {
                ui.add(egui::TextEdit::singleline(&mut draft.label).hint_text("name").desired_width(200.0));
            }
            if let Some(inverse) = &w.inverse {
                ui.label(RichText::new(if *inverse == w.word { "reads the same both ways".to_string() } else { format!("other way: {inverse}") }).color(P.faint));
            }
            ui.label(RichText::new(match w.uses {
                0 => "unused".to_string(),
                1 => "1 use".to_string(),
                n => format!("{n} uses"),
            })
            .color(P.faint));
        });
        if w.list == "version_word" {
            ui.horizontal(|ui| {
                ui.label(RichText::new("mark").color(P.faint));
                ui.add(egui::TextEdit::singleline(&mut draft.mark).desired_width(180.0));
                if w.data.get("substring").and_then(|s| s.as_bool()) == Some(true) {
                    ui.label(RichText::new("found anywhere in a title").color(P.faint));
                }
            });
        }
        if w.list == "relation_kind" {
            ui.add(egui::TextEdit::singleline(&mut draft.template).hint_text("%A is a remix of %B").desired_width(420.0));
        }
        ui.add(egui::TextEdit::singleline(&mut draft.description).hint_text("a note about it").desired_width(420.0));
        let template_was = w.data.get("template").and_then(|t| t.as_str()).unwrap_or("");
        let mark_was = w.data.get("mark").and_then(|t| t.as_str()).unwrap_or("");
        let changed = (w.list == "version_word" && draft.mark != mark_was) || draft.rename != w.word || draft.label != w.label || draft.description != w.description || (w.list == "relation_kind" && draft.template != template_was);
        ui.horizontal(|ui| {
            if ui.add_enabled(changed && !busy, egui::Button::new("Save").small()).clicked() {
                let mut body = serde_json::json!({ "label": draft.label, "description": draft.description });
                if draft.rename != w.word {
                    body["rename_to"] = serde_json::json!(draft.rename.trim());
                }
                if w.list == "relation_kind" && draft.template != template_was {
                    body["template"] = serde_json::json!(draft.template);
                }
                if w.list == "version_word" && draft.mark != mark_was {
                    body["mark"] = serde_json::json!(draft.mark.trim());
                }
                let path = path.clone();
                self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::PUT, &path, Some(body)).await.map(|_| "Saved.".to_string()).map_err(|e| e.message) });
            }
            let can_remove = !w.locked && w.uses == 0;
            let tip = if w.locked { "the app is built around it" } else if w.uses > 0 { "something uses it" } else { "" };
            if ui.add_enabled(can_remove && !busy, egui::Button::new("Remove").small()).on_disabled_hover_text(tip).clicked() {
                let path = path.clone();
                self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::DELETE, &path, None).await.map(|_| "Removed.".to_string()).map_err(|e| e.message) });
            }
        });
    }

    fn add_form(&mut self, ui: &mut egui::Ui, shell: &Shell, list: &str, busy: bool) {
        let relation = list == "relation_kind";
        let review = list == "review_kind";
        ui.horizontal_wrapped(|ui| {
            if review {
                ui.add(egui::TextEdit::singleline(&mut self.adding.label).hint_text("name, e.g. Missing lyrics").desired_width(260.0));
            } else {
                ui.add(egui::TextEdit::singleline(&mut self.adding.rename).hint_text("the word").desired_width(180.0));
            }
        });
        if relation {
            ui.add(egui::TextEdit::singleline(&mut self.adding.template).hint_text("its sentence: %A is a cover of %B").desired_width(420.0));
            ui.add(egui::TextEdit::singleline(&mut self.adding_inverse).hint_text("the word for the other way (the same word: it reads the same both ways)").desired_width(420.0));
            if !self.adding_inverse.trim().is_empty() && self.adding_inverse.trim() != self.adding.rename.trim() {
                ui.add(egui::TextEdit::singleline(&mut self.adding_inverse_template).hint_text("its sentence the other way: %B is a cover of %A").desired_width(420.0));
            }
        }
        if list == "version_word" {
            ui.add(egui::TextEdit::singleline(&mut self.adding.mark).hint_text("its mark: what versions with it have in common").desired_width(420.0));
            ui.checkbox(&mut self.adding_substring, "found anywhere in a title (for languages written without spaces)");
        }
        if !review {
            ui.add(egui::TextEdit::singleline(&mut self.adding.description).hint_text("a note about it").desired_width(420.0));
        }
        let ready = if review { !self.adding.label.trim().is_empty() } else { !self.adding.rename.trim().is_empty() };
        if ui.add_enabled(ready && !busy, egui::Button::new("Add")).clicked() {
            let mut body = serde_json::json!({ "list": list, "word": self.adding.rename.trim(), "label": self.adding.label.trim(), "description": self.adding.description.trim() });
            if list == "version_word" {
                body["mark"] = serde_json::json!(self.adding.mark.trim());
                body["substring"] = serde_json::json!(self.adding_substring);
            }
            if relation {
                body["template"] = serde_json::json!(self.adding.template.trim());
                let inverse = if self.adding_inverse.trim().is_empty() { self.adding.rename.trim() } else { self.adding_inverse.trim() };
                body["inverse"] = serde_json::json!(inverse);
                body["inverse_template"] = serde_json::json!(self.adding_inverse_template.trim());
            }
            self.action.send_with(shell, |api| async move { api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/vocabulary", Some(body)).await.map(|_| "Added.".to_string()).map_err(|e| e.message) });
        }
    }
}
