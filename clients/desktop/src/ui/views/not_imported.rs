//! Review's "Didn't import": files the server refused (missing tags…) or failed on, kept
//! with their audio (`GET /api/ingest-problems`). Left, the list; right, the chosen file:
//! where it came from, why, its tags, a play button, and where it goes (every file goes
//! somewhere; Alt+1/2/3 choose):
//!
//! 1. **a new track**: the track editor on a draft, the file as a preview imports it (its
//!    tags, its source), with `single:` staged when it's on no album. Importing applies
//!    the changes with it; it needs a title and an artist in the end (see `editor::draft`).
//!    It can be another version of a track that's there (the relation kinds that say
//!    this track first: instrumental, remix, alternate…): that track chosen, its title
//!    (with "(Instrumental)"…), the relation, its status and its artists are staged. An
//!    instrumental's artists are its producers (as primary and producer; the song's
//!    other artists are taken off if the tags had them), any other version's the song's;
//! 2. **a file of a track that's there** (`replace_file`): the track found by typing (the
//!    file's name is typed in to start with), then "make it this track's file" (its file
//!    becomes an older one) or "keep it as an older file of this track";
//! 3. **an extra** (vocals, stems, a video…) of a track, an album, an artist, a grouping,
//!    or of nothing yet. A video starts here, of kind "video"; every video shown can be
//!    kept at once too.
//!
//! Adding or importing can say the file is an original file (an og file, named as it was
//! uploaded): on to start with for WAV files, which are nearly always the artist's own.
//! Review's filter works here too: words in the name, the path (folders), the reason or
//! the tags.
//!
//! **Many at once**: files marked in the list (`x`, Ctrl+click, Shift+click a range, Ctrl+A
//! every one shown, Esc clears) go somewhere together: as new tracks (each previewed, then
//! the track editor over all their drafts: what's staged goes on every one, each keeping
//! its own title and source; imported one request each, a few at a time), or as extras of
//! one thing. A file of a track and another version of a track stay one file at a time.
//!
//! Done ones leave the list at once (the server resolves them too).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::catalog::model::Role;
use crate::catalog::Track;
use crate::catalog::Catalog;
use crate::playback::{AudioSource, QueueItem};
use crate::ui::edit::editor::{DraftOf, Imported, TrackEditor};
use crate::ui::selection::Selection;
use crate::ui::task::Task;
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

/// Where the chosen file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Fate {
    #[default]
    NewTrack,
    FileOf,
    Extra,
}

impl Fate {
    const ALL: [(Fate, &'static str, egui::Key); 3] =
        [(Fate::NewTrack, "A new track", egui::Key::Num1), (Fate::FileOf, "A file of a track that's there", egui::Key::Num2), (Fate::Extra, "An extra", egui::Key::Num3)];
}

/// What an extra is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Of {
    #[default]
    Track,
    Album,
    Artist,
    Grouping,
    Nothing,
}

impl Of {
    const ALL: [(Of, &'static str); 5] = [(Of::Track, "a track"), (Of::Album, "an album"), (Of::Artist, "an artist"), (Of::Grouping, "a grouping"), (Of::Nothing, "nothing yet")];

    /// Its kind in an extra's links.
    fn link(self) -> &'static str {
        match self {
            Of::Track => "track",
            Of::Album => "album",
            Of::Artist => "artist",
            Of::Grouping | Of::Nothing => "grouping",
        }
    }
}

/// The file as a new track: the preview's draft, and the editor on it.
struct Draft {
    /// The original filename it was previewed with (another one: previewed again).
    og: Option<String>,
    /// Waiting for a preview (its number).
    asked: Option<u64>,
    arrived: Task<(u64, Result<Imported, String>)>,
    /// The draft track (None: not there yet).
    track: Option<Uuid>,
    /// What the file lacks to be imported, as it is.
    missing: Vec<String>,
    /// It's a track's file already (that track, if it can be seen).
    duplicate: Option<Option<Uuid>>,
    error: Option<String>,
    /// To be previewed again (the library changed since).
    again: bool,
    editor: TrackEditor,
    /// It's this (`instrumental_of`…) of a track that's there: the kind chosen, and what
    /// was staged for it (in words).
    version: Option<String>,
    staged_version: Option<String>,
}

/// Many files going somewhere together (see the module's notes).
struct Bulk {
    fate: Fate,
    /// New tracks: each file's draft as it's previewed (its track, or why there's none).
    drafts: HashMap<Uuid, Result<Uuid, String>>,
    /// Files being previewed.
    asking: HashSet<Uuid>,
    arrived: Arc<Mutex<Vec<(Uuid, Result<Imported, String>)>>>,
    /// Drafts on an album already (no `single:` to start with then).
    on_albums: HashSet<Uuid>,
    /// WAV files are said to be original files (named as they were uploaded).
    og_wavs: bool,
    /// `single:` was considered (once every draft is there).
    started: bool,
    editor: TrackEditor,
    /// Extras being kept: how many done, of how many; and what came of it.
    sending: Option<(Arc<std::sync::atomic::AtomicUsize>, usize)>,
    sent: Task<(Vec<Uuid>, Vec<String>)>,
}

impl Bulk {
    fn new() -> Bulk {
        Bulk {
            fate: Fate::NewTrack,
            drafts: HashMap::new(),
            asking: HashSet::new(),
            arrived: Arc::default(),
            on_albums: HashSet::new(),
            og_wavs: true,
            started: false,
            editor: TrackEditor::default(),
            sending: None,
            sent: Task::default(),
        }
    }
}

#[derive(Default)]
pub struct NotImported {
    pub list: Vec<Problem>,
    pub loading: bool,
    pub error: Option<String>,
    asked: u64,
    arrived: Task<(u64, Result<Vec<Problem>, String>)>,
    selected: Option<Uuid>,
    fate: Fate,
    /// The track it's a file or an extra of: found by typing, then chosen (a place in the
    /// catalog).
    search: Option<InlineSearch>,
    target: Option<u32>,
    reason: String,
    /// An extra: its kind (guessed from the file's name), and what it's of.
    extra_kind: String,
    of: Of,
    /// The album or artist it's an extra of: found by typing, then chosen.
    of_search: Option<InlineSearch>,
    /// What it's an extra of, if not a track: (its id, in words).
    of_target: Option<(Uuid, String)>,
    /// The grouping it's an extra of: typed, then chosen.
    grouping_query: String,
    /// It's an original file (an og file), with this name: on to start with for WAVs.
    is_og: bool,
    og_name: String,
    /// The form was filled for this file (moving to another fills it again).
    filled_for: Option<Uuid>,
    draft: Option<Draft>,
    drafts_asked: u64,
    /// Marked files (by id), and where a range of marks starts (a row shown).
    marked: Selection<Uuid>,
    /// The marked files, going somewhere together.
    bulk: Option<Bulk>,
    sending: bool,
    done: Task<Done>,
    /// The last thing done (or why not), with the track it went to.
    notice: Option<(Result<String, String>, Option<Uuid>)>,
    /// Videos being kept as music videos, many at once: how many done, of how many.
    batch: Option<(Arc<std::sync::atomic::AtomicUsize>, usize)>,
    batch_done: Task<String>,
}

impl NotImported {
    /// Fetches every open entry in the background.
    pub fn refresh(&mut self, shell: &Shell) {
        self.asked += 1;
        self.loading = true;
        let number = self.asked;
        self.arrived.start_with(shell, |api| async move {
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
            (number, result)
        });
    }

    /// Takes in what arrived. Call once per frame.
    pub fn poll(&mut self, shell: &mut Shell) {
        let arrived = self.arrived.poll();
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
        let done = self.done.poll();
        if let Some((problem, result)) = done {
            self.sending = false;
            match result {
                Ok((text, track)) => {
                    // Gone from the list at once; on to the one in its place.
                    let at = self.list.iter().position(|p| p.id == problem);
                    self.list.retain(|p| p.id != problem);
                    self.marked.retain(|id| *id != problem);
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
        let batch_done = self.batch_done.poll();
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
        // Marks: only of files still listed.
        let listed: HashSet<Uuid> = self.list.iter().map(|p| p.id).collect();
        self.marked.retain(|id| listed.contains(id));
        let ids: Vec<Uuid> = shown.iter().map(|&n| self.list[n].id).collect();
        let selected_at = self.selected.and_then(|id| ids.iter().position(|x| *x == id)).unwrap_or(0);
        // Many files being imported or kept: shown until they're done.
        let busy = self.bulk.as_ref().is_some_and(|b| b.editor.applying() || b.sending.is_some());
        if !ui.ctx().egui_wants_keyboard_input() && !shell.modal_open && !shell.palette_open && !busy {
            let id_at = |n: usize| ids.get(n).copied();
            self.marked.keys(ui.ctx(), selected_at, ids.len(), id_at, true);
        }
        if !self.marked.is_empty() {
            // Esc is for the marks, not "back".
            shell.page_takes_back = true;
        }
        if self.marked.is_empty() && !busy {
            self.bulk = None;
        }
        let rest = ui.available_rect_before_wrap();
        let list_rect = egui::Rect::from_min_size(rest.min, Vec2::new(LIST_WIDTH, rest.height()));
        let detail_rect = egui::Rect::from_min_max(egui::pos2(list_rect.right() + 24.0, rest.top()), rest.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        self.list(&mut list_ui, &shown);
        let mut detail_ui = ui.new_child(egui::UiBuilder::new().max_rect(detail_rect));
        if self.marked.is_empty() && !busy {
            self.detail(&mut detail_ui, shell, catalog);
        } else {
            let marked: Vec<Uuid> = self.list.iter().map(|p| p.id).filter(|id| self.marked.contains(id)).collect();
            self.many(&mut detail_ui, shell, catalog, &marked);
        }
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
        let ids: Vec<Uuid> = shown.iter().map(|&n| self.list[n].id).collect();
        let selected_at = self.selected.and_then(|id| ids.iter().position(|x| *x == id)).unwrap_or(0);
        if !self.marked.is_empty() {
            ui.label(RichText::new(format!("{} marked · x marks · Esc clears", crate::ui::thousands(self.marked.len()))).color(P.accent).small());
        }
        egui::ScrollArea::vertical().id_salt("not-imported-list").auto_shrink([false, false]).show_rows(ui, ROW, shown.len(), |ui, range| {
            for n in range {
                let p = &self.list[shown[n]];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                let chosen = self.selected == Some(p.id);
                let marked = self.marked.contains(&p.id);
                if chosen || response.hovered() {
                    ui.painter().rect_filled(rect, 8, if chosen { P.accent_soft } else { P.surface });
                }
                // The mark's box (a click on it marks or unmarks).
                let mark = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(16.0));
                let on_box = response.hover_pos().is_some_and(|pos| mark.expand(6.0).contains(pos));
                super::marking::paint_box(ui.painter(), p.id, mark, marked, on_box);
                let painter = ui.painter();
                widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(34.0, 6.0), Vec2::new(rect.width() - 44.0, 18.0)), &p.uploaded_filename, font(13.5, medium()), P.text);
                let sub = p.path.as_deref().map(folder).filter(|f| !f.is_empty()).map(|f| format!("{} · {f}", p.reason)).unwrap_or_else(|| p.reason.clone());
                widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(34.0, 25.0), Vec2::new(rect.width() - 44.0, 16.0)), &sub, font(12.0, medium()), P.muted);
                if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    let id_at = |n: usize| ids.get(n).copied();
                    if on_box {
                        self.marked.toggle(n, id_at);
                    } else if modifiers.command || modifiers.shift {
                        self.marked.click(n, selected_at, modifiers, id_at);
                    } else {
                        self.selected = Some(p.id);
                        self.marked.cursor_at(n);
                    }
                }
            }
        });
    }

    fn detail(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        let Some(p) = self.selected.and_then(|id| self.list.iter().find(|p| p.id == id)).cloned() else {
            self.notice_line(ui, shell);
            return;
        };
        // A new file chosen: its name typed in to find its track, where it goes guessed.
        if self.filled_for != Some(p.id) {
            self.filled_for = Some(p.id);
            let name = searchable_name(&p.uploaded_filename);
            let mut search = InlineSearch::new(Part::Tracks);
            search.set_query(&name, shell);
            self.search = Some(search);
            self.target = None;
            self.reason.clear();
            self.is_og = is_wav(&p.uploaded_filename);
            self.og_name = p.uploaded_filename.clone();
            let video = is_video(&p.uploaded_filename);
            // (A guess the library has no such kind for is "other".)
            let guess = if video { "video" } else { guess_kind(&p.uploaded_filename) };
            let has = |kind: &str| shell.account.vocab.as_ref().is_none_or(|v| v.extra_kinds.iter().any(|k| k == kind));
            self.extra_kind = if has(guess) { guess.to_string() } else { "other".to_string() };
            self.fate = if video { Fate::Extra } else { Fate::NewTrack };
            self.of = Of::Track;
            self.of_search = None;
            self.of_target = None;
            self.grouping_query.clear();
            self.draft = None;
        }
        // Alt+1/2/3: where it goes.
        if !shell.modal_open {
            for (fate, _, key) in Fate::ALL {
                if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::ALT, key)) && (fate != Fate::FileOf || shell.account.can("replace_file")) {
                    self.fate = fate;
                }
            }
        }
        if self.fate == Fate::NewTrack {
            self.keep_draft(shell, &p);
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
                        codec: None,
                        sha256: None,
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

            // Where it goes.
            ui.horizontal(|ui| {
                for (n, (fate, label, _)) in Fate::ALL.into_iter().enumerate() {
                    if fate == Fate::FileOf && !shell.account.can("replace_file") {
                        continue;
                    }
                    if widgets::chip(ui, label, self.fate == fate) {
                        self.fate = fate;
                    }
                    ui.label(RichText::new(format!("Alt+{}", n + 1)).color(P.faint).small());
                    ui.add_space(6.0);
                }
            });
            ui.add_space(8.0);
            // A track or a track's file: whether it's an original file.
            if self.fate != Fate::Extra {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.is_og, "It's an original file (og file)").on_hover_text("The file as the artist made and named it, not a rip or a download. On to start with for WAV files");
                    if self.is_og {
                        ui.label(RichText::new("named").color(P.muted));
                        ui.add(egui::TextEdit::singleline(&mut self.og_name).id(egui::Id::new("not-imported-og")).desired_width(360.0));
                    }
                });
                ui.add_space(10.0);
            }
            match self.fate {
                Fate::NewTrack => self.new_track(ui, shell, catalog, &p),
                Fate::FileOf => self.add_to_track(ui, shell, catalog, &p),
                Fate::Extra => self.extra(ui, shell, catalog, &p),
            }
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

    /// "A file of a track that's there": the track, then as its file or an older one.
    fn add_to_track(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, p: &Problem) {
        ui.label(RichText::new("The same song already in the library (a better copy without tags, an older version…). Found by its name; type to find another.").color(P.muted));
        if let Some(search) = self.search.as_mut() {
            let hit = search.show(ui, shell, Some(catalog), egui::Id::new("not-imported-track"), "the track: type its title or artist", &[]);
            if let Some((Choice::Hit(i), _)) = hit {
                self.target = Some(i);
            }
        }
        let Some(i) = self.target else { return };
        let t = catalog.track_at(i);
        ui.add_space(6.0);
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                if play(ui, "not-imported-play-track", 28.0) {
                    shell.act(Action::Play { tracks: vec![i], start: 0 });
                }
                ui.vertical(|ui| {
                    if widgets::link(ui, t.title(), font(15.0, semibold()), P.text) {
                        shell.act(Action::Go(Page::Track(t.id())));
                    }
                    let album = catalog.main_album(t).map(|a| format!(" · {}", a.title())).unwrap_or_default();
                    let codec = t.codec().map(|c| format!(" · {c}")).unwrap_or_default();
                    ui.label(RichText::new(format!("{}{album}{codec}", t.artist_display())).color(P.muted));
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
                    let body = serde_json::json!({ "track_id": t.id(), "add_as": add_as, "reason": self.reason.trim(), "original_filename": self.og() });
                    let text = if current { format!("“{}” plays this file now (its old file is kept as an older one).", t.title()) } else { format!("Kept as an older file of “{}”.", t.title()) };
                    self.send(shell, p.id, format!("/api/ingest-problems/{}/add-to-track", p.id), body, Some(t.id()), text);
                }
            });
        });
    }

    /// Keeps the draft up to date: previewed for this file, and again when it's said to be
    /// another original file (once that name isn't being typed).
    fn keep_draft(&mut self, shell: &Shell, p: &Problem) {
        let og = self.og();
        let typing = shell.ctx.memory(|m| m.has_focus(egui::Id::new("not-imported-og")));
        let draft = self.draft.get_or_insert_with(|| Draft {
            og: og.clone(),
            asked: None,
            arrived: Task::default(),
            track: None,
            missing: Vec::new(),
            duplicate: None,
            error: None,
            again: false,
            editor: TrackEditor::default(),
            version: None,
            staged_version: None,
        });
        if !draft.editor.take_stale_drafts().is_empty() {
            draft.again = true;
        }
        let arrived = draft.arrived.poll();
        if let Some((number, answer)) = arrived
            && draft.asked == Some(number)
        {
            draft.asked = None;
            match answer {
                Ok(imported) => match (imported.track, imported.draft) {
                    (Some(track), Some(ids)) => {
                        let first = draft.track.is_none();
                        draft.track = Some(track.id);
                        draft.missing = imported.missing;
                        let flags = imported.edited.map(|e| e.flags).unwrap_or_default();
                        let with = serde_json::json!({ "original_filename": draft.og });
                        let loose = track.albums.is_empty();
                        draft.editor.draft(DraftOf { problem: p.id, path: format!("/api/ingest-problems/{}/import", p.id), ids, with, flags }, track);
                        if first {
                            if loose {
                                draft.editor.start_single();
                            }
                            draft.editor.opened();
                        }
                    }
                    _ => draft.duplicate = Some(imported.track_id),
                },
                Err(err) => draft.error = Some(err),
            }
        }
        let stale = draft.og != og && !typing;
        if draft.asked.is_none() && draft.error.is_none() && draft.duplicate.is_none() && (draft.track.is_none() || stale || draft.again) {
            draft.og = og.clone();
            draft.again = false;
            self.drafts_asked += 1;
            let number = self.drafts_asked;
            draft.asked = Some(number);
            let path = format!("/api/ingest-problems/{}/import", p.id);
            let body = serde_json::json!({ "preview": true, "original_filename": og });
            draft.arrived.start_with(shell, |api| async move { (number, api.send_json::<Imported>(reqwest::Method::POST, &path, Some(body)).await.map_err(|e| e.message)) });
        }
    }

    /// "A new track": the editor on the draft.
    fn new_track(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, p: &Problem) {
        let Some(draft) = self.draft.as_mut() else { return };
        if let Some(err) = &draft.error {
            ui.label(RichText::new(format!("Couldn't preview it: {err}")).color(P.danger));
            if ui.button("Try again").clicked() {
                draft.error = None;
            }
            return;
        }
        if let Some(track) = draft.duplicate {
            ui.label(RichText::new("This exact file is a track's already.").color(P.muted));
            if let Some(track) = track
                && widgets::link(ui, "open the track", font(13.0, medium()), P.muted)
            {
                shell.act(Action::Go(Page::Track(track)));
            }
            return;
        }
        let Some(track) = draft.track else {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Importing it as a preview…").color(P.faint));
            });
            return;
        };
        let lacks = |what: &str| draft.missing.iter().any(|m| m == what);
        let needs = match (lacks("title"), lacks("artist")) {
            (true, true) => Some("Its tags have no title and no artist: give them (title:…  artist:…)."),
            (true, false) => Some("Its tags have no title: give it one (title:…)."),
            (false, true) => Some("Its tags have no artist: give it one (artist:…)."),
            (false, false) => None,
        };
        if let Some(needs) = needs {
            ui.label(RichText::new(needs).color(P.text));
        }
        if draft.asked.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Previewing it again (another original file name, or the library changed)…").color(P.faint));
            });
        }
        // The instrumental (acapella, remix…) of a track that's there.
        // (The kinds that say this track first: "%A is a remix of %B", not "%B is…".)
        let kinds: Vec<String> = shell.account.vocab.as_ref().map(|v| v.relation_kinds.iter().filter(|k| k.template.starts_with("%A")).map(|k| k.kind.clone()).collect()).unwrap_or_default();
        if !kinds.is_empty() && shell.account.can("edit_relations") {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Another version of a track that's there:").color(P.muted));
                for kind in &kinds {
                    let chosen = draft.version.as_deref() == Some(kind.as_str());
                    if widgets::chip(ui, &version_label(kind), chosen) {
                        draft.version = (!chosen).then(|| kind.clone());
                    }
                }
            });
            if let Some(kind) = draft.version.clone()
                && let Some(search) = self.search.as_mut()
            {
                let hint = format!("the track it's a version of ({}): type its title or artist", version_label(&kind));
                if let Some((Choice::Hit(i), _)) = search.show(ui, shell, Some(catalog), egui::Id::new("not-imported-version-of"), &hint, &[]) {
                    let t = catalog.track_at(i);
                    let now = draft.editor.credits_after(track).unwrap_or_default();
                    let (lines, tracks, artists) = version_of(&kind, t, &now, catalog);
                    let mut words = format!("Staged as a version ({}) of “{}”.", version_label(&kind), t.title());
                    if artists.iter().all(|(_, _, added)| !added) {
                        let whose = if kind == INSTRUMENTAL { "It has no producer credited" } else { "It has no artists" };
                        words.push_str(&format!(" {whose}: type its artist (artist:…)."));
                    }
                    draft.staged_version = Some(words);
                    let artists: Vec<(String, Uuid)> = artists.into_iter().map(|(name, id, _)| (name, id)).collect();
                    draft.editor.stage_for_you(lines, &tracks, &artists);
                }
            }
            if let Some(words) = &draft.staged_version {
                ui.label(RichText::new(words).color(P.muted).small());
            }
        }
        ui.add_space(4.0);
        draft.editor.offer(&[]);
        draft.editor.show(ui, shell, catalog, &[track], None);
        if let Some((_, made)) = draft.editor.take_imported().into_iter().next() {
            self.done.put((p.id, Ok(("Imported.".into(), Some(made)))));
        }
    }

    /// "An extra": its kind, and what it's of.
    fn extra(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, p: &Problem) {
        ui.label(RichText::new("Not a track of its own: kept with what it's of (vocals, a stem, a video…), and seen like it.").color(P.muted));
        ui.add_space(4.0);
        let Some((links, what, track)) = self.extra_of(ui, shell, catalog) else { return };
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with it)").desired_width(420.0));
        let label = format!("Keep it as an extra ({}) of {what}", self.extra_kind);
        if ui.add_enabled(!self.sending, egui::Button::new(label)).clicked() {
            let body = serde_json::json!({ "name": searchable_name(&p.uploaded_filename), "kind": self.extra_kind, "links": links, "reason": self.reason.trim() });
            let text = format!("Kept as an extra ({}) of {what}.", self.extra_kind);
            self.send(shell, p.id, format!("/api/ingest-problems/{}/extra", p.id), body, track, text);
        }
    }

    /// An extra's kind and what it's of, chosen: (its links, what it's of in words, the
    /// track it's of). None: nothing chosen yet (it says so).
    fn extra_of(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) -> Option<(Vec<serde_json::Value>, String, Option<Uuid>)> {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Kind:").color(P.muted));
            for k in &crate::ui::edit::extras::kinds(shell) {
                if widgets::chip(ui, k, self.extra_kind == *k) {
                    self.extra_kind = k.to_string();
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Of:").color(P.muted));
            for (of, label) in Of::ALL {
                if widgets::chip(ui, label, self.of == of) && self.of != of {
                    self.of = of;
                    self.of_target = None;
                    self.of_search = match of {
                        Of::Album => Some(InlineSearch::new(Part::Albums)),
                        Of::Artist => Some(InlineSearch::new(Part::Artists)),
                        _ => None,
                    };
                }
            }
        });
        ui.add_space(6.0);
        // What it's of: (its id, in words).
        let target: Option<(Uuid, String)> = match self.of {
            Of::Track => {
                let search = self.search.get_or_insert_with(|| InlineSearch::new(Part::Tracks));
                if let Some((Choice::Hit(i), _)) = search.show(ui, shell, Some(catalog), egui::Id::new("not-imported-track"), "the track: type its title or artist", &[]) {
                    self.target = Some(i);
                }
                self.target.and_then(|i| catalog.get_track(i)).map(|t| (t.id(), format!("the track “{}” ({})", t.title(), t.artist_display())))
            }
            Of::Album | Of::Artist => {
                let album = self.of == Of::Album;
                if let Some(search) = self.of_search.as_mut() {
                    let hint = if album { "the album: type its title" } else { "the artist: type their name" };
                    if let Some((Choice::Hit(i), _)) = search.show(ui, shell, Some(catalog), egui::Id::new("not-imported-of"), hint, &[]) {
                        self.of_target = if album {
                            catalog.get_album(i).map(|a| (a.id(), format!("the album “{}” ({})", a.title(), a.artist_display())))
                        } else {
                            catalog.artists.get(i as usize).map(|a| (a.id, format!("the artist “{}”", a.name)))
                        };
                    }
                }
                self.of_target.clone()
            }
            Of::Grouping => {
                ui.add(egui::TextEdit::singleline(&mut self.grouping_query).hint_text("the grouping: type its name").desired_width(420.0));
                let query = self.grouping_query.trim().to_lowercase();
                if !query.is_empty() {
                    for g in catalog.groupings.iter().filter(|g| g.name.to_lowercase().contains(&query)).take(8) {
                        let chosen = self.of_target.as_ref().is_some_and(|t| t.0 == g.id);
                        if ui.selectable_label(chosen, format!("{} ({})", g.name, g.kind)).clicked() {
                            self.of_target = Some((g.id, format!("the {} “{}”", g.kind, g.name)));
                        }
                    }
                }
                self.of_target.clone()
            }
            Of::Nothing => None,
        };
        ui.add_space(6.0);
        let what = match (&target, self.of) {
            (_, Of::Nothing) => "nothing yet (Review: “Unlinked extras” links it later)".to_string(),
            (Some((_, words)), _) => words.clone(),
            (None, _) => {
                ui.label(RichText::new("Choose what it's of.").color(P.faint));
                return None;
            }
        };
        let links: Vec<serde_json::Value> = target.iter().map(|(id, _)| serde_json::json!({ "kind": self.of.link(), "id": id })).collect();
        let track = (self.of == Of::Track).then(|| target.as_ref().map(|t| t.0)).flatten();
        Some((links, what, track))
    }

    /// The marked files, going somewhere together: as new tracks (the editor over their
    /// drafts) or as extras of one thing.
    fn many(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, marked: &[Uuid]) {
        self.notice_line(ui, shell);
        let bulk = self.bulk.get_or_insert_with(Bulk::new);
        // Alt+1/3: where they go (a file of a track is one at a time).
        if !shell.modal_open && !bulk.editor.applying_or_applied() && bulk.sending.is_none() {
            for (fate, key) in [(Fate::NewTrack, egui::Key::Num1), (Fate::Extra, egui::Key::Num3)] {
                if ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::ALT, key)) {
                    bulk.fate = fate;
                }
            }
        }
        ui.label(RichText::new(format!("{} files", crate::ui::thousands(marked.len()))).font(font(22.0, bold())));
        ui.horizontal(|ui| {
            for (fate, label, key) in [(Fate::NewTrack, "New tracks", "Alt+1"), (Fate::Extra, "Extras", "Alt+3")] {
                if widgets::chip(ui, label, bulk.fate == fate) && !bulk.editor.applying_or_applied() && bulk.sending.is_none() {
                    bulk.fate = fate;
                }
                ui.label(RichText::new(key).color(P.faint).small());
                ui.add_space(6.0);
            }
            ui.label(RichText::new("(a file of a track, or a version of one: one file at a time)").color(P.faint).small());
        });
        ui.add_space(8.0);
        match bulk.fate {
            Fate::Extra => self.many_extras(ui, shell, catalog, marked),
            _ => self.many_tracks(ui, shell, catalog, marked),
        }
    }

    /// The marked files as new tracks: each previewed, then the editor over the drafts.
    fn many_tracks(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, marked: &[Uuid]) {
        let wav: HashSet<Uuid> = self.list.iter().filter(|p| is_wav(&p.uploaded_filename)).map(|p| p.id).collect();
        let names: HashMap<Uuid, String> = self.list.iter().map(|p| (p.id, p.uploaded_filename.clone())).collect();
        let Some(bulk) = self.bulk.as_mut() else { return };
        // What's arrived.
        let arrived = std::mem::take(&mut *bulk.arrived.lock().unwrap());
        for (problem, answer) in arrived {
            bulk.asking.remove(&problem);
            let result = match answer {
                Ok(imported) => match (imported.track, imported.draft) {
                    (Some(track), Some(ids)) => {
                        let id = track.id;
                        if track.albums.is_empty() {
                            bulk.on_albums.remove(&problem);
                        } else {
                            bulk.on_albums.insert(problem);
                        }
                        let og = (bulk.og_wavs && wav.contains(&problem)).then(|| names.get(&problem).cloned()).flatten();
                        let flags = imported.edited.map(|e| e.flags).unwrap_or_default();
                        bulk.editor.draft(DraftOf { problem, path: format!("/api/ingest-problems/{problem}/import"), ids, with: serde_json::json!({ "original_filename": og }), flags }, track);
                        Ok(id)
                    }
                    _ => Err("this exact file is a track's already".to_string()),
                },
                Err(err) => Err(err),
            };
            bulk.drafts.insert(problem, result);
        }
        // Stale ones (the library changed since): previewed again.
        for problem in bulk.editor.take_stale_drafts() {
            bulk.drafts.remove(&problem);
        }
        // WAVs as original files, or not: theirs previewed again.
        let applying = bulk.editor.applying_or_applied();
        ui.add_enabled_ui(!applying, |ui| {
            if ui.checkbox(&mut bulk.og_wavs, "WAV files are original files (og files, named as uploaded)").changed() {
                for problem in &wav {
                    bulk.drafts.remove(problem);
                }
            }
        });
        // Each file previewed (a few at a time).
        if !applying {
            let limit = Arc::new(tokio::sync::Semaphore::new(4));
            let ask: Vec<&Uuid> = marked.iter().filter(|p| !bulk.drafts.contains_key(p) && !bulk.asking.contains(p)).collect();
            for problem in ask {
                bulk.asking.insert(*problem);
                let og = (bulk.og_wavs && wav.contains(problem)).then(|| names.get(problem).cloned()).flatten();
                let body = serde_json::json!({ "preview": true, "original_filename": og });
                let (api, arrived, ctx, limit, problem) = (shell.api.clone(), bulk.arrived.clone(), shell.ctx.clone(), limit.clone(), *problem);
                shell.runtime.spawn(async move {
                    let _permit = limit.acquire().await;
                    let answer = api.send_json::<Imported>(reqwest::Method::POST, &format!("/api/ingest-problems/{problem}/import"), Some(body)).await.map_err(|e| e.message);
                    arrived.lock().unwrap().push((problem, answer));
                    ctx.request_repaint();
                });
            }
        }
        let waiting = marked.iter().filter(|p| !bulk.drafts.contains_key(p)).count();
        if waiting > 0 {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new(format!("Importing them as a preview… {} of {}", marked.len() - waiting, marked.len())).color(P.faint));
            });
            return;
        }
        let refused: Vec<String> = marked.iter().filter_map(|p| Some(format!("{}: {}", names.get(p)?, bulk.drafts.get(p)?.as_ref().err()?))).collect();
        if !refused.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("{} of them left out", refused.len())).color(P.danger)).id_salt("not-imported-left-out").show(ui, |ui| {
                for line in &refused {
                    ui.label(RichText::new(line).color(P.muted).small());
                }
            });
        }
        let targets: Vec<Uuid> = marked.iter().filter_map(|p| bulk.drafts.get(p)?.as_ref().ok().copied()).collect();
        if targets.is_empty() {
            return;
        }
        // A loose track becomes a single of its own: when none of them is on an album.
        if !bulk.started {
            bulk.started = true;
            if marked.iter().all(|p| !bulk.on_albums.contains(p)) {
                bulk.editor.start_single();
            }
        }
        bulk.editor.offer(&[]);
        bulk.editor.show(ui, shell, catalog, &targets, None);
        let imported = bulk.editor.take_imported();
        if !imported.is_empty() {
            let gone: HashSet<Uuid> = imported.iter().map(|(problem, _)| *problem).collect();
            self.list.retain(|p| !gone.contains(&p.id));
            self.marked.retain(|id| !gone.contains(id));
            let failed = self.marked.len();
            let mut text = format!("Imported {} files.", crate::ui::thousands(gone.len()));
            if failed > 0 {
                text.push_str(&format!(" {failed} weren't (still marked: the editor says why)."));
            }
            self.notice = Some((Ok(text), imported.first().map(|(_, track)| *track).filter(|_| gone.len() == 1)));
            shell.act(Action::RefreshLibrary);
        }
    }

    /// The marked files as extras of one thing, one after another.
    fn many_extras(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, marked: &[Uuid]) {
        let sent = self.bulk.as_mut().and_then(|b| b.sent.poll());
        if let Some((kept, failed)) = sent {
            let gone: HashSet<Uuid> = kept.iter().copied().collect();
            self.list.retain(|p| !gone.contains(&p.id));
            self.marked.retain(|id| !gone.contains(id));
            let mut text = format!("Kept {} files as extras.", crate::ui::thousands(kept.len()));
            if !failed.is_empty() {
                text.push_str(&format!(" Not these: {}", failed.join("; ")));
            }
            self.notice = Some((if failed.is_empty() { Ok(text) } else { Err(text) }, None));
            if let Some(b) = self.bulk.as_mut() {
                b.sending = None;
            }
            shell.act(Action::RefreshLibrary);
            return;
        }
        if let Some((done, of)) = self.bulk.as_ref().and_then(|b| b.sending.clone()) {
            ui.add(egui::ProgressBar::new(done.load(std::sync::atomic::Ordering::Relaxed) as f32 / of.max(1) as f32).text("Keeping them as extras…"));
            return;
        }
        ui.label(RichText::new("Not tracks of their own: each kept with what it's of (named after its file), and seen like it.").color(P.muted));
        ui.add_space(4.0);
        let Some((links, what, _)) = self.extra_of(ui, shell, catalog) else { return };
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with each)").desired_width(420.0));
        let label = format!("Keep the {} files as extras ({}) of {what}", crate::ui::thousands(marked.len()), self.extra_kind);
        if ui.button(label).clicked() {
            let files: Vec<(Uuid, String)> = self.list.iter().filter(|p| marked.contains(&p.id)).map(|p| (p.id, p.uploaded_filename.clone())).collect();
            let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let Some(bulk) = self.bulk.as_mut() else { return };
            bulk.sending = Some((done.clone(), files.len()));
            let ctx = shell.ctx.clone();
            let (kind, reason) = (self.extra_kind.clone(), self.reason.trim().to_string());
            bulk.sent.send_with(shell, |api| async move {
                let (mut kept, mut failed) = (Vec::new(), Vec::new());
                for (id, file) in files {
                    let body = serde_json::json!({ "name": searchable_name(&file), "kind": kind, "links": links, "reason": reason });
                    match api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/ingest-problems/{id}/extra"), Some(body)).await {
                        Ok(_) => kept.push(id),
                        Err(err) => failed.push(format!("{file}: {}", err.message)),
                    }
                    done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ctx.request_repaint();
                }
                (kept, failed)
            });
        }
    }

    /// Videos kept as music videos, one after another (each gets its facts and thumbnail
    /// on the server): extras of kind "video", linked to nothing yet.
    fn keep_videos(&mut self, shell: &Shell, videos: Vec<(Uuid, String)>) {
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.batch = Some((done.clone(), videos.len()));
        let ctx = shell.ctx.clone();
        self.batch_done.send_with(shell, |api| async move {
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
            text
        });
    }

    /// The original filename to send: the name, if it's said to be an original file.
    fn og(&self) -> Option<String> {
        Some(self.og_name.trim().to_string()).filter(|n| self.is_og && !n.is_empty())
    }

    /// Sends an action on `problem`; `track`: the track it goes to (known, or from the
    /// answer's `track_id`).
    fn send(&mut self, shell: &Shell, problem: Uuid, path: String, body: serde_json::Value, track: Option<Uuid>, text: String) {
        self.sending = true;
        self.notice = None;
        self.done.send_with(shell, |api| async move {
            let answer = api.send_json::<serde_json::Value>(reqwest::Method::POST, &path, Some(body)).await.map(Some);
            let result = answer
                .map(|answer| {
                    let track = track.or_else(|| answer.as_ref()?.get("track_id")?.as_str()?.parse().ok());
                    (text, track)
                })
                .map_err(|e| e.message);
            (problem, result)
        });
    }
}

/// The relation kind whose artists are the producers of the song's.
const INSTRUMENTAL: &str = "instrumental_of";

/// A relation kind as a choice: "instrumental_of" → "instrumental", "sped_up_of" → "sped up".
fn version_label(kind: &str) -> String {
    kind.strip_suffix("_of").unwrap_or(kind).replace('_', " ")
}

/// What's staged to make a draft the `kind` (`instrumental_of`, `alternate`…) of `t`, as
/// typed lines: its title (`t`'s, with "(Instrumental)" for a `…_of` kind), the relation,
/// `t`'s status, and its artists. An instrumental's are `t`'s producers, as primary and as
/// producer (`t`'s other primary artists taken off where `credits`, the draft's as staged,
/// has them); any other version's are `t`'s, in their roles. With the tracks and artists
/// the lines name: (name, id), and for artists whether it's one credited.
fn version_of(kind: &str, t: Track<'_>, credits: &[crate::ui::edit::editor::model::Credit], catalog: &Catalog) -> (Vec<String>, Vec<(String, Uuid)>, Vec<(String, Uuid, bool)>) {
    use crate::ui::edit::editor::quoted;
    let title = match kind.strip_suffix("_of") {
        Some(word) => {
            let word: Vec<String> = word
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
                })
                .collect();
            format!("{} ({})", t.title(), word.join(" "))
        }
        None => t.title().to_string(),
    };
    let mut lines = vec![format!("title:{}", quoted(&title)), format!("related:{} kind:{kind}", quoted(t.title())), format!("status:{}", t.status())];
    let name = |id: Uuid| catalog.artist(id).map(|a| a.name.clone());
    let role = |r: &Role| match r {
        Role::Primary => Some("primary"),
        Role::Featured => Some("featured"),
        Role::Producer => Some("producer"),
        Role::Other => None,
    };
    let mut artists = Vec::new();
    if kind == INSTRUMENTAL {
        let producers: Vec<Uuid> = t.credits().iter().filter(|c| c.1 == Role::Producer).map(|c| c.0).collect();
        for c in t.credits().iter().filter(|c| c.1 == Role::Primary && !producers.contains(&c.0)) {
            if credits.iter().any(|d| d.artist_id == c.0)
                && let Some(n) = name(c.0)
            {
                lines.push(format!("-artist:{}", quoted(&n)));
                artists.push((n, c.0, false));
            }
        }
        for p in producers {
            if let Some(n) = name(p) {
                lines.push(format!("artist:{} role:primary", quoted(&n)));
                lines.push(format!("artist:{} role:producer", quoted(&n)));
                artists.push((n, p, true));
            }
        }
    } else {
        for c in t.credits() {
            if let (Some(n), Some(r)) = (name(c.0), role(&c.1)) {
                lines.push(format!("artist:{} role:{r}", quoted(&n)));
                artists.push((n, c.0, true));
            }
        }
    }
    (lines, vec![(t.title().to_string(), t.id())], artists)
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
    fn the_instrumental_of_a_track_is_its_producers() {
        use crate::ui::edit::editor::model::Credit;
        let (ann, bo, cy) = ("00000000-0000-7000-8000-0000000000a1", "00000000-0000-7000-8000-0000000000a2", "00000000-0000-7000-8000-0000000000a3");
        let song = "00000000-0000-7000-8000-0000000000b1";
        let body = serde_json::json!({
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": [[song, "Glass: Harbor", "Ann, Cy", [[ann, "primary", null], [cy, "primary", null], [bo, "producer", null], [cy, "producer", null]], [], 1000,
                                 "00000000-0000-7000-8000-0000000000f1", null, null, null, null, "unreleased", [], "2026-01-01", false, null, [], [], null, null, null]]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"], "rows": []},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[ann, "Ann", "", [], 1, 0], [bo, "Bo Lind", "", [], 1, 0], [cy, "Cy", "", [], 1, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        let catalog = Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap());
        let t = catalog.track_at(0);
        // The tags credited the song's artists.
        let tags = [ann, cy].map(|a| Credit { artist_id: a.parse().unwrap(), role: "primary".into(), alias_id: None });
        let (lines, tracks, artists) = version_of("instrumental_of", t, &tags, &catalog);
        assert_eq!(
            lines,
            [
                "title:\"Glass: Harbor (Instrumental)\"",
                "related:\"Glass: Harbor\" kind:instrumental_of",
                "status:unreleased",
                "-artist:Ann",
                "artist:Bo Lind role:primary",
                "artist:Bo Lind role:producer",
                "artist:Cy role:primary",
                "artist:Cy role:producer",
            ]
        );
        assert_eq!(tracks, [("Glass: Harbor".to_string(), song.parse().unwrap())]);
        let producers: Vec<&str> = artists.iter().filter(|a| a.2).map(|a| a.0.as_str()).collect();
        assert_eq!(producers, ["Bo Lind", "Cy"]);
        assert_eq!(version_label("sped_up_of"), "sped up");

        // Any other version: the song's artists, in their roles.
        let (lines, _, _) = version_of("sped_up_of", t, &tags, &catalog);
        assert_eq!(
            lines[..2],
            ["title:\"Glass: Harbor (Sped Up)\"".to_string(), "related:\"Glass: Harbor\" kind:sped_up_of".to_string()]
        );
        assert_eq!(lines[3..], ["artist:Ann role:primary", "artist:Cy role:primary", "artist:Bo Lind role:producer", "artist:Cy role:producer"]);
        // An alternate version: the same title.
        let (lines, _, _) = version_of("alternate", t, &[], &catalog);
        assert_eq!(lines[..2], ["title:\"Glass: Harbor\"".to_string(), "related:\"Glass: Harbor\" kind:alternate".to_string()]);
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
