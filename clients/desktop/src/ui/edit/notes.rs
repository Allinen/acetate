//! Notes on a track, album or artist: shown on its page (links in them open in the
//! browser), and in edit mode written, edited, taken off (kept, and brought back from
//! "Taken off"), and put back from their history. Text files make notes too, one each
//! (named after the file): chosen in the file dialog, pasted after copying them in a file
//! manager (Ctrl+V), or dropped on the window where that works; listed first, then made.
//! See the server's `db::notes`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::details::Note;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::Shell;

/// What notes are written on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Owner {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Owner {
    /// The field naming it in `POST /api/notes`.
    fn field(self) -> (&'static str, Uuid) {
        match self {
            Owner::Track(id) => ("track_id", id),
            Owner::Album(id) => ("album_id", id),
            Owner::Artist(id) => ("artist_id", id),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Owner::Track(_) => "track",
            Owner::Album(_) => "album",
            Owner::Artist(_) => "artist",
        }
    }

    /// Its details fetched again (its notes are in them).
    fn refresh(self, shell: &mut Shell) {
        match self {
            Owner::Track(id) => shell.track_details.refresh(id),
            Owner::Album(id) => shell.album_details.refresh(id),
            Owner::Artist(id) => shell.artist_details.refresh(id),
        }
    }
}

/// Text files notes are made from.
const TEXT: &[&str] = &["txt", "md", "text", "nfo"];

/// Lines of a note shown before "Show all".
const SHORT: usize = 16;

/// A note being written or edited.
struct Editing {
    /// None: a new one.
    note: Option<Uuid>,
    base: Option<Uuid>,
    title: String,
    body: String,
    reason: String,
}

/// A text file, read, to become a note.
struct TextFile {
    title: String,
    body: String,
    /// Not UTF-8: read with the odd character replaced.
    lossy: bool,
    /// Couldn't be read (left out).
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Revision {
    title: String,
    body: String,
    hidden: bool,
    created_at: String,
    created_by: Option<String>,
    #[serde(default)]
    reason: String,
}

/// A note's history: which note, and what arrived.
type History = (Uuid, Arc<Mutex<Option<Result<Vec<Revision>, String>>>>);

#[derive(Default)]
pub struct NotesView {
    owner: Option<Owner>,
    /// Long notes shown whole.
    whole: HashSet<Uuid>,
    editing: Option<Editing>,
    sending: bool,
    answer: Arc<Mutex<Option<Result<String, String>>>>,
    result: Option<Result<String, String>>,
    history: Option<History>,
    /// Text files chosen in the dialog, and those read, to be made notes.
    chosen: Arc<Mutex<Vec<PathBuf>>>,
    files: Vec<TextFile>,
    made: Arc<AtomicUsize>,
    making: usize,
}

impl NotesView {
    /// Whether there's anything to show for these notes (outside edit mode: shown ones).
    pub fn worth_showing(shell: &Shell, notes: &[Note]) -> bool {
        (shell.edit_mode && shell.account.can("edit_metadata")) || notes.iter().any(|n| !n.hidden)
    }

    /// The notes on `owner`. `compact`: each folded under its title (album and artist
    /// pages); else shown whole. `take_files`: pasted or dropped text files make notes here
    /// (where only one notes list is shown).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner, notes: &[Note], compact: bool, take_files: bool) {
        if self.owner != Some(owner) {
            *self = NotesView { owner: Some(owner), ..Default::default() };
        }
        let answer = self.answer.lock().unwrap().take();
        if let Some(result) = answer {
            self.sending = false;
            if result.is_ok() {
                self.editing = None;
                self.files.clear();
            }
            owner.refresh(shell);
            self.result = Some(result);
        }
        let can = shell.edit_mode && shell.account.can("edit_metadata");

        // Text files given: read, and listed to be made notes.
        if can {
            let mut paths = std::mem::take(&mut *self.chosen.lock().unwrap());
            if take_files && !shell.palette_open && !shell.modal_open {
                paths.extend(crate::ui::files::given(ui.ctx()));
            }
            for path in paths.into_iter().filter(|p| TEXT.contains(&crate::ui::files::extension(p).as_str())) {
                self.files.push(read_text(&path));
            }
        }

        let shown: Vec<&Note> = notes.iter().filter(|n| !n.hidden).collect();
        if shown.is_empty() && !can {
            return;
        }
        let mut edit: Option<Editing> = None;
        let mut hide: Option<(Uuid, bool)> = None;
        let mut history: Option<Uuid> = None;
        for note in &shown {
            if self.editing.as_ref().is_some_and(|e| e.note == Some(note.id)) {
                self.form(ui, shell, owner);
                continue;
            }
            let title = if note.title.is_empty() { first_line(&note.body) } else { note.title.clone() };
            let mut meta = vec![note.updated_at.get(..10).unwrap_or(&note.updated_at).to_string()];
            if let Some(by) = &note.updated_by {
                meta.push(by.clone());
            }
            if note.merged_from.is_some() {
                meta.push(format!("from a {} merged into this one", owner.name()));
            }
            let mut actions = |ui: &mut egui::Ui| {
                if can {
                    ui.horizontal(|ui| {
                        if ui.small_button("Edit").clicked() {
                            edit = Some(Editing { note: Some(note.id), base: Some(note.revision_id), title: note.title.clone(), body: note.body.clone(), reason: String::new() });
                        }
                        if ui.small_button("Take off").on_hover_text("Hidden, not deleted: brought back from “Taken off”").clicked() {
                            hide = Some((note.id, true));
                        }
                        if shell.account.can("view_history") && ui.small_button("History").clicked() {
                            history = Some(note.id);
                        }
                    });
                }
            };
            if compact {
                egui::CollapsingHeader::new(RichText::new(&title).font(font(14.5, semibold()))).id_salt(("note", note.id)).show(ui, |ui| {
                    ui.label(RichText::new(meta.join("  ·  ")).color(P.faint).small());
                    self.body(ui, note);
                    actions(ui);
                });
            } else {
                ui.label(RichText::new(&title).font(font(15.0, semibold())));
                ui.label(RichText::new(meta.join("  ·  ")).color(P.faint).small());
                self.body(ui, note);
                actions(ui);
                ui.add_space(10.0);
            }
            if self.history.as_ref().is_some_and(|h| h.0 == note.id) {
                self.history_list(ui, &mut edit, note);
            }
        }
        if let Some(e) = edit {
            self.editing = Some(e);
        }
        if let Some(id) = history {
            self.ask_history(shell, id);
        }
        if !can {
            return;
        }

        // ---- Edit mode: a new note, text files, what was taken off ----
        if self.editing.as_ref().is_some_and(|e| e.note.is_none()) {
            self.form(ui, shell, owner);
        }
        ui.horizontal_wrapped(|ui| {
            if self.editing.is_none() && ui.button("New note").clicked() {
                self.editing = Some(Editing { note: None, base: None, title: String::new(), body: String::new(), reason: String::new() });
            }
            if ui.button("Notes from text files…").on_hover_text("One note each, named after the file").clicked() {
                crate::ui::files::choose(shell, "Text files to make notes of", ("Text", TEXT), true, &self.chosen);
            }
            if take_files {
                ui.label(RichText::new("or paste copied text files (Ctrl+V)").color(P.faint).small());
            }
        });
        self.files_to_make(ui, shell, owner);
        let hidden: Vec<&Note> = notes.iter().filter(|n| n.hidden).collect();
        if !hidden.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("Taken off ({})", hidden.len())).color(P.muted)).id_salt(("notes-hidden", owner.field().1)).show(ui, |ui| {
                for note in hidden {
                    ui.horizontal(|ui| {
                        let title = if note.title.is_empty() { first_line(&note.body) } else { note.title.clone() };
                        ui.label(RichText::new(title).color(P.muted));
                        if ui.small_button("Bring back").clicked() {
                            hide = Some((note.id, false));
                        }
                    });
                }
            });
        }
        if let Some((note, hidden)) = hide {
            let text = if hidden { "Taken off (kept: “Taken off” brings it back)." } else { "Brought back." };
            self.send(shell, reqwest::Method::POST, format!("/api/notes/{note}/hidden"), serde_json::json!({ "hidden": hidden }), text.into());
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

    /// A note's text: long ones shortened until "Show all".
    fn body(&mut self, ui: &mut egui::Ui, note: &Note) {
        let lines: Vec<&str> = note.body.lines().collect();
        let whole = self.whole.contains(&note.id) || lines.len() <= SHORT + 4;
        let shown = if whole { &lines[..] } else { &lines[..SHORT] };
        for line in shown {
            text_line(ui, line);
        }
        if !whole && ui.link(format!("Show all ({} lines)", lines.len())).clicked() {
            self.whole.insert(note.id);
        }
    }

    /// The note being written or edited.
    fn form(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner) {
        let Some(e) = self.editing.as_mut() else { return };
        let mut save = false;
        let mut cancel = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut e.title).hint_text("Title (optional)").font(font(15.0, semibold())).desired_width(f32::INFINITY));
            ui.add(egui::TextEdit::multiline(&mut e.body).hint_text("The note").desired_rows(10).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut e.reason).hint_text("Why (optional: kept with it)").desired_width(320.0));
                let ready = !self.sending && !(e.title.trim().is_empty() && e.body.trim().is_empty());
                save = ui.add_enabled(ready, egui::Button::new(if e.note.is_some() { "Save" } else { "Add the note" })).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        ui.add_space(8.0);
        if cancel {
            self.editing = None;
            return;
        }
        if save && let Some(e) = self.editing.as_ref() {
            let (method, path, body, text) = match e.note {
                Some(note) => (
                    reqwest::Method::PUT,
                    format!("/api/notes/{note}"),
                    serde_json::json!({ "base_revision_id": e.base, "title": e.title, "body": e.body, "reason": e.reason }),
                    "Saved.",
                ),
                None => {
                    let (field, id) = owner.field();
                    let mut body = serde_json::json!({ "title": e.title, "body": e.body, "reason": e.reason });
                    body[field] = serde_json::json!(id);
                    (reqwest::Method::POST, "/api/notes".to_string(), body, "Added.")
                }
            };
            self.send(shell, method, path, body, text.into());
        }
    }

    /// Text files read, to be made notes: listed, then made one by one.
    fn files_to_make(&mut self, ui: &mut egui::Ui, shell: &mut Shell, owner: Owner) {
        if self.files.is_empty() {
            return;
        }
        let good = self.files.iter().filter(|f| f.error.is_none()).count();
        let mut make = false;
        let mut cancel = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.label(RichText::new(format!("{good} notes from text files, named after them")).font(font(14.0, semibold())));
            for f in &mut self.files {
                ui.horizontal(|ui| match &f.error {
                    Some(err) => {
                        ui.label(RichText::new(format!("{}: {err} (left out)", f.title)).color(P.danger));
                    }
                    None => {
                        ui.add(egui::TextEdit::singleline(&mut f.title).desired_width(260.0));
                        let size = f.body.lines().count();
                        ui.label(RichText::new(format!("{size} lines · {}", first_line(&f.body))).color(P.muted));
                        if f.lossy {
                            ui.label(RichText::new("not UTF-8: odd characters replaced").color(P.danger).small());
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                make = ui.add_enabled(!self.sending && good > 0, egui::Button::new(format!("Make {good} notes"))).clicked();
                cancel = ui.add_enabled(!self.sending, egui::Button::new("Cancel")).clicked();
                if self.sending && self.making > 0 {
                    ui.label(RichText::new(format!("{} of {}…", self.made.load(Ordering::Relaxed), self.making)).color(P.muted));
                }
            });
        });
        ui.add_space(8.0);
        if cancel {
            self.files.clear();
        }
        if make {
            let (field, id) = owner.field();
            let notes: Vec<serde_json::Value> = self
                .files
                .iter()
                .filter(|f| f.error.is_none())
                .map(|f| {
                    let mut body = serde_json::json!({ "title": f.title.trim(), "body": f.body, "reason": "from a text file" });
                    body[field] = serde_json::json!(id);
                    body
                })
                .collect();
            self.sending = true;
            self.result = None;
            self.making = notes.len();
            self.made.store(0, Ordering::Relaxed);
            let (api, ctx, answer, made) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone(), self.made.clone());
            shell.runtime.spawn(async move {
                let mut failed = Vec::new();
                for note in notes {
                    let title = note["title"].as_str().unwrap_or("").to_string();
                    if let Err(err) = api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/notes", Some(note)).await {
                        failed.push(format!("{title}: {}", err.message));
                    }
                    made.fetch_add(1, Ordering::Relaxed);
                    ctx.request_repaint();
                }
                let done = made.load(Ordering::Relaxed) - failed.len();
                *answer.lock().unwrap() = Some(if failed.is_empty() { Ok(format!("{done} notes made.")) } else { Err(format!("{done} notes made; not these: {}", failed.join("; "))) });
                ctx.request_repaint();
            });
        }
    }

    /// A note's history, newest first: each can be put back (into the form, to look at and
    /// save).
    fn history_list(&mut self, ui: &mut egui::Ui, edit: &mut Option<Editing>, note: &Note) {
        let Some((_, arrived)) = &self.history else { return };
        let arrived = arrived.lock().unwrap().clone();
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("History").font(font(13.5, semibold())));
                if ui.small_button("Close").clicked() {
                    self.history = None;
                }
            });
            match arrived {
                None => {
                    ui.label(RichText::new("Loading…").color(P.muted));
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err).color(P.danger));
                }
                Some(Ok(revisions)) => {
                    for (n, r) in revisions.iter().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            let by = r.created_by.as_deref().map(|b| format!(" · {b}")).unwrap_or_default();
                            let why = if r.reason.is_empty() { String::new() } else { format!(" · “{}”", r.reason) };
                            let what = if r.hidden { "taken off" } else if n + 1 == revisions.len() { "written" } else { "edited" };
                            ui.label(RichText::new(format!("{}{by} · {what}{why}", r.created_at.get(..16).unwrap_or(&r.created_at).replace('T', " "))).color(P.muted));
                            ui.label(RichText::new(first_line(&r.body)).color(P.faint));
                            if n > 0 && ui.small_button("Put back").on_hover_text("Into the form, to look at and save").clicked() {
                                *edit = Some(Editing { note: Some(note.id), base: Some(note.revision_id), title: r.title.clone(), body: r.body.clone(), reason: "put back an earlier version".into() });
                            }
                        });
                    }
                }
            }
        });
    }

    fn ask_history(&mut self, shell: &Shell, note: Uuid) {
        if self.history.as_ref().is_some_and(|h| h.0 == note) {
            self.history = None;
            return;
        }
        let arrived: Arc<Mutex<Option<Result<Vec<Revision>, String>>>> = Arc::default();
        let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), arrived.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Vec<Revision>>(reqwest::Method::GET, &format!("/api/notes/{note}/revisions"), None).await.map_err(|e| e.message);
            *into.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
        self.history = Some((note, arrived));
    }

    fn send(&mut self, shell: &Shell, method: reqwest::Method, path: String, body: serde_json::Value, text: String) {
        self.sending = true;
        self.result = None;
        let (api, ctx, answer) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<serde_json::Value>(method, &path, Some(body)).await.map(|_| text).map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// A line of a note, its links clickable (opened in the browser).
fn text_line(ui: &mut egui::Ui, line: &str) {
    if line.trim().is_empty() {
        ui.add_space(6.0);
        return;
    }
    if !line.contains("http://") && !line.contains("https://") {
        ui.add(egui::Label::new(RichText::new(line).font(font(14.0, medium()))).wrap().selectable(true));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (n, word) in line.split(' ').enumerate() {
            let space = if n == 0 { "" } else { " " };
            if word.starts_with("http://") || word.starts_with("https://") {
                ui.label(space);
                ui.hyperlink_to(RichText::new(word).font(font(14.0, medium())), word.trim_end_matches(['.', ',', ')', ']']));
            } else {
                ui.add(egui::Label::new(RichText::new(format!("{space}{word}")).font(font(14.0, medium()))).selectable(true));
            }
        }
    });
}

/// A note's first line with text (for titles and lists).
fn first_line(body: &str) -> String {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut short: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        short.push('…');
    }
    short
}

/// A text file, read to become a note (titled after the file).
fn read_text(path: &std::path::Path) -> TextFile {
    let title = path.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
    match std::fs::read(path) {
        Ok(bytes) => {
            // A byte order mark isn't text.
            let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            match std::str::from_utf8(bytes) {
                Ok(text) => TextFile { title, body: text.replace("\r\n", "\n"), lossy: false, error: None },
                Err(_) => TextFile { title, body: String::from_utf8_lossy(bytes).replace("\r\n", "\n"), lossy: true, error: None },
            }
        }
        Err(err) => TextFile { title, body: String::new(), lossy: false, error: Some(err.to_string()) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_files_become_notes_named_after_them() {
        let dir = std::env::temp_dir().join(format!("acetate-notes-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("studio_session.txt");
        std::fs::write(&file, b"\xEF\xBB\xBFTracked at home.\r\nMixed later.\r\n").unwrap();
        let read = read_text(&file);
        assert_eq!(read.title, "studio session");
        assert_eq!(read.body, "Tracked at home.\nMixed later.\n");
        assert!(!read.lossy && read.error.is_none());
        let latin = dir.join("old.txt");
        std::fs::write(&latin, b"caf\xE9").unwrap();
        assert!(read_text(&latin).lossy);
        assert!(read_text(&dir.join("missing.txt")).error.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_lines() {
        assert_eq!(first_line("\n\n  Hello there  \nmore"), "Hello there");
        assert_eq!(first_line(&"x".repeat(90)).chars().count(), 81);
    }
}
