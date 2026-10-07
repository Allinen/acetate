//! The track editor: one or many tracks changed by typing what to change, the way a filter
//! is typed: `artist:Ann role:featured`, `album:Some EP #1+`, `status:demo`, `date:2020`
//! (see `parse` for the line, `resolve` for what it means). The one editor for tracks: the
//! track page in edit mode, Review, and the tables' Ctrl+E (one track or the marked ones).
//!
//! * **The line** (a `ui::edit::commandline`, its language `crate::commandline`'s: `|`
//!   between changes, each a key with its own arguments): Enter turns it into staged
//!   changes (chips), all of it or, if a part can't be (no artist like that), none of it,
//!   saying why. While a part is typed, its
//!   choices are listed under the line (artists, albums, tracks, roles, statuses…):
//!   ↑/↓ choose, **Tab** takes the choice and writes the next part to type (after an
//!   artist its `role:`, then `as:`, then its place `#`), Enter takes it and stages the
//!   line. Plain words find what can be changed by name. Backspace on an empty line takes
//!   the last chip back to edit; `/` comes back to the line.
//! * **State**: with one track, its fields as rows, each with the flags about it (now, and
//!   after, from the server's preview). ↓ from an empty line goes into the rows (↑/↓ or
//!   j/k, Esc back): Enter edits a row through the line (`title:` and its value, ready to
//!   change); on an artist `r` its role, `a` the name it's credited under, Alt+↑/↓ its
//!   place, Delete takes it off (and an album, a related track, the date); on a cover `m`
//!   makes it the main one, Alt+↑/↓ moves it, Space shows the original; on a source Enter
//!   corrects it (its form opens under the rows), `o` says the file came from it. With
//!   many tracks: each field as they have it (how many differ), and as they will.
//! * **Singles and groupings**: `single:` makes each track a single of its own (last, from
//!   the track as the edit leaves it; a track on an album already gets one too, said in
//!   the preview), `group:` puts it in a grouping (or a new one, `kind:` after it), and
//!   `-group:` takes it out.
//! * **Verbs**: plain words also find the tools that stay separate, where the page around
//!   the editor has them (`offer`): merging the two marked tracks, `credited as`… Taking one
//!   hands it to the page (`take_verb`), which opens its tool.
//! * **On a track's page** the line is the page's commandline (`show_on_page`; in listen
//!   mode the line alone, `show_line`): the page's keys (`K::Page`: `play:`, `note:`,
//!   `og:`…) and its actions (plain words) are in it too, and mean what
//!   `views::track_commands` says. Its changes are staged as chips with the editor's, and
//!   Ctrl+Enter hands them to the page (`take_page_edits`), which sends each on its own.
//! * **Covers and sources** are changes like the rest, applied with them: `cover:paste`
//!   (or Ctrl+V with an image, or an image dropped, anywhere in the editor), `cover:file`,
//!   `cover:<album or track>` (a copy of its cover), `cover:#3 #1`, `-cover:#2`;
//!   `source:<link>`, `source:custom vk, …`, `source:distrokid album_uuid:… index:1` (any
//!   platform, field by field, as the server's shapes have them), `-source:#1`,
//!   `origin:#2`. An image is
//!   uploaded as soon as it's given (staged on the server), and only becomes a cover when
//!   the edit is applied.
//! * **Preview**: each track's changes in the history's words, the albums that change
//!   along (`along:off` to leave them), and its review flags after (fixed, still there,
//!   new), from the server's own checks. Ctrl+Enter applies what it shows; with many
//!   tracks only once it's up to date, with one as soon as it is.
//! * **Flags by hand**: `flag:Missing SoundCloud metadata` flags it ("Fix later", or a kind
//!   people made; a name no kind has makes a new one, listed on Review like the others),
//!   `-flag:…` settles its open flags of that kind (any kind). Many tracks at once too.
//! * **Import metadata from**: a SoundCloud track's JSON, a yt-dlp JSON or a YouTube link
//!   pasted (Ctrl+V, into the empty line or outside any field; one track) stages what it
//!   says (see `import`): `version:SoundCloud` (a new version with only that, not the shown
//!   one; on a draft, its version renamed), title, date, genre, the artists (the one whose
//!   account it is, else one by that name, else a new one), the uploader's account given
//!   to that artist (a chip of its own: take it back for someone else's upload), the
//!   track's source there and its artwork as a cover.
//! * **Credits in the title**: `parse:feat prod [prod]` finds them with the rules typed
//!   (and `split:`, `whole:`, `keep:`, `word:` after it; see `crate::title_credits`), each
//!   track its own result (`title_parse`): its title without them, and the names credited
//!   (an artist's name or alias exactly, ignoring case; none: a new artist, made when it's
//!   staged; more than one: chosen before applying). **The check** (`check`, in place of
//!   the fields with many tracks, under the rows with one; ↓ goes to it) lists every name
//!   and what it credits, the corrections, and every track it changes, the odd ones
//!   first: Enter says who a name is, `c` the next artist with that name, `s` splits it,
//!   `w` keeps a name whole, `n` says it isn't one, `x` leaves a track out, Delete takes a
//!   correction back. Each correction parses again (`title_parse::TitlesParsed::again`).
//! * **A draft** (`draft`): a file that didn't import, as a preview imports it. Edited like
//!   a track (its source among the rows); applying imports it with the changes. It starts
//!   with `single:` staged when it's on no album, so a loose track gets a single of its own.

mod check;
mod engine;
mod import;
pub mod model;
mod panel;
mod parse;
mod resolve;
mod title_parse;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};
use uuid::Uuid;

use engine::{Change, Engine, Outcome, Phase, PlanState};
use model::Op;
use panel::{Flag, Row, RowKind};
use resolve::{Candidate, Ctx, Names, Pick, PinKind, Unit, Want};
pub use resolve::find_in;
use crate::account::Vocab;
use crate::details::AliasInfo;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::edit::commandline::{self as cl, CommandLine};
use crate::ui::views::page_commands::{self as pc, Chosen, Known, Taken};
use crate::ui::views::track_commands::{self as tc, PageKey};
use crate::ui::{thousands, Action, Shell};

pub use engine::{DraftOf, Imported};
use import::{Metadata, Pasted};
pub use panel::line_for;
pub use parse::{quoted, K};

/// A line in the editor's language (its keys, and the track page's).
#[cfg(test)]
pub fn parse_line(text: &str) -> parse::Line {
    parse::parse(text)
}

/// A staged change: what it was typed as, what it does.
#[derive(Debug, Clone)]
struct Staged {
    text: String,
    label: String,
    ops: Vec<Op>,
    warning: Option<String>,
    /// Its text can be typed back (Backspace puts it in the line). Not for changes made
    /// for you whose names alone wouldn't say which artist (ambiguous names).
    editable: bool,
    /// Staged by `stage_for_you` (staging for you again replaces it).
    for_you: bool,
    /// One of the track page's changes (`ops` is empty): sent on its own by the page.
    page: Option<tc::Edit>,
}

/// A tool that stays separate, opened from the line (plain words find it) by the page around
/// the editor, which says which it has (`TrackEditor::offer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// The two marked tracks side by side, to merge.
    MergeTwo,
    /// An artist's name on the marked tracks and their albums.
    CreditedAs,
    /// The marked tracks' files marked as original files.
    MarkOriginal,
}

impl Verb {
    /// How the list says it (the words that find it are in it).
    pub fn label(self) -> &'static str {
        match self {
            Verb::MergeTwo => "Merge the two marked tracks…  (Ctrl+M)",
            Verb::CreditedAs => "Credited as… (an artist's name on these tracks and their albums)",
            Verb::MarkOriginal => "Mark as original files… (og files whose tags didn't say so)",
        }
    }
}

/// What resolving a line needs, apart from the editor itself.
struct Here<'a> {
    catalog: &'a Catalog,
    vocab: &'a Vocab,
    targets: usize,
    album: Option<Uuid>,
    staged_album: Option<Uuid>,
    may_relate: bool,
    may_source: bool,
    /// For `parse:` (see `Ctx`).
    titles: Option<&'a [(Uuid, String)]>,
    artist_names: Option<&'a crate::title_credits::NameTable>,
    /// The track page around it: its keys mean something.
    page: Option<&'a tc::Here<'a>>,
}

/// An image uploaded for a cover: (its hash, what it is in words, where it goes), or why not.
type Uploaded = Result<(String, String, Option<usize>), String>;

static NEXT: AtomicU64 = AtomicU64::new(1);

pub struct TrackEditor {
    /// Its own (its line's id: several editors may exist, one per track being edited).
    id: u64,
    /// The line: what's typed, its list, the cursor.
    cl: CommandLine,
    staged: Vec<Staged>,
    names: Names,
    lookup: Lookup,
    /// Tracks to copy a cover from (`lookup` has the albums).
    lookup_tracks: Lookup,
    engine: Engine,
    /// Esc was pressed with changes staged: again discards them.
    confirm_discard: bool,
    notice: Option<String>,
    /// The line resolved: (what for, the units).
    resolved: Option<(u64, Vec<Unit>)>,
    /// The version edited (one track; None: the shown one).
    variant: Option<Uuid>,
    /// A new version was just saved with this name: it's chosen for editing once the page has it.
    choose_version: Option<String>,
    /// The row the keys are on (one track); None: the line.
    row: Option<usize>,
    /// A one-track edit was just applied: its flags after (taken by `take_saved`).
    saved: Option<Vec<String>>,
    /// Ctrl+Enter with one track before the preview was there: apply once it is.
    apply_when_ready: bool,
    /// Images being uploaded for covers, and those done (staged as changes when they are).
    uploading: usize,
    uploaded: Arc<Mutex<Vec<Uploaded>>>,
    /// The file dialog's answer (`cover:file`), and where that cover goes.
    chosen: Arc<Mutex<Vec<std::path::PathBuf>>>,
    chosen_at: Option<usize>,
    /// A source being corrected (one track): its place, and the form.
    correcting: Option<(usize, crate::ui::edit::sources::Form)>,
    /// On a track's page: the names chosen for its keys (extras, notes), the groups (who
    /// sees an extra), files chosen in the dialog (`file:choose` after an extra), what the
    /// line asked the page to do, and its changes saved (for the page to send).
    page_known: Known,
    page_groups: crate::ui::edit::extras::Groups,
    page_chosen: Chosen,
    page_act: Option<tc::Act>,
    page_out: Vec<pc::Staged<tc::Edit>>,
    /// The frame the line alone was last shown in (`show_line`).
    last_pass: u64,
    /// The tools the page around it has, and the one asked for (taken by the page).
    verbs: Vec<Verb>,
    verb: Option<Verb>,
    /// Drafts applied: (the file, the track it became) (taken by `take_imported`).
    imported: Vec<(Uuid, Uuid)>,
    /// Lines to stage for you (`stage_for_you`).
    for_you: Vec<String>,
    /// Metadata being imported (pasted), and what was fetched for it.
    importing: bool,
    fetched: Arc<Mutex<Option<Result<Fetched, String>>>>,
    /// Every artist's names, for `parse:` (made from the catalog it's for, when asked).
    artist_names: Option<(*const Catalog, Arc<crate::title_credits::NameTable>)>,
    /// The row of the check (a staged `parse:`) the keys are on; brought into view.
    check: Option<usize>,
    check_scroll: bool,
    /// The line asked for something for the check (who a name is, a name kept whole).
    prompt: Option<Prompt>,
    /// Staged `parse:` chips corrected, to parse again (once the names they need are there):
    /// (the chip's change as staged, corrected).
    pending_parse: Vec<(Arc<title_parse::TitlesParsed>, title_parse::TitlesParsed)>,
    /// Back to the check from the line (a prompt answered).
    to_check: bool,
}

/// What the line is asked for, for the check.
#[derive(Debug, Clone, PartialEq)]
enum Prompt {
    /// Which artist a name is (the line finds it).
    Who(String),
    /// A name kept whole (in the nth `parse:` chip): the line has the text it was found in.
    Whole(usize, String),
}

/// What importing metadata fetched: what it says, who has the uploader's account, and its
/// artwork.
struct Fetched {
    meta: Metadata,
    owners: Vec<Owner>,
    artwork: Option<Vec<u8>>,
}

/// An artist or grouping with an account (`POST /api/sources/accounts`).
#[derive(serde::Deserialize)]
struct Owner {
    kind: String,
    id: Uuid,
    name: String,
}

impl Default for TrackEditor {
    fn default() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        TrackEditor {
            id,
            cl: CommandLine::new(egui::Id::new(("track-editor-line", id))),
            staged: Vec::new(),
            names: Names::default(),
            lookup: Lookup::default(),
            lookup_tracks: Lookup::default(),
            engine: Engine::default(),
            confirm_discard: false,
            notice: None,
            resolved: None,
            variant: None,
            choose_version: None,
            row: None,
            saved: None,
            apply_when_ready: false,
            uploading: 0,
            uploaded: Arc::default(),
            chosen: Arc::default(),
            chosen_at: None,
            correcting: None,
            page_known: Known::default(),
            page_groups: Default::default(),
            page_chosen: Chosen::new("file"),
            page_act: None,
            page_out: Vec::new(),
            last_pass: 0,
            verbs: Vec::new(),
            verb: None,
            imported: Vec::new(),
            for_you: Vec::new(),
            importing: false,
            fetched: Arc::default(),
            artist_names: None,
            check: None,
            check_scroll: false,
            prompt: None,
            pending_parse: Vec::new(),
            to_check: false,
        }
    }
}

impl TrackEditor {
    /// The cursor goes to the line.
    pub fn opened(&mut self) {
        self.cl.focus = true;
        self.cl.gently = false;
        self.row = None;
    }

    /// The cursor goes to the line unless it's in another field (a search being typed,
    /// whose results changed the track shown): for a track shown, not a key pressed.
    pub fn gently(&mut self) {
        self.cl.gently = self.cl.focus;
    }

    /// Applying, or applied and its outcome not closed yet.
    pub fn applying_or_applied(&self) -> bool {
        self.engine.applied.lock().unwrap().phase != Phase::Editing
    }

    /// Applying, right now.
    pub fn applying(&self) -> bool {
        self.engine.applied.lock().unwrap().phase == Phase::Applying
    }

    /// Changes are staged (Esc is the editor's then: it asks before discarding them).
    pub fn has_changes(&self) -> bool {
        !self.staged.is_empty()
    }

    /// Changes to the track's metadata are staged (not only the page's).
    pub fn has_metadata_changes(&self) -> bool {
        self.staged.iter().any(|s| s.page.is_none())
    }

    /// Applied to every track, and what came of it still shown (until Close). (Some
    /// failed: not this; the changes are kept, to try again.)
    pub fn applied(&self) -> bool {
        let a = self.engine.applied.lock().unwrap();
        a.phase == Phase::Finished && !a.outcomes.values().any(|o| matches!(o, Outcome::Failed(_)))
    }

    /// Done with what was applied (Close): the changes are cleared (not the page's, staged
    /// since).
    pub fn close(&mut self) {
        self.staged.retain(|s| s.page.is_some());
        self.correcting = None;
        self.names = Names::default();
        self.engine.forget();
    }

    /// The keys are on its rows (a page's own keys wait).
    pub fn takes_keys(&self) -> bool {
        self.row.is_some()
    }

    /// Nothing staged and nothing typed.
    pub fn is_blank(&self) -> bool {
        self.staged.is_empty() && self.cl.text.trim().is_empty()
    }

    /// Empties the line if it holds exactly `text` (a prefill nobody touched).
    pub fn clear_line_if(&mut self, text: &str) {
        if self.cl.text == text {
            self.cl.text.clear();
            self.resolved = None;
        }
    }

    /// Types `text` in the line (replacing what's there), with the cursor after it.
    pub fn prefill(&mut self, text: &str) {
        self.cl.text = text.to_string();
        self.cl.highlighted = 0;
        self.row = None;
        self.resolved = None;
        self.cl.focus = true;
        self.cl.gently = false;
        self.cl.place_cursor = true;
    }

    /// The version edited (one track), if another than the shown one was chosen.
    pub fn variant(&self) -> Option<Uuid> {
        self.variant
    }

    /// Chooses the version just made by saving once the page has it (it takes a refresh).
    pub fn choose_new_version(&mut self, variants: &[crate::details::Variant]) {
        let Some(label) = &self.choose_version else { return };
        if let Some(v) = variants.iter().rev().find(|v| &v.label == label) {
            self.variant = Some(v.id);
            self.choose_version = None;
        }
    }

    /// Edits `variant` (one track). Only without staged changes: they're for the version
    /// they were typed for.
    pub fn set_variant(&mut self, variant: Uuid) {
        if !self.has_metadata_changes() {
            self.variant = Some(variant);
        }
    }

    /// The tools the page around it has (`Verb`), found by plain words in the line. Said
    /// before each `show`.
    pub fn offer(&mut self, verbs: &[Verb]) {
        if self.verbs != verbs {
            self.verbs = verbs.to_vec();
        }
    }

    /// The tool asked for in the line, for the page to open (once).
    pub fn take_verb(&mut self) -> Option<Verb> {
        self.verb.take()
    }

    /// On a track's page: what the line asked the page to do (once).
    pub fn take_page_act(&mut self) -> Option<tc::Act> {
        self.page_act.take()
    }

    /// On a track's page: its changes Ctrl+Enter saved, for the page to send (once).
    pub fn take_page_edits(&mut self) -> Vec<pc::Staged<tc::Edit>> {
        std::mem::take(&mut self.page_out)
    }

    /// The page's changes that couldn't be saved: staged again, saying why.
    pub fn restage(&mut self, failed: Vec<pc::Staged<tc::Edit>>) {
        for s in failed {
            self.staged.push(Staged { text: s.text, label: s.label, ops: Vec::new(), warning: s.failed, editable: true, for_you: false, page: Some(s.edit) });
        }
    }

    /// Something else on the page gets the keys (a section, the note editor): the line
    /// doesn't take the cursor back.
    pub fn let_go(&mut self, ctx: &egui::Context) {
        pc::let_go(&mut self.cl, ctx);
    }

    /// The editor on a track's page (edit mode): its line is the page's commandline too
    /// (see the module's notes). Returns true when it's done and asks to close.
    pub fn show_on_page(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, page: &tc::Here) -> bool {
        self.show_with(ui, shell, catalog, &[page.id], None, Some(page))
    }

    /// The line alone, on a track's page in listen mode: the page's keys and actions
    /// (listening, its parts, going elsewhere); the editor's say they're for edit mode. The
    /// changes staged in edit mode are kept, and shown. What it asks the page to do is
    /// taken with `take_page_act`.
    pub fn show_line(&mut self, ui: &mut egui::Ui, shell: &mut Shell, page: &tc::Here) {
        let ctx = ui.ctx().clone();
        pc::arrive(&mut self.cl, &ctx, shell, &mut self.last_pass);
        let focused = self.cl.hold_focus(&ctx);
        let line = parse::parse(&self.cl.text);
        let words = line.parts.is_empty() && !line.words.is_empty();
        let rows: Vec<tc::Row> = match line.parts.last() {
            _ if words => tc::actions(&line.words.join(" "), page),
            Some(last) if tc::is_page_part(last, line.last_goes_with()) => tc::choices(last, line.last_goes_with(), page, &self.page_known),
            _ => Vec::new(),
        };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));
        let mut take: Option<(usize, bool)> = None;
        let mut enter = false;
        if focused {
            let pressed = self.cl.keys(&ctx, rows.len(), false, !self.staged.is_empty());
            if (pressed.tab || pressed.enter) && !rows.is_empty() {
                take = Some((self.cl.highlighted, pressed.tab));
            }
            enter = pressed.enter;
            if pressed.escape {
                self.notice = None;
            }
            // Backspace on an empty line: the last change back, to edit.
            if pressed.back
                && let Some(last) = self.staged.pop()
                && last.editable
            {
                self.cl.text = last.text;
                self.cl.cursor_to_end(&ctx);
            }
        }
        let hint = "/  play  queue  star  rating:  lyrics  file  history  (a word finds the rest · | between commands · Alt+E: edit mode)";
        if self.cl.field(ui, true, hint).changed() {
            self.notice = None;
        }
        let mut clicked = None;
        if !rows.is_empty() {
            let list = rows.iter().map(|r| (r.label.clone(), r.sub.clone())).collect();
            cl::list(ui, list, self.cl.highlighted, &mut clicked, "↑/↓ choose · Tab: take it · Enter: take it and do it");
        }
        if let Some(n) = clicked {
            take = Some((n, true));
        }
        if let Some((n, tab)) = take
            && let Some(row) = rows.get(n)
            && !self.take_page_row(row, &line, tab, &ctx)
        {
            enter = false;
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = tc::resolve_line(&self.cl.text, page, &self.page_known);
            match done.iter().find_map(|d| d.as_ref().err()) {
                Some(err) => self.notice = Some(format!("Not done: {err}")),
                None => {
                    for (_, does) in done.into_iter().flatten() {
                        if let tc::Does::Act(act) = does {
                            self.page_act = Some(act);
                        }
                    }
                    self.cl.text.clear();
                    self.notice = None;
                }
            }
        }
        // What Enter would do, when no list is in the way.
        if rows.is_empty() && !self.cl.text.trim().is_empty() {
            let shown: Vec<Result<(String, Option<String>), String>> = tc::resolve_line(&self.cl.text, page, &self.page_known).into_iter().map(|r| r.map(|(label, _)| (label, None))).collect();
            cl::would_do(ui, &shown, "Nothing to do by that name");
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted).small());
        }
        // Changes staged in edit mode: kept, and saved there.
        if !self.staged.is_empty() {
            shell.form_dirty = true;
            ui.add_space(4.0);
            let chips: Vec<cl::Chip> = self.staged.iter().map(|s| cl::Chip { text: &s.text, label: &s.label, warning: s.warning.as_deref() }).collect();
            if let Some(n) = cl::chips(ui, &chips, true) {
                self.staged.remove(n);
            }
            ui.label(RichText::new("Not saved: they're saved in edit mode (e, then Ctrl+Enter)").color(P.faint).small());
        }
    }

    /// A row of the page's taken (Tab, Enter or a click; `tab`: the next part written after
    /// it). Returns whether Enter goes on to do the line.
    fn take_page_row(&mut self, row: &tc::Row, line: &parse::Line, tab: bool, ctx: &egui::Context) -> bool {
        match pc::take(&mut self.cl, ctx, &mut self.page_known, line, &row.choice, tab) {
            Taken::Filled => !tab,
            Taken::Inserted => false,
            Taken::Act(act) => {
                self.page_act = Some(act);
                false
            }
            Taken::Stage(s) => {
                // (Found by words: they're done with.)
                if line.parts.is_empty() {
                    self.cl.text.clear();
                }
                self.staged.push(Staged { text: s.text, label: s.label, ops: Vec::new(), warning: None, editable: true, for_you: false, page: Some(s.edit) });
                false
            }
        }
    }

    /// The units of the page's commands in `text` (one per command, as `resolve` made them),
    /// as the page says (not kept with the rest: the page changes).
    fn page_units(&self, text: &str, units: &mut [Unit], page: Option<&tc::Here>) {
        let Some(page) = page else { return };
        for (unit, command) in units.iter_mut().zip(parse::parse(text).commands()) {
            if matches!(command[0].key, K::Page(_)) {
                unit.result = tc::resolve(command, page, &self.page_known).map(|(label, does)| resolve::Resolved { label, page: Some(does), ..Default::default() });
            }
        }
    }

    /// Edits a draft (see the module's notes), one of the targets: `track` as the preview
    /// made it. Again with a newer preview of the same file: the changes staged stay.
    pub fn draft(&mut self, of: DraftOf, track: crate::details::TrackDetail) {
        self.engine.set_draft(of, track);
    }

    /// Stages `single:` if nothing is staged yet: for drafts on no album (a loose track
    /// becomes a single of its own, instead of one more on "No album").
    pub fn start_single(&mut self) {
        if self.staged.is_empty() {
            let label = "a single of its own, for each (named after it as it'll be; its primary artists, date and covers)".to_string();
            self.staged.push(Staged { text: "single:".into(), label, ops: vec![Op::Single], warning: None, editable: true, for_you: false, page: None });
        }
    }

    /// Changes staged for you, each as if typed and staged (a chip of its own, taken back
    /// like any), the tracks and artists they name under these names being these. Those
    /// staged for you before are taken back first. Staged when it's shown next; one that
    /// can't be is left in the line, saying why.
    pub fn stage_for_you(&mut self, lines: Vec<String>, tracks: &[(String, Uuid)], artists: &[(String, Uuid)]) {
        for (name, id) in tracks {
            self.names.pin(PinKind::Track, name, *id);
        }
        for (name, id) in artists {
            self.names.pin(PinKind::Artist, name, *id);
        }
        self.staged.retain(|s| !s.for_you);
        self.for_you = lines;
    }

    /// The files whose drafts don't fit their import anymore (the library changed since
    /// their preview): once. The page previews them again (`draft`), the changes staged
    /// staying.
    pub fn take_stale_drafts(&mut self) -> Vec<Uuid> {
        let stale: Vec<Uuid> = self.engine.stale_drafts.lock().unwrap().drain().collect();
        if !stale.is_empty() && self.engine.applied.lock().unwrap().phase == Phase::Editing {
            self.engine.forget();
        }
        stale
    }

    /// Drafts imported: (the file, the track it became), once.
    pub fn take_imported(&mut self) -> Vec<(Uuid, Uuid)> {
        std::mem::take(&mut self.imported)
    }

    /// The flags after a one-track edit just applied (once).
    pub fn take_saved(&mut self) -> Option<Vec<String>> {
        self.saved.take()
    }

    /// One track's title as it is (loaded by the editor), once it's there.
    pub fn title(&self, track: Uuid) -> Option<String> {
        self.engine.ready(track).map(|r| r.base.meta.title.clone())
    }

    /// One track's credits as the staged changes leave them.
    pub fn credits_after(&self, track: Uuid) -> Option<Vec<model::Credit>> {
        self.engine.ready(track).map(|r| r.meta.credits.clone())
    }

    /// Stages crediting `to` wherever `from` is credited now (same role, same place): an
    /// ambiguous name settled on another of its artists.
    pub fn replace_artist(&mut self, catalog: &Catalog, track: Uuid, from: Uuid, to: Uuid) {
        let Some(credits) = self.credits_after(track) else { return };
        let mut ops = Vec::new();
        for (at, c) in credits.iter().enumerate().filter(|(_, c)| c.artist_id == from) {
            ops.push(Op::Uncredit { artist: from, role: Some(c.role.clone()) });
            ops.push(Op::Credit { artist: to, role: Some(c.role.clone()), alias: Some(None), at: Some(at) });
        }
        if ops.is_empty() {
            return;
        }
        let label = |id: Uuid| catalog.artist_label(id).unwrap_or_else(|| "(an artist)".into());
        let text = format!("artist: {} → {}", label(from), label(to));
        self.staged.push(Staged { label: text.clone(), text, ops, warning: None, editable: false, for_you: false, page: None });
    }

    fn ops(&self) -> Vec<Op> {
        self.staged.iter().flat_map(|s| s.ops.clone()).collect()
    }

    /// The staged `parse:` chips' changes, in the order staged (the check is theirs).
    fn staged_parses(&self) -> Vec<Arc<title_parse::TitlesParsed>> {
        self.staged.iter().flat_map(|s| s.ops.iter()).filter_map(|op| if let Op::ParseTitle(p) = op { Some(p.clone()) } else { None }).collect()
    }

    /// The nth staged `parse:` corrected by `change`, then parsed again (`settle_parse`).
    fn reparse(&mut self, k: usize, change: impl FnOnce(&mut title_parse::TitlesParsed)) {
        let Some(from) = self.staged_parses().get(k).cloned() else { return };
        match self.pending_parse.iter_mut().find(|(f, _)| Arc::ptr_eq(f, &from)) {
            Some((_, p)) => change(p),
            None => {
                let mut p = (*from).clone();
                change(&mut p);
                self.pending_parse.push((from, p));
            }
        }
        self.apply_when_ready = false;
    }

    /// Every staged `parse:` parsed again (a name chosen: it's in any of them), corrected
    /// by `change` first.
    fn reparse_all(&mut self, change: impl Fn(&mut title_parse::TitlesParsed)) {
        for k in 0..self.staged_parses().len() {
            self.reparse(k, &change);
        }
    }

    /// Corrected `parse:` chips parsed again and staged in their place, once the names they
    /// need are looked up (tried again each frame until then).
    fn settle_parse(&mut self, shell: &mut Shell, here: &Here) {
        let Some(table) = here.artist_names else { return };
        for (from, pending) in std::mem::take(&mut self.pending_parse) {
            let mut looking = Vec::new();
            let again = {
                let ctx = here.ctx(&self.names);
                let mut look = Look(&mut *shell);
                pending.again(table, &mut |name| Some(resolve::credited(name, &ctx, &mut look, &mut looking)))
            };
            let Some(mut again) = again.filter(|_| looking.is_empty()) else {
                self.pending_parse.push((from, pending));
                shell.ctx.request_repaint_after(std::time::Duration::from_millis(100));
                continue;
            };
            again.make_new(&mut |name| self.names.new_artist(name));
            let (label, warning) = resolve::parse_label(&again);
            let again = Arc::new(again);
            // The chip it came from (taken back meanwhile: nothing to do).
            for s in &mut self.staged {
                let mut here = false;
                for op in &mut s.ops {
                    if let Op::ParseTitle(p) = op
                        && Arc::ptr_eq(p, &from)
                    {
                        *p = again.clone();
                        here = true;
                    }
                }
                if here {
                    (s.label, s.warning) = (label.clone(), warning.clone());
                }
            }
            self.resolved = None;
        }
    }

    /// A key on the check's row `n`.
    fn check_key(&mut self, entry: &check::Entry, key: Key, here: &Here) {
        use check::{Entry, Fix};
        let parses = self.staged_parses();
        let name_of = |k: usize, i: usize| parses.get(k).and_then(|p| p.names.get(i));
        if let (Entry::Name(k, i), Key::Enter | Key::C) = (entry, key)
            && let Some(n) = name_of(*k, *i).filter(|n| n.credited == title_parse::Credited::Own)
        {
            self.notice = Some(format!("“{}” stands for each track's own primary artists (n: it isn't a credit).", n.text));
            return;
        }
        match (entry, key) {
            (Entry::Name(k, i), Key::Enter) => {
                let Some(name) = name_of(*k, *i).map(|n| n.text.clone()) else { return };
                self.cl.text = format!("artist:{}", parse::quoted(&name));
                self.prompt = Some(Prompt::Who(name));
                self.cl.focus = true;
                self.cl.place_cursor = true;
            }
            (Entry::Name(k, i), Key::C) => {
                let Some(name) = name_of(*k, *i).map(|n| n.text.clone()) else { return };
                let candidates = here.artist_names.map(|t| t.get(&name).to_vec()).unwrap_or_default();
                if candidates.len() < 2 {
                    self.notice = Some(format!("Only {} artist has the name “{name}” (Enter chooses another).", if candidates.is_empty() { "no" } else { "one" }));
                    return;
                }
                let now = self.names.pinned(PinKind::Artist, &name).and_then(|id| candidates.iter().position(|c| *c == id));
                let next = candidates[now.map_or(0, |at| (at + 1) % candidates.len())];
                self.names.pin(PinKind::Artist, &name, next);
                self.reparse_all(|_| {});
            }
            (Entry::Name(k, i), Key::S) => {
                let Some(name) = name_of(*k, *i).map(|n| n.text.clone()) else { return };
                let pieces = crate::title_credits::split_by_hand(&name, &parses[*k].rules);
                if pieces.len() < 2 {
                    self.notice = Some(format!("“{name}” has no separator to split on (, & + x and)."));
                    return;
                }
                self.notice = Some(format!("“{name}” split: {}.", pieces.join(", ")));
                self.reparse(*k, |p| p.fixes.split.push(name));
            }
            (Entry::Track(..), Key::S | Key::W | Key::N | Key::C) => {
                self.notice = Some("On a track, x leaves it out; s, w, n and c are for a name's row (above).".into());
            }
            (Entry::Name(k, i), Key::W) => {
                let Some(n) = name_of(*k, *i) else { return };
                self.cl.text = n.part.clone();
                self.prompt = Some(Prompt::Whole(*k, n.text.clone()));
                self.cl.focus = true;
                self.cl.place_cursor = true;
            }
            (Entry::Name(k, i), Key::N) => {
                let Some(name) = name_of(*k, *i).map(|n| n.text.clone()) else { return };
                self.reparse(*k, |p| p.fixes.not_names.push(name));
            }
            (Entry::Track(track, _), Key::X) => {
                // Out of every chip that has it (or back into all of them).
                let (track, out) = (*track, check::left_out(&parses, *track));
                self.reparse_all(|p| {
                    if !p.titles.iter().any(|(t, _)| *t == track) {
                        return;
                    }
                    p.skip.retain(|t| *t != track);
                    if !out {
                        p.skip.push(track);
                    }
                });
            }
            (Entry::Fixed(k, fix, name), Key::Delete) => {
                let (fix, name) = (*fix, name.clone());
                self.reparse(*k, |p| {
                    let list = match fix {
                        Fix::NotName => &mut p.fixes.not_names,
                        Fix::Split => &mut p.fixes.split,
                        Fix::Whole => &mut p.fixes.whole,
                    };
                    list.retain(|n| *n != name);
                });
            }
            (Entry::Chosen(name, _), Key::Delete) => {
                self.names.unpin(PinKind::Artist, name);
                self.reparse_all(|_| {});
            }
            _ => {}
        }
    }

    /// The line's answer to the check's prompt (Enter).
    fn answer_prompt(&mut self, prompt: Prompt) {
        match prompt {
            Prompt::Who(name) => {
                self.notice = Some(format!("Choose who “{name}” is in the list (Esc: never mind)."));
                return;
            }
            Prompt::Whole(k, _) => {
                let whole = self.cl.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if whole.is_empty() {
                    self.notice = Some("Leave the name to keep whole in the commandline (Esc: never mind).".into());
                    return;
                }
                self.reparse(k, |p| p.fixes.whole.push(whole));
            }
        }
        self.cl.text.clear();
        self.prompt = None;
        self.to_check = true;
    }

    /// The editor for `targets` (in the order shown); `album`: the list is this album's.
    /// With one track it's laid out top to bottom (the page around it scrolls); with many,
    /// in two columns filling the room given. Returns true when it's done and asks to close.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, targets: &[Uuid], album: Option<Uuid>) -> bool {
        self.show_with(ui, shell, catalog, targets, album, None)
    }

    /// `show`, with the track page around it (`page`), if it's there.
    fn show_with(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, targets: &[Uuid], album: Option<Uuid>, page: Option<&tc::Here>) -> bool {
        let ctx = ui.ctx().clone();
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let single = targets.len() == 1;
        let variant = if single { self.variant } else { None };
        let change = Change::of(self.ops(), &self.names);
        self.engine.update(shell, catalog, targets, &change, &self.names, variant);
        self.finished(shell, targets);
        let phase = self.engine.applied.lock().unwrap().phase;
        let line_id = self.cl.id();
        if !self.staged.is_empty() || self.uploading > 0 {
            shell.form_dirty = true;
        }
        if phase == Phase::Editing {
            self.images(&ctx, shell, targets.len());
        }

        // Ctrl+Enter with one track came before its preview: applied once it's there.
        if self.apply_when_ready && phase == Phase::Editing && self.engine.preview_current() && !self.loading() {
            self.apply_when_ready = false;
            if self.refused() > 0 {
                self.notice = Some("Not applied: the preview says it can't be (in red).".into());
            } else if self.changing() > 0 {
                self.engine.apply(shell, &change, variant);
            }
        }

        // On a track's page: files chosen for an extra put in the line; the groups asked for
        // once `visibility:` is typed.
        if let Some(page) = page {
            self.page_chosen.put_in_line(&mut self.cl);
            tc::ask_groups(&self.cl.text, page, shell, &mut self.page_known, &self.page_groups);
        }
        // The cursor to the line: asked until it holds.
        let focused = self.cl.hold_focus(&ctx);
        if focused {
            self.row = None;
            if self.prompt.is_none() {
                self.check = None;
            }
        }
        let typing = ctx.egui_wants_keyboard_input();
        let free = keys_free(&ctx, ui.layer_id(), shell.palette_open, shell.modal_was_open);
        // Metadata pasted to import (into the empty line, or outside any field).
        if phase == Phase::Editing && single && !shell.palette_open && !shell.modal_open && ((focused && self.cl.text.trim().is_empty()) || !typing) {
            let pasted = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(text) = e { import::recognize(text) } else { None }));
            if let Some(pasted) = pasted {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
                self.fetch(shell, pasted);
            }
        }
        let fetched = self.fetched.lock().unwrap().take();
        if let Some(result) = fetched {
            self.importing = false;
            match result {
                Ok(fetched) => self.stage_imported(fetched, shell, catalog),
                Err(err) => self.notice = Some(format!("Couldn't import it: {err}")),
            }
        }
        // `/` (the app's key) puts the cursor here; outside a field, the rows keep it from
        // the app, so it's taken here too.
        shell.slash_target = Some(line_id);
        if !typing && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "/"))) {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/")));
            self.cl.focus = true;
            self.row = None;
        }

        // ---- What the line means, and its list ----
        let staged_album = change.ops.iter().rev().find_map(|op| if let Op::Album { album, .. } = op { Some(*album) } else { None });
        let may_relate = shell.account.can("edit_relations");
        let may_source = shell.account.can("edit_sources");
        // `parse:` typed: each target's title as the staged changes leave it (once they're
        // all loaded), and every artist's names.
        let typed_parse = parse::parse(&self.cl.text).parts.iter().any(|p| p.key == K::Parse);
        let parsing = typed_parse || !self.staged_parses().is_empty();
        let titles: Option<Vec<(Uuid, String)>> = typed_parse.then(|| targets.iter().map(|t| self.engine.ready(*t).map(|r| (*t, r.meta.title.clone()))).collect()).flatten();
        if parsing && self.artist_names.as_ref().is_none_or(|(of, _)| !std::ptr::eq(*of, Arc::as_ptr(catalog))) {
            let names = catalog.artists.iter().flat_map(|a| std::iter::once((a.id, a.name.as_str())).chain(a.aliases.iter().map(move |x| (a.id, x.as_str()))));
            self.artist_names = Some((Arc::as_ptr(catalog), Arc::new(crate::title_credits::NameTable::new(names))));
        }
        let artist_names = self.artist_names.as_ref().filter(|_| parsing).map(|(_, t)| t.clone());
        let here = Here { catalog, vocab: &vocab, targets: targets.len(), album, staged_album, may_relate, may_source, titles: titles.as_deref(), artist_names: artist_names.as_deref(), page };
        self.settle_parse(shell, &here);
        // Staged for you (`stage_for_you`); what can't be is left in the line.
        let mut left = Vec::new();
        for text in std::mem::take(&mut self.for_you) {
            let before = self.staged.len();
            if self.stage(&text, shell, &here) {
                self.staged[before..].iter_mut().for_each(|s| s.for_you = true);
            } else {
                left.push(text);
            }
        }
        if !left.is_empty() {
            self.cl.text = left.join(" | ");
            self.cl.focus = true;
            self.cl.place_cursor = true;
        }
        let parsed = parse::parse(&self.cl.text);
        let last = parsed.parts.last().cloned();
        // What the last part goes with: the part before its modifiers, and those (none: it
        // isn't a modifier).
        let group: &[parse::Part] = parsed.last_goes_with();
        // Tab or Enter takes what's listed: the matches must be for exactly what's typed
        // (found in the background, they can still be the last keystroke's), so then
        // they're found now.
        let taking = focused && ctx.input(|i| i.key_pressed(Key::Tab) || i.key_pressed(Key::Enter));
        // (A cover's: albums here, tracks below; what's typed without its `from:`.)
        let query = |l: &parse::Part| if l.key == K::Cover { resolve::cover_value(l).unwrap_or_default() } else { l.value.clone() };
        let hits = match last.as_ref().and_then(|l| resolve::search_for(l).map(|sp| (sp, query(l)))) {
            Some((sp, value)) if taking => self.lookup.now(catalog, sp, &value),
            Some((sp, value)) => self.lookup.hits(shell, sp, &value),
            None => Vec::new(),
        };
        let track_hits = match last.as_ref().and_then(resolve::cover_value) {
            Some(value) if taking => self.lookup_tracks.now(catalog, SearchPart::Tracks, &value),
            Some(value) => self.lookup_tracks.hits(shell, SearchPart::Tracks, &value),
            None => Vec::new(),
        };
        // (A name kept whole is typed as plain words: they find nothing then.)
        let words = parsed.parts.is_empty() && !parsed.words.is_empty() && !matches!(self.prompt, Some(Prompt::Whole(..)));
        let (candidates, actions, units) = {
            let mut look = Look(&mut *shell);
            let rctx = here.ctx(&self.names);
            let candidates: Vec<Candidate> = match &last {
                Some(l) if !words => resolve::candidates(l, group, &rctx, &hits, &track_hits, &mut look),
                _ => Vec::new(),
            };
            let actions = if words { resolve::actions(&parsed.words.join(" "), &rctx) } else { Vec::new() };
            let typed = parsed.words.join(" ");
            let verbs: Vec<Verb> = if words { self.verbs.iter().copied().filter(|v| crate::catalog::query::loosely_in(&typed, v.label())).collect() } else { Vec::new() };
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (&self.cl.text, self.names.version, staged_album, look.0.generation, targets.len(), &titles).hash(&mut h);
                h.finish()
            };
            let mut units = match &self.resolved {
                Some((k, units)) if *k == key => units.clone(),
                _ => {
                    let (units, waiting) = resolve::resolve(&self.cl.text, &rctx, &mut look);
                    self.resolved = if waiting { None } else { Some((key, units.clone())) };
                    units
                }
            };
            self.page_units(&self.cl.text, &mut units, page);
            (candidates, (verbs, actions), units)
        };
        let (verbs, actions) = actions;
        // The page's: its actions (words), or the choices for one of its keys being typed (in
        // place of the editor's).
        let page_part = page.is_some() && !words && last.as_ref().is_some_and(|l| tc::is_page_part(l, group));
        let page_rows: Vec<tc::Row> = match page {
            Some(p) if words => tc::actions(&parsed.words.join(" "), p),
            Some(p) if page_part => last.as_ref().map(|l| tc::choices(l, group, p, &self.page_known)).unwrap_or_default(),
            _ => Vec::new(),
        };
        let candidates = if page_part { Vec::new() } else { candidates };
        let choices = if words { page_rows.len() + verbs.len() + actions.len() } else if page_part { page_rows.len() } else { candidates.len() };
        self.cl.highlighted = self.cl.highlighted.min(choices.saturating_sub(1));

        // One track's rows (and the flags on them).
        let rows: Vec<Row> = match (single, targets.first().and_then(|t| self.engine.ready(*t))) {
            (true, Some(r)) => {
                let track = targets[0];
                let done = &shell.flags.done;
                let now: Vec<String> = match self.engine.drafts.get(&track) {
                    Some(d) => d.flags.clone(),
                    None => catalog.issues.get(&track).map(|f| f.iter().filter(|(id, _)| !done.contains(id)).map(|(_, k)| k.clone()).collect()).unwrap_or_default(),
                };
                let after = self.after_flags(track);
                // The names credits are under: the track's own, and those of artists chosen here.
                let mut aliases: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
                if let Some(d) = self.engine.detail(track) {
                    aliases.extend(d.credits.iter().filter_map(|c| Some((c.alias_id?, c.alias.clone()?))));
                    aliases.extend(d.alias_names.iter().map(|(k, v)| (*k, v.clone())));
                }
                let unnamed: Vec<Uuid> = r.meta.credits.iter().filter(|c| c.alias_id.is_some_and(|a| !aliases.contains_key(&a))).map(|c| c.artist_id).collect();
                for artist in unnamed {
                    if let Some(a) = shell.artist_details.get(artist).value.as_ref() {
                        aliases.extend(a.aliases.iter().map(|x| (x.id, x.name.clone())));
                    }
                }
                let ambiguous: Vec<(Uuid, Vec<Uuid>)> = shell
                    .flags
                    .list
                    .iter()
                    .filter(|f| f.track_id == track && f.kind == "ambiguous_artist" && !done.contains(&f.id))
                    .map(|f| (f.id, f.candidates().into_iter().map(|c| c.0).collect()))
                    .collect();
                let names = self.names.stand_ins();
                let covers = self.engine.detail(track).map(|d| d.covers.clone()).unwrap_or_default();
                let know = panel::Know { catalog, names: &names, aliases: &aliases, ambiguous: &ambiguous, covers: &covers };
                panel::rows(r, &know, &now, after.as_deref())
            }
            _ => Vec::new(),
        };
        if let Some(row) = self.row {
            self.row = (!rows.is_empty()).then(|| row.min(rows.len() - 1));
        }
        // The check of a staged `parse:`, and the row of it the keys are on.
        let parses = self.staged_parses();
        let checking: Option<(Vec<Arc<title_parse::TitlesParsed>>, Vec<check::Entry>)> = (!parses.is_empty()).then(|| {
            let entries = check::entries(&parses, &self.names, &self.engine);
            (parses, entries)
        });
        self.check = match (&checking, self.check) {
            (Some((_, e)), Some(n)) => check::selectable(e, n.min(e.len().saturating_sub(1)), false).or_else(|| check::selectable(e, 0, true)),
            _ => None,
        };
        let first_check = checking.as_ref().and_then(|(_, e)| check::selectable(e, 0, true));
        // On the rows or the check, the keys are theirs (Esc too: back to the line, not off
        // the page).
        if self.row.is_some() || self.check.is_some() {
            shell.page_takes_keys = true;
            shell.page_takes_back = true;
        }

        // ---- Keys in the line ----
        let mut take: Option<(usize, bool)> = None;
        let mut stage = false;
        if focused {
            let down_too = !rows.is_empty() || first_check.is_some();
            let cl::Pressed { down, tab, enter, back, escape, .. } = self.cl.keys(&ctx, choices, down_too, !self.staged.is_empty());
            if down && choices == 0 {
                if !rows.is_empty() {
                    // Into the rows.
                    self.row = Some(0);
                    ctx.memory_mut(|m| m.surrender_focus(line_id));
                } else {
                    self.check = first_check;
                    self.check_scroll = true;
                    ctx.memory_mut(|m| m.surrender_focus(line_id));
                }
            }
            if tab && choices > 0 {
                take = Some((self.cl.highlighted, true));
            }
            if enter {
                if choices > 0 {
                    take = Some((self.cl.highlighted, false));
                }
                stage = !words;
            }
            if back
                && let Some(last) = self.staged.pop()
            {
                self.apply_when_ready = false;
                if last.editable {
                    self.cl.text = last.text;
                    self.cl.cursor_to_end(&ctx);
                }
            }
            // (Esc emptied the line.) A prompt let go: back to the check.
            if escape && self.prompt.take().is_some() {
                self.to_check = true;
            }
        }
        // ---- Keys on a row ----
        if let Some(n) = self.row.filter(|_| !typing && free)
            && let Some(row) = rows.get(n)
            && let Some(r) = targets.first().and_then(|t| self.engine.ready(*t)).cloned()
        {
            let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
            // Alt first: a key asked for without Alt matches with it too.
            let earlier = pressed(Key::ArrowUp, Modifiers::ALT) || pressed(Key::K, Modifiers::ALT);
            let later = !earlier && (pressed(Key::ArrowDown, Modifiers::ALT) || pressed(Key::J, Modifiers::ALT));
            if earlier || later {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                self.row_key(row.kind, if earlier { RowKey::Earlier } else { RowKey::Later }, &r, shell, &here);
                // The cursor goes with the artist or cover moved (they're rows next to each other).
                let (at, len) = match row.kind {
                    RowKind::Credit(at) => (Some(at), r.meta.credits.len()),
                    RowKind::Cover(at) => (Some(at), r.attached.covers.len()),
                    _ => (None, 0),
                };
                if let Some(at) = at {
                    if earlier && at > 0 {
                        self.row = Some(n - 1);
                    } else if later && at + 1 < len {
                        self.row = Some(n + 1);
                    }
                }
            } else if pressed(Key::ArrowDown, Modifiers::NONE) || pressed(Key::J, Modifiers::NONE) {
                // Past the last row: the check, if there's one.
                if n + 1 >= rows.len() && first_check.is_some() {
                    self.row = None;
                    self.check = first_check;
                    self.check_scroll = true;
                } else {
                    self.row = Some((n + 1).min(rows.len() - 1));
                }
            } else if pressed(Key::ArrowUp, Modifiers::NONE) || pressed(Key::K, Modifiers::NONE) {
                if n == 0 {
                    self.row = None;
                    self.cl.focus = true;
                } else {
                    self.row = Some(n - 1);
                }
            } else if pressed(Key::Escape, Modifiers::NONE) {
                self.row = None;
                self.cl.focus = true;
            } else {
                let action = if pressed(Key::Enter, Modifiers::NONE) {
                    Some(RowKey::Edit)
                } else if pressed(Key::R, Modifiers::NONE) {
                    Some(RowKey::Role)
                } else if pressed(Key::A, Modifiers::NONE) {
                    Some(RowKey::Alias)
                } else if pressed(Key::Delete, Modifiers::NONE) {
                    Some(RowKey::Remove)
                } else if pressed(Key::C, Modifiers::NONE) {
                    Some(RowKey::Next)
                } else if pressed(Key::M, Modifiers::NONE) {
                    Some(RowKey::Main)
                } else if pressed(Key::O, Modifiers::NONE) {
                    Some(RowKey::Origin)
                } else if pressed(Key::Space, Modifiers::NONE) {
                    Some(RowKey::View)
                } else {
                    None
                };
                if let Some(action) = action {
                    // The letter typed is still in this frame's input: not into the line.
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                    if action == RowKey::Next {
                        // An ambiguous name: its next candidate credited instead.
                        if let (Some((_, candidates)), Some(from)) = (&row.choose, row.artist)
                            && let Some(at) = candidates.iter().position(|c| *c == from)
                        {
                            let to = candidates[(at + 1) % candidates.len()];
                            self.replace_artist(catalog, targets[0], from, to);
                        }
                    } else {
                        self.row_key(row.kind, action, &r, shell, &here);
                    }
                }
            }
        }
        // ---- Keys on the check ----
        if let (Some(n), Some((_, entries))) = (self.check.filter(|_| !typing && free), &checking) {
            let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
            if pressed(Key::ArrowDown) || pressed(Key::J) {
                self.check = check::selectable(entries, n + 1, true).or(Some(n));
                self.check_scroll = true;
            } else if pressed(Key::ArrowUp) || pressed(Key::K) {
                match check::selectable(entries, n.saturating_sub(1), false).filter(|&m| m < n) {
                    Some(m) => self.check = Some(m),
                    None => {
                        self.check = None;
                        if rows.is_empty() {
                            self.cl.focus = true;
                        } else {
                            self.row = Some(rows.len() - 1);
                        }
                    }
                }
                self.check_scroll = true;
            } else if pressed(Key::Escape) {
                self.check = None;
                self.cl.focus = true;
            } else if let Some(key) = [Key::Enter, Key::C, Key::S, Key::W, Key::N, Key::X, Key::Delete].into_iter().find(|k| pressed(*k)) {
                // The letter typed is still in this frame's input: not into the line.
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                self.check_key(&entries[n], key, &here);
            }
        }
        // Ctrl+Enter: what's typed is staged first; then the preview is applied.
        let ctrl_enter = free && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        if ctrl_enter && !self.cl.text.trim().is_empty() {
            stage = true;
        }
        // Esc outside the line, with changes staged: asks, and again discards them. Once
        // they're applied: Close.
        let mut esc_closed = false;
        if !typing && self.row.is_none() && !self.staged.is_empty() && phase != Phase::Applying && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if phase == Phase::Finished && (self.applied() || self.confirm_discard) {
                self.close();
                self.confirm_discard = false;
                esc_closed = true;
            } else if self.confirm_discard {
                self.staged.clear();
                self.confirm_discard = false;
            } else {
                self.confirm_discard = true;
            }
        }

        // ---- Header ----
        if !single {
            ui.horizontal(|ui| {
                let loaded = self.engine.loaded_count(targets);
                let mut parts = vec![format!("{} tracks", thousands(targets.len()))];
                if loaded < targets.len() {
                    parts.push(format!("loading {} of {}", thousands(loaded), thousands(targets.len())));
                } else if !self.staged.is_empty() {
                    parts.push(format!("{} change", thousands(self.changing())));
                }
                ui.label(RichText::new(parts.join(" · ")).color(P.faint).small());
            });
        }

        // ---- The line ----
        let editing = phase == Phase::Editing;
        let hint = if page.is_some() {
            "/  title:  artist:  album:  cover:paste  note:  play  lyrics  (a word finds the rest · | between commands · Ctrl+Enter applies · paste a SoundCloud/yt-dlp JSON or a YouTube link: its metadata)"
        } else {
            "What to change: artist:…  album:…  status:demo | date:2020  cover:paste  source:<link>  (| between changes · Tab: the next part · Enter: stage it · paste a SoundCloud/yt-dlp JSON or a YouTube link: its metadata)"
        };
        let response = self.cl.field(ui, editing, hint);
        if response.changed() {
            self.notice = None;
            self.confirm_discard = false;
        }

        // A choice taken (Tab, Enter or a click).
        let mut clicked = None;
        if words && (!page_rows.is_empty() || !actions.is_empty() || !verbs.is_empty()) {
            // The page's and the tools first: a word that names one means it.
            let rows = page_rows
                .iter()
                .map(|r| (r.label.clone(), r.sub.clone()))
                .chain(verbs.iter().map(|v| (v.label().to_string(), "opens it".to_string())))
                .chain(actions.iter().map(|(label, insert)| (label.clone(), insert.clone())))
                .collect();
            cl::list(ui, rows, self.cl.highlighted, &mut clicked, LIST_HELP);
        } else if page_part && !page_rows.is_empty() {
            cl::list(ui, page_rows.iter().map(|r| (r.label.clone(), r.sub.clone())).collect(), self.cl.highlighted, &mut clicked, LIST_HELP);
        } else if !candidates.is_empty() {
            cl::list(ui, candidates.iter().map(|c| (c.label.clone(), c.sub.clone())).collect(), self.cl.highlighted, &mut clicked, LIST_HELP);
        }
        if let Some(n) = clicked {
            take = Some((n, true));
        }
        if let Some((n, next)) = take {
            if words {
                // (The page's rows, then the tools, then the editor's.)
                let tool = n.checked_sub(page_rows.len());
                let action = tool.and_then(|t| t.checked_sub(verbs.len()));
                if let Some(row) = page_rows.get(n) {
                    self.take_page_row(row, &parsed, next, &ctx);
                    stage = false;
                } else if let Some(verb) = tool.and_then(|t| verbs.get(t)) {
                    self.verb = Some(*verb);
                    self.cl.text.clear();
                    stage = false;
                } else if let Some((_, insert)) = action.and_then(|a| actions.get(a)) {
                    self.cl.text = insert.clone();
                    self.cl.cursor_to_end(&ctx);
                    stage = false;
                }
            } else if page_part {
                if let Some(row) = page_rows.get(n)
                    && !self.take_page_row(row, &parsed, next, &ctx)
                {
                    stage = false;
                }
            } else if let (Some(c), Some(l)) = (candidates.get(n), &last) {
                self.take(c, l, next, &ctx);
                if next {
                    stage = false;
                }
            }
            self.cl.highlighted = 0;
            // (A tool asked for gets the cursor, not the line.)
            if self.verb.is_none() {
                self.cl.focus = true;
            } else {
                ctx.memory_mut(|m| m.surrender_focus(line_id));
            }
        }
        // On a track's page, `file:paste` and `file:choose` after an extra: the paths in the
        // line first (what's staged is what the line says).
        if stage && page.is_some() && self.prompt.is_none() {
            let dialog = pc::Dialog { title: "Files for the extra", kinds: ("Any file", &[]), many: true };
            if let Err(err) = pc::expand_files::<K>(&mut self.cl.text, shell, |p| p.key == K::Page(PageKey::File) && p.arg, true, dialog, &self.page_chosen) {
                self.notice = Some(err);
                stage = false;
            }
        }
        let mut not_staged = false;
        if stage && let Some(prompt) = self.prompt.clone() {
            self.answer_prompt(prompt);
        } else if stage {
            let line = std::mem::take(&mut self.cl.text);
            if !self.stage(&line, shell, &here) {
                self.cl.text = line;
                not_staged = true;
            }
        }
        // Ctrl+Enter on a track's page: its changes go to the page, to be sent (what was
        // typed staged first; if it couldn't be, nothing is).
        if ctrl_enter && page.is_some() && !not_staged && self.prompt.is_none() {
            let (out, keep): (Vec<Staged>, Vec<Staged>) = std::mem::take(&mut self.staged).into_iter().partition(|s| s.page.is_some());
            self.staged = keep;
            self.page_out.extend(out.into_iter().filter_map(|s| Some(pc::Staged { edit: s.page?, text: s.text, label: s.label, failed: None })));
        }
        // A prompt answered (or let go): back to the check.
        if std::mem::take(&mut self.to_check) {
            self.cl.focus = false;
            ctx.memory_mut(|m| m.surrender_focus(line_id));
            self.check = self.check.or(first_check);
            self.check_scroll = true;
        }

        // What the check asked the line for.
        if let Some(prompt) = &self.prompt {
            let text = match prompt {
                Prompt::Who(name) => format!("Who is “{name}”? Choose the artist in the list (or a new one) · Esc: never mind"),
                Prompt::Whole(_, name) => format!("Keep a name whole: leave exactly the name in the commandline (it has where “{name}” was found), then Enter · Esc: never mind"),
            };
            ui.label(RichText::new(text).color(P.accent).small());
        }
        // What Enter would do (or why it can't), when no list is in the way.
        if self.prompt.is_none() && candidates.is_empty() && page_rows.is_empty() && actions.is_empty() && verbs.is_empty() && !self.cl.text.trim().is_empty() {
            let shown: Vec<Result<(String, Option<String>), String>> = units.iter().map(|u| u.result.as_ref().map(|r| (r.label.clone(), r.warning.clone())).map_err(|e| e.clone())).collect();
            cl::would_do(ui, &shown, "Nothing to change by that name");
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted).small());
        }
        if self.importing {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Importing the metadata…").color(P.faint).small());
            });
        }
        if self.uploading > 0 {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new("Uploading the image (a change once it's there)…").color(P.faint).small());
            });
        }

        // ---- The staged changes ----
        if self.staged.is_empty() {
            if self.cl.text.is_empty() {
                let more = if single { " · ↓ goes to the fields below" } else { "" };
                ui.label(
                    RichText::new(format!("key:value sets it (several at once: | between them) · -key:value takes one off, -key clears · a word finds what to change · Ctrl+Enter applies{more}"))
                        .color(P.faint)
                        .small(),
                );
            }
        } else {
            ui.add_space(4.0);
            let chips: Vec<cl::Chip> = self.staged.iter().map(|s| cl::Chip { text: &s.text, label: &s.label, warning: s.warning.as_deref() }).collect();
            let remove = cl::chips(ui, &chips, editing);
            if let Some(n) = remove {
                self.staged.remove(n);
                self.apply_when_ready = false;
            }
            if self.confirm_discard {
                let text = if phase == Phase::Finished {
                    "Some weren't changed (below): Esc again closes anyway, discarding the changes (Keep editing tries again)."
                } else {
                    "Discard these changes? Esc again to discard them."
                };
                ui.label(RichText::new(text).color(P.danger).small());
            }
        }
        ui.add_space(8.0);

        // ---- State and preview ----
        let mut closed = esc_closed;
        let check_id = egui::Id::new(("track-editor-check", self.id));
        let mut check_clicked = None;
        if single {
            self.rows_view(ui, shell, catalog, targets[0], &rows, &vocab);
            if let Some((p, entries)) = &checking {
                ui.add_space(10.0);
                let view = check::View { parses: p, names: &self.names, catalog, engine: &self.engine };
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 260.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_height(260.0);
                    check_clicked = check::draw(ui, &view, entries, self.check, self.check_scroll, check_id);
                });
            }
            ui.add_space(10.0);
            closed = self.preview(ui, shell, catalog, &change, ctrl_enter, variant, true);
        } else {
            let (height, full) = (ui.available_height(), ui.available_width());
            let left = (full * 0.38).floor();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(left, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_height(height);
                    match &checking {
                        Some((p, entries)) => {
                            let view = check::View { parses: p, names: &self.names, catalog, engine: &self.engine };
                            check_clicked = check::draw(ui, &view, entries, self.check, self.check_scroll, check_id);
                        }
                        None => {
                            egui::ScrollArea::vertical().id_salt(("track-editor-state", self.id)).auto_shrink([false, false]).show(ui, |ui| {
                                state(ui, &self.engine, catalog, &self.names);
                            });
                        }
                    }
                });
                ui.add_space(16.0);
                ui.allocate_ui_with_layout(egui::vec2(full - left - 16.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_height(height);
                    closed = self.preview(ui, shell, catalog, &change, ctrl_enter, variant, false);
                });
            });
        }
        self.check_scroll = false;
        if let Some(n) = check_clicked {
            self.check = Some(n);
            self.row = None;
            ctx.memory_mut(|m| m.surrender_focus(line_id));
        }
        closed
    }

    fn loading(&self) -> bool {
        self.engine.plans.iter().any(|p| matches!(p.state, PlanState::Loading))
    }

    /// Tracks the staged changes change.
    fn changing(&self) -> usize {
        self.engine.plans.iter().filter(|p| matches!(&p.state, PlanState::Ready(r) if r.changes())).count()
    }

    /// Tracks the preview says can't be written.
    fn refused(&self) -> usize {
        self.engine.preview.lock().unwrap().answers.values().filter(|a| a.error.is_some()).count()
    }

    /// A track's flags after the edit, as the preview (or the edit just applied) says.
    fn after_flags(&self, track: Uuid) -> Option<Vec<String>> {
        let applied = self.engine.applied.lock().unwrap();
        if let Some(a) = applied.answers.get(&track) {
            return Some(a.flags.clone());
        }
        drop(applied);
        let preview = self.engine.preview.lock().unwrap();
        (preview.key == self.engine.plan_key && !preview.running && (!self.staged.is_empty() || self.engine.is_draft()))
            .then(|| preview.answers.get(&track).filter(|a| a.error.is_none()).map(|a| a.flags.clone()))
            .flatten()
    }

    /// Applying just finished: the library, the flags and the tracks' pages catch up. One
    /// track written: the changes are done with (its flags kept for `take_saved`).
    fn finished(&mut self, shell: &mut Shell, targets: &[Uuid]) {
        let (seen, outcomes, answers) = {
            let mut a = self.engine.applied.lock().unwrap();
            (std::mem::take(&mut a.finished_unseen), a.outcomes.clone(), a.answers.clone())
        };
        if !seen {
            return;
        }
        shell.act(Action::RefreshLibrary);
        shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
        // New kinds of flag: named everywhere from now on.
        if !model::new_flag_kinds(&self.ops()).is_empty() {
            let api = shell.api.clone();
            shell.account.refresh_vocab(&api, shell.runtime.handle(), &shell.ctx);
        }
        // (A draft is gone: the track it became is new.)
        if targets.len() <= 100 && !self.engine.is_draft() {
            for t in targets {
                shell.track_details.refresh(*t);
            }
        }
        // Drafts imported: the page takes the tracks they became (the drafts are gone).
        if self.engine.is_draft() {
            let made = self.engine.applied.lock().unwrap().made.clone();
            self.imported = outcomes
                .iter()
                .filter(|(_, o)| matches!(o, Outcome::Done))
                .filter_map(|(track, _)| Some((self.engine.drafts.get(track)?.problem, *made.get(track)?)))
                .collect();
            return;
        }
        let all_done = !outcomes.is_empty() && outcomes.values().all(|o| matches!(o, Outcome::Done));
        if targets.len() == 1 && all_done {
            self.choose_version = model::version(&self.ops());
            self.saved = Some(answers.get(&targets[0]).map(|a| a.flags.clone()).unwrap_or_default());
            // (A track page's changes staged since are its own.)
            self.staged.retain(|s| s.page.is_some());
            self.correcting = None;
            self.names = Names::default();
            self.engine.forget();
            self.notice = Some("Saved.".into());
        }
    }

    /// Puts the cursor at the end of the line (after the line was changed for you).
    /// A choice from the list: its text in the line for the value being typed, and what it
    /// stands for from now on; `next`: the next part to type after it.
    fn take(&mut self, c: &Candidate, part: &parse::Part, next: bool, ctx: &egui::Context) {
        // Who a name is, for the check: that artist (or a new one) from now on.
        if let Some(Prompt::Who(name)) = self.prompt.clone() {
            let id = match &c.pick {
                Some(Pick::Pin(PinKind::Artist, id)) => Some(*id),
                Some(Pick::NewArtist) => Some(self.names.new_artist(&c.text)),
                _ => None,
            };
            if let Some(id) = id {
                self.names.pin(PinKind::Artist, &name, id);
                self.reparse_all(|_| {});
                self.cl.text.clear();
                self.prompt = None;
                self.resolved = None;
                self.to_check = true;
            }
            return;
        }
        match &c.pick {
            Some(Pick::Pin(kind, id)) => self.names.pin(*kind, &c.text, *id),
            Some(Pick::NewArtist) => {
                let id = self.names.new_artist(&c.text);
                self.names.pin(PinKind::Artist, &c.text, id);
            }
            Some(Pick::NewAlbum) => {
                let id = self.names.new_album(&c.text);
                self.names.pin(PinKind::Album, &c.text, id);
            }
            Some(Pick::NewGrouping) => {
                let id = self.names.new_grouping(&c.text);
                self.names.pin(PinKind::Grouping, &c.text, id);
            }
            None => {}
        }
        let next = c.next.as_deref().filter(|_| next);
        // (What's typed already stays as typed: not quoted.)
        let text = (c.text != part.value).then(|| parse::quoted(&c.text));
        self.cl.take(ctx, part.value_start, part.end, text.as_deref(), next);
        self.resolved = None;
    }

    /// `text` staged: all of it, or (something in it can't be) none of it, saying why.
    fn stage(&mut self, text: &str, shell: &mut Shell, here: &Here) -> bool {
        if text.trim().is_empty() {
            return true;
        }
        let (mut units, _) = resolve::resolve(text, &here.ctx(&self.names), &mut Look(&mut *shell));
        self.page_units(text, &mut units, here.page);
        if units.is_empty() {
            self.notice = Some("Nothing to change there: key:value (artist:…, status:…), or a word to find what to change.".into());
            return false;
        }
        if let Some(err) = units.iter().find_map(|u| u.result.as_ref().err()) {
            self.notice = Some(format!("Not staged: {err}"));
            return false;
        }
        // What asking for something first says (no image in the clipboard…).
        let mut notice = None;
        for u in units {
            let Ok(mut r) = u.result else { continue };
            // The page's: done by the page, or staged to be sent by it.
            match r.page.take() {
                Some(tc::Does::Act(act)) => {
                    self.page_act = Some(act);
                    continue;
                }
                Some(tc::Does::Edit(edit)) => {
                    self.staged.push(Staged { text: u.text, label: r.label, ops: Vec::new(), warning: None, editable: true, for_you: false, page: Some(edit) });
                    continue;
                }
                None => {}
            }
            if let Some((id, kind)) = &r.new_kind {
                self.names.set_kind(*id, kind);
            }
            // `parse:`: the new artists it credits made (their stand-ins).
            for op in &mut r.ops {
                if let Op::ParseTitle(parsed) = op {
                    let mut made = (**parsed).clone();
                    made.make_new(&mut |name| self.names.new_artist(name));
                    *op = Op::ParseTitle(Arc::new(made));
                }
            }
            // The aliases it credits, named (for the preview's words).
            for op in &r.ops {
                if let Op::Credit { artist, alias: Some(Some(alias)), .. } = op
                    && let Some(a) = shell.artist_details.get(*artist).value.as_ref().and_then(|d| d.aliases.iter().find(|x| x.id == *alias)).cloned()
                {
                    self.names.alias(a.id, &a.name);
                }
            }
            match r.want {
                Some(Want::Paste { at }) => match crate::ui::edit::cover_list::clipboard_image() {
                    Some(Ok((bytes, name))) => self.upload(shell, bytes, name, "the pasted image".into(), at),
                    Some(Err(err)) => notice = Some(err),
                    None => notice = Some("There's no image in the clipboard (copy an image, or an image file, first).".to_string()),
                },
                Some(Want::File { at }) => {
                    self.chosen_at = at;
                    crate::ui::files::choose(shell, "A cover", ("Images", crate::ui::files::IMAGES), false, &self.chosen);
                }
                Some(Want::Correct(n)) => self.correct(n, here.vocab),
                None => self.staged.push(Staged { text: u.text, label: r.label, ops: r.ops, warning: r.warning, editable: true, for_you: false, page: None }),
            }
        }
        // A Ctrl+Enter waiting was for what was staged before.
        self.apply_when_ready = false;
        self.resolved = None;
        self.notice = notice;
        self.confirm_discard = false;
        true
    }

    /// Images for covers: pasted (Ctrl+V) or dropped anywhere in the editor, chosen in the
    /// file dialog, uploaded; and those uploaded, staged as changes.
    fn images(&mut self, ctx: &egui::Context, shell: &mut Shell, targets: usize) {
        let chosen = std::mem::take(&mut *self.chosen.lock().unwrap());
        let from_file = !chosen.is_empty();
        let taking = !shell.palette_open && !shell.modal_open;
        if taking || from_file {
            let at = if from_file { self.chosen_at } else { None };
            match crate::ui::edit::cover_list::take_image(ctx, taking, chosen) {
                Some(Ok((bytes, name))) => {
                    let what = if name == "pasted.png" { "the pasted image".to_string() } else { format!("the image “{name}”") };
                    self.upload(shell, bytes, name, what, at);
                }
                Some(Err(err)) => self.notice = Some(err),
                None => {}
            }
        }
        let done = std::mem::take(&mut *self.uploaded.lock().unwrap());
        for result in done {
            self.uploading = self.uploading.saturating_sub(1);
            match result {
                Ok((sha, what, at)) => {
                    let place = match at {
                        None => String::new(),
                        Some(0) => " as the main one".into(),
                        Some(n) => format!(" as #{}", n + 1),
                    };
                    let warning = (targets > 1).then(|| format!("each of the {targets} tracks gets this cover"));
                    let op = Op::Cover { image: model::Image::Staged, sha, at };
                    self.staged.push(Staged { text: format!("cover: {what}{place}"), label: format!("cover: + {what}{place}"), ops: vec![op], warning, editable: false, for_you: false, page: None });
                    self.apply_when_ready = false;
                }
                Err(err) => self.notice = Some(format!("The image couldn't be uploaded: {err}")),
            }
        }
    }

    /// Fetches what importing `pasted` needs, in the background: a YouTube video's
    /// metadata (the server's yt-dlp), who has the uploader's account, the artwork.
    fn fetch(&mut self, shell: &Shell, pasted: Pasted) {
        self.importing = true;
        self.notice = None;
        let (api, out, ctx) = (shell.api.clone(), self.fetched.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = async {
                let json = match pasted {
                    Pasted::Json(json) => json,
                    Pasted::Broken(why) => return Err(why),
                    Pasted::YouTube(link) => {
                        let body = serde_json::json!({ "link": link });
                        api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/metadata/youtube", Some(body)).await.map_err(|e| e.message)?
                    }
                };
                let meta = import::parse(&json)?;
                let owners = match &meta.account {
                    Some(a) => {
                        let body = serde_json::json!({ "platform": a.platform, "ids": a.ids });
                        api.send_json::<Vec<Owner>>(reqwest::Method::POST, "/api/sources/accounts", Some(body)).await.map_err(|e| e.message)?
                    }
                    None => Vec::new(),
                };
                let mut artwork = None;
                for url in &meta.artwork {
                    // The first that's there (an original is a JPEG or a PNG), and an image.
                    if let Ok(answer) = reqwest::get(url).await
                        && answer.status().is_success()
                        && let Ok(bytes) = answer.bytes().await
                        && image::guess_format(&bytes).is_ok()
                    {
                        artwork = Some(bytes.to_vec());
                        break;
                    }
                }
                Ok::<_, String>(Fetched { meta, owners, artwork })
            }
            .await;
            *out.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// Stages what was imported, each its own chip (taken back like any).
    fn stage_imported(&mut self, fetched: Fetched, shell: &mut Shell, catalog: &Catalog) {
        let Fetched { meta: m, owners, artwork } = fetched;
        let from = m.from;
        let may_source = shell.account.can("edit_sources");
        let mut chips: Vec<Staged> = Vec::new();
        let mut chip = |text: String, label: String, ops: Vec<Op>, warning: Option<String>| chips.push(Staged { text, label, ops, warning, editable: false, for_you: false, page: None });
        let version = match self.engine.is_draft() {
            true => format!("its version named “{from}”, with only what's imported"),
            false => format!("a new version “{from}” with only what's imported (not the shown one: choose it after)"),
        };
        chip(format!("version:{from}"), version, vec![Op::Version(from.to_string())], None);
        chip(format!("title:{}", m.title), format!("title: {}", m.title), vec![Op::Title(m.title.clone())], None);
        if let Some(date) = &m.date {
            chip(format!("date:{date}"), format!("release date: {date}"), vec![Op::Date(Some(date.clone()))], None);
        }
        if let Some(genre) = &m.genre {
            chip(format!("genre:{genre}"), format!("genre: + {genre}"), vec![Op::Genre(genre.clone())], None);
        }
        let credit = |artist: Uuid| Op::Credit { artist, role: Some("primary".into()), alias: None, at: None };
        for (n, name) in m.artists.iter().enumerate() {
            let account = m.account.as_ref().filter(|_| n == 0);
            let has = account.map(|a| format!(" (it has the account {})", a.label)).unwrap_or_default();
            match account.and(owners.first()) {
                Some(o) if o.kind == "grouping" => {
                    chip(format!("group:{}", o.name), format!("into the collective “{}”{has}, no one credited as it", o.name), vec![Op::Group(o.id)], Some("give it its artist too (artist:…)".into()));
                }
                Some(o) => chip(format!("artist:{}", o.name), format!("artists: + {}{has}", o.name), vec![credit(o.id)], None),
                None => {
                    let mut named: Vec<&crate::catalog::model::Artist> = catalog.artists.iter().filter(|a| a.name.eq_ignore_ascii_case(name.trim())).collect();
                    named.sort_by_key(|a| std::cmp::Reverse(a.track_count));
                    let (id, label, warning) = match named.as_slice() {
                        [] => (self.names.new_artist(name), format!("artists: + {name} (new)"), None),
                        [one] => (one.id, format!("artists: + {}", one.label()), None),
                        [first, ..] => (first.id, format!("artists: + {}", first.label()), Some(format!("{} artists are named “{name}”: the one with the most tracks", named.len()))),
                    };
                    chip(format!("artist:{name}"), label, vec![credit(id)], warning);
                    if let Some(a) = account.filter(|_| may_source) {
                        let op = Op::Account { artist: id, platform: a.platform.clone(), label: a.label.clone(), data: a.data.clone() };
                        chip(format!("account:{} → {name}", a.label), format!("{name} gets the account {} as a source (take this back for someone else's upload)", a.label), vec![op], None);
                    }
                }
            }
        }
        if may_source {
            let (platform, id, data) = m.source.clone();
            // A stand-in id, as `source:` makes them.
            let stand_in = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (&platform, data.to_string()).hash(&mut h);
                Uuid::from_u128(0xACE7_5000_0000_4000_8000_0000_0000_0000 | h.finish() as u128)
            };
            let op = Op::Source { id: stand_in, platform: platform.clone(), name: id.clone(), data };
            chip(format!("source:{platform} {id}"), format!("source: + {platform} {id}, with what the {from} JSON says"), vec![op], None);
        }
        let count = chips.len();
        self.staged.extend(chips);
        self.apply_when_ready = false;
        let mut notice = format!("Imported from {from}: {count} changes staged (take back what's wrong).");
        if !may_source {
            notice.push_str(" Its source and account aren't: they need the edit_sources permission.");
        }
        match artwork {
            Some(bytes) => self.upload(shell, bytes, format!("{}-artwork.jpg", from.to_lowercase()), format!("the {from} artwork"), None),
            None if !m.artwork.is_empty() => notice.push_str(" Its artwork couldn't be loaded."),
            None => {}
        }
        self.notice = Some(notice);
    }

    /// Uploads an image for a cover (staged on the server; a change once it's there).
    fn upload(&mut self, shell: &Shell, bytes: Vec<u8>, name: String, what: String, at: Option<usize>) {
        #[derive(serde::Deserialize)]
        struct StagedImage {
            blob_sha256: String,
        }
        self.uploading += 1;
        self.notice = None;
        let (api, uploaded, ctx) = (shell.api.clone(), self.uploaded.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let result = api.upload::<StagedImage>("/api/covers/staged", bytes, &name).await.map(|s| (s.blob_sha256, what, at)).map_err(|err| err.message);
            uploaded.lock().unwrap().push(result);
            ctx.request_repaint();
        });
    }

    /// Opens the form correcting the source in place `n` (one track), as the staged changes
    /// leave it.
    fn correct(&mut self, n: usize, vocab: &Vocab) {
        let Some(s) = (self.engine.plans.len() == 1).then(|| self.engine.plans.first().and_then(|p| self.engine.ready(p.track))).flatten().and_then(|r| r.attached.sources.get(n)) else {
            self.notice = Some(format!("It has no source #{}.", n + 1));
            return;
        };
        let shapes: Vec<crate::account::SourceShape> = vocab.source_shapes.iter().filter(|x| x.kind == "track").cloned().collect();
        self.correcting = Some((n, crate::ui::edit::sources::Form::for_data(s.id, &s.platform, &s.data, &shapes)));
        self.row = None;
    }

    /// The form correcting a source, under the rows: staged as a change when it's done.
    fn correction(&mut self, ui: &mut egui::Ui, vocab: &Vocab) {
        let Some((n, form)) = self.correcting.as_mut() else { return };
        let n = *n;
        let Some(shape) = vocab.source_shapes.iter().find(|x| x.kind == "track" && x.platform == form.platform) else {
            self.correcting = None;
            return;
        };
        let (mut stage, mut cancel) = (false, false);
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.label(RichText::new(format!("Correcting source #{} ({}): a new snapshot of it when applied, the old ones kept.", n + 1, form.platform)).color(P.muted));
            form.fields(ui, shape);
            ui.horizontal(|ui| {
                stage = ui.button("Stage the correction").clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
        if cancel {
            self.correcting = None;
        } else if stage {
            match (form.data(shape), form.correcting) {
                (Ok(data), Some(id)) => {
                    let label = format!("source #{} ({}) corrected", n + 1, form.platform);
                    self.staged.push(Staged { text: format!("source:#{} corrected", n + 1), label, ops: vec![Op::CorrectSource { id, data }], warning: None, editable: false, for_you: false, page: None });
                    self.correcting = None;
                    self.apply_when_ready = false;
                    self.cl.focus = true;
                }
                (Err(err), _) => self.notice = Some(err),
                (_, None) => self.correcting = None,
            }
        }
    }

    /// A key on one track's row: an edit through the line (filled in, the cursor there), or
    /// a change staged at once (taking something off, moving an artist).
    fn row_key(&mut self, kind: RowKind, key: RowKey, r: &engine::Ready, shell: &mut Shell, here: &Here) {
        let q = parse::quoted;
        let catalog = here.catalog;
        // A name in the line that stands for exactly this one.
        let artist = |names: &mut Names, id: Uuid| -> String {
            let name = catalog.artist(id).map(|a| a.name.clone()).or_else(|| names.stand_ins().get(&id).cloned()).unwrap_or_default();
            names.pin(PinKind::Artist, &name, id);
            q(&name)
        };
        let album = |names: &mut Names, id: Uuid| -> String {
            let title = catalog.album(id).map(|a| a.title().to_string()).or_else(|| names.stand_ins().get(&id).cloned()).unwrap_or_default();
            names.pin(PinKind::Album, &title, id);
            q(&title)
        };
        let track = |names: &mut Names, id: Uuid| -> String {
            let title = catalog.track_index(id).map(|i| catalog.track_at(i).title().to_string()).unwrap_or_default();
            names.pin(PinKind::Track, &title, id);
            q(&title)
        };
        let fill = |this: &mut Self, text: String| this.prefill(&text);
        match (kind, key) {
            (RowKind::Title, RowKey::Edit) => fill(self, format!("title:{}", q(&r.meta.title))),
            (RowKind::Date, RowKey::Edit) => fill(self, format!("date:{}", r.meta.release_date.clone().unwrap_or_default())),
            (RowKind::Date, RowKey::Remove) => {
                self.stage("date:none", shell, here);
            }
            (RowKind::Comment, RowKey::Edit) => fill(self, format!("comment:{}", q(r.meta.comment.as_deref().unwrap_or_default()))),
            (RowKind::Comment, RowKey::Remove) => {
                self.stage("comment:none", shell, here);
            }
            (RowKind::Status, RowKey::Edit) => fill(self, "status:".into()),
            (RowKind::Genres, RowKey::Edit) => fill(self, "genre:".into()),
            (RowKind::Genres, RowKey::Remove) => {
                self.stage("-genre", shell, here);
            }
            (RowKind::AddArtist, RowKey::Edit) => fill(self, "artist:".into()),
            (RowKind::AddAlbum, RowKey::Edit) => fill(self, "album:".into()),
            (RowKind::AddRelated, RowKey::Edit) => fill(self, "related:".into()),
            (RowKind::AddCover, RowKey::Edit) => fill(self, "cover:".into()),
            (RowKind::AddGroup, RowKey::Edit) | (RowKind::Group(_), RowKey::Edit) => fill(self, "group:".into()),
            (RowKind::Group(n), RowKey::Remove) => {
                if let Some(g) = r.attached.groups.get(n).copied() {
                    let name = catalog.grouping(g).map(|x| x.name.clone()).or_else(|| self.names.stand_ins().get(&g).cloned()).unwrap_or_default();
                    self.names.pin(PinKind::Grouping, &name, g);
                    if self.names.is_new(g) {
                        // Not made yet: what put it there is taken back.
                        self.staged.retain(|s| !s.ops.contains(&Op::Group(g)));
                    } else {
                        self.stage(&format!("-group:{}", q(&name)), shell, here);
                    }
                }
            }
            (RowKind::AddSource, RowKey::Edit) => fill(self, "source:".into()),
            (RowKind::Cover(n), key) => {
                let at = n + 1;
                match key {
                    RowKey::Edit => fill(self, format!("cover:#{at} #")),
                    RowKey::Main if n > 0 => {
                        self.stage(&format!("cover:#{at} #1"), shell, here);
                    }
                    RowKey::Earlier if n > 0 => {
                        self.stage(&format!("cover:#{at} #{n}"), shell, here);
                    }
                    RowKey::Later if n + 1 < r.attached.covers.len() => {
                        self.stage(&format!("cover:#{at} #{}", at + 1), shell, here);
                    }
                    RowKey::Remove => {
                        self.stage(&format!("-cover:#{at}"), shell, here);
                    }
                    RowKey::View => {
                        if let Some(key) = r.attached.covers.get(n).and_then(|p| crate::catalog::model::CoverKey::from_hex(&p.sha)) {
                            let ctx = shell.ctx.clone();
                            shell.open_original(&ctx, crate::catalog::model::Cover { id: Uuid::nil(), key });
                        }
                    }
                    _ => {}
                }
            }
            (RowKind::Source(n), key) => {
                let at = n + 1;
                match key {
                    RowKey::Edit => {
                        self.stage(&format!("source:#{at}"), shell, here);
                    }
                    RowKey::Remove => {
                        self.stage(&format!("-source:#{at}"), shell, here);
                    }
                    RowKey::Origin => {
                        self.stage(&format!("origin:#{at}"), shell, here);
                    }
                    _ => {}
                }
            }
            (RowKind::Credit(n), key) => {
                let Some(c) = r.meta.credits.get(n).cloned() else { return };
                let name = artist(&mut self.names, c.artist_id);
                match key {
                    // Another role: off in this one, on in the one chosen, same place.
                    RowKey::Edit | RowKey::Role => fill(self, format!("-artist:{name} role:{} | artist:{name} #{} role:", c.role, n + 1)),
                    RowKey::Alias => fill(self, format!("artist:{name} role:{} as:", c.role)),
                    RowKey::Remove => {
                        self.stage(&format!("-artist:{name} role:{}", c.role), shell, here);
                    }
                    RowKey::Earlier if n > 0 => {
                        self.stage(&format!("artist:{name} role:{} #{n}", c.role), shell, here);
                    }
                    RowKey::Later if n + 1 < r.meta.credits.len() => {
                        self.stage(&format!("artist:{name} role:{} #{}", c.role, n + 2), shell, here);
                    }
                    _ => {}
                }
            }
            (RowKind::Album(n), key) => {
                let Some(m) = r.facts.albums.get(n).cloned() else { return };
                let title = album(&mut self.names, m.album_id);
                match key {
                    RowKey::Edit => {
                        let numbers = match (m.disc_number, m.track_number) {
                            (Some(d), Some(t)) => format!(" #{d}-{t}"),
                            (_, Some(t)) => format!(" #{t}"),
                            _ => " #".into(),
                        };
                        fill(self, format!("album:{title}{numbers}"));
                    }
                    RowKey::Remove => {
                        self.stage(&format!("-album:{title}"), shell, here);
                    }
                    _ => {}
                }
            }
            (RowKind::Related(n), key) => {
                let Some(rel) = r.facts.relations.get(n).cloned() else { return };
                let title = track(&mut self.names, rel.track_id);
                match key {
                    RowKey::Edit => fill(self, format!("-related:{title} | related:{title} kind:")),
                    RowKey::Remove => {
                        self.stage(&format!("-related:{title}"), shell, here);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// One track's rows: each field (the one the keys are on lit), its value as it will be
    /// (what it was, struck through), and the flags about it.
    fn rows_view(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, track: Uuid, rows: &[Row], vocab: &Vocab) {
        let mut original = None;
        if rows.is_empty() {
            let text = match self.engine.plans.first().map(|p| &p.state) {
                Some(PlanState::Failed(err)) => err.clone(),
                _ => "Loading the track…".into(),
            };
            ui.label(RichText::new(text).color(P.faint));
            return;
        }
        let mut clicked = None;
        // An ambiguous name's candidate clicked: (the flag, the one credited, the one clicked).
        let mut chosen: Option<(Uuid, Uuid, Uuid)> = None;
        // The value gets the room the key and the flags leave (a grid's cells are only as
        // wide as they're told: text in them wraps after a word or two otherwise).
        let value_width = (ui.available_width() - 90.0 - 220.0).max(240.0);
        egui::Grid::new(("track-editor-rows", self.id)).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            for (n, row) in rows.iter().enumerate() {
                let lit = self.row == Some(n);
                let key = RichText::new(format!("{}:", row.key)).color(if lit { P.accent } else { P.faint });
                if ui.add(egui::Label::new(key).sense(egui::Sense::click())).clicked() {
                    clicked = Some(n);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.set_min_width(value_width);
                    ui.set_max_width(value_width);
                    let empty = matches!(row.kind, RowKind::AddArtist | RowKind::AddAlbum | RowKind::AddRelated | RowKind::AddCover | RowKind::AddSource | RowKind::AddGroup);
                    let mut value = if row.kind == RowKind::Removed {
                        // Taken off by the staged changes.
                        RichText::new(row.was.clone().unwrap_or_default()).color(P.faint).strikethrough()
                    } else {
                        let text = if row.value.is_empty() { "—".to_string() } else { row.value.clone() };
                        let color = if row.was.is_some() { P.accent } else if empty || row.kind == RowKind::Other { P.faint } else { P.text };
                        RichText::new(text).color(color)
                    };
                    if lit {
                        value = value.background_color(P.surface_hi);
                    }
                    // A cover: small (a click: the original).
                    if let Some(key) = row.image.as_deref().and_then(crate::catalog::model::CoverKey::from_hex) {
                        let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::click());
                        let cover = crate::catalog::model::Cover { id: Uuid::nil(), key };
                        shell.cover(ui, Some(cover), Uuid::from_u128(key.0 as u128), rect, 4, 0.0);
                        if response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text("The original").clicked() {
                            original = Some(cover);
                        }
                    }
                    if ui.add(egui::Label::new(value).sense(egui::Sense::click())).clicked() {
                        clicked = Some(n);
                    }
                    if row.kind == RowKind::Removed {
                        ui.label(RichText::new("taken off").color(P.faint).small());
                    } else if let Some(was) = row.was.as_ref().filter(|w| *w != "new") {
                        let was = if was.is_empty() { "—" } else { was.as_str() };
                        ui.label(RichText::new(was).color(P.faint).strikethrough());
                    }
                    // An ambiguous name: which of its artists it is (the credited one lit).
                    if let (Some((flag, candidates)), Some(credited)) = (&row.choose, row.artist) {
                        ui.label(RichText::new("which:").color(P.faint).small());
                        for c in candidates {
                            let label = catalog.artist_label(*c).unwrap_or_else(|| "(an artist)".into());
                            let size = catalog.artist(*c).map(|a| format!(" · {} tracks", a.track_count)).unwrap_or_default();
                            if ui.selectable_label(*c == credited, RichText::new(format!("{label}{size}")).small()).clicked() {
                                chosen = Some((*flag, credited, *c));
                            }
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.set_min_width(200.0);
                    for (kind, state) in &row.flags {
                        let name = vocab.review_kind_name(kind).to_lowercase();
                        let (text, color) = match state {
                            Flag::Open => (format!("⚠ {name}"), P.danger),
                            Flag::Still => (format!("⚠ {name} (still)"), P.danger),
                            Flag::Fixed => (format!("✓ {name}"), P.accent),
                            Flag::New => (format!("⚠ {name} (new)"), P.danger),
                        };
                        ui.label(RichText::new(text).color(color).small());
                    }
                });
                ui.end_row();
            }
        });
        if let Some(n) = clicked {
            self.row = Some(n);
            ui.ctx().memory_mut(|m| m.surrender_focus(self.cl.id()));
        }
        if let Some(cover) = original {
            shell.open_original(ui.ctx(), cover);
        }
        self.correction(ui, vocab);
        match chosen {
            // The one credited, as it is now: it was right.
            Some((flag, credited, c)) if c == credited && !self.has_changes() => {
                shell.flags.resolve(&shell.api, shell.runtime.handle(), &[flag], "the credited artist was right");
            }
            Some((_, credited, c)) if c != credited => self.replace_artist(catalog, track, credited, c),
            _ => {}
        }
        if self.row.is_some() {
            ui.label(
                RichText::new("Enter edits it · on an artist: r role, a credited as, Alt+↑/↓ (or Alt+k/j) its place, c the next of an ambiguous name's artists · on a cover: m the main one, Alt+↑/↓ its place, Space the original · on a source: Enter corrects it, o the file came from it · Delete takes it off · Esc back to the commandline")
                    .color(P.faint)
                    .small(),
            );
        } else if rows.iter().any(|r| r.choose.is_some()) {
            ui.label(RichText::new("An ambiguous name: click which artist it is (the one credited, as it is: it was right).").color(P.faint).small());
        }
    }

    /// The preview (and applying it, and what came of it). True: closed. `single`: one
    /// track (the rows say its flags; applying waits for the preview if it must).
    #[allow(clippy::too_many_arguments)]
    fn preview(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, change: &Change, ctrl_enter: bool, variant: Option<Uuid>, single: bool) -> bool {
        let (phase, done, total, outcomes, applied_answers) = {
            let a = self.engine.applied.lock().unwrap();
            (a.phase, a.done, a.total, a.outcomes.clone(), a.answers.clone())
        };
        let (answers, preview_error, running) = {
            let p = self.engine.preview.lock().unwrap();
            (p.answers.clone(), p.error.clone(), p.running)
        };
        let changing = self.changing();
        let unchanged = self.engine.plans.iter().filter(|p| matches!(&p.state, PlanState::Ready(r) if !r.changes())).count();
        let refused = answers.values().filter(|a| a.error.is_some()).count();
        let current = self.engine.preview_current();
        let loading = self.loading();
        let mut closed = false;

        match phase {
            Phase::Editing => {
                // A name more than one artist has: chosen first (or its tracks left out).
                let mut undecided: Vec<String> = self.staged_parses().iter().flat_map(|p| p.undecided().iter().map(|n| format!("“{}”", n.text)).collect::<Vec<_>>()).collect();
                undecided.dedup();
                let can = current && changing > refused && !loading && self.cl.text.trim().is_empty() && self.uploading == 0 && undecided.is_empty();
                let text = if self.engine.is_draft() && single { "Import  (Ctrl+Enter)".to_string() } else if self.engine.is_draft() { format!("Import {} files  (Ctrl+Enter)", thousands(changing - refused.min(changing))) } else if single { "Apply  (Ctrl+Enter)".to_string() } else { format!("Apply to {} tracks  (Ctrl+Enter)", thousands(changing - refused.min(changing))) };
                let mut apply = false;
                ui.horizontal(|ui| {
                    if changing > 0 {
                        apply = ui.add_enabled(can, egui::Button::new(text)).clicked();
                    }
                    if changing == 0 {
                        if self.has_metadata_changes() {
                            ui.label(RichText::new("It changes nothing.").color(P.faint));
                        }
                    } else if loading || running || !current {
                        ui.add(egui::Spinner::new());
                        ui.label(RichText::new("Checking it with the server…").color(P.faint));
                    }
                });
                if !undecided.is_empty() {
                    ui.label(RichText::new(format!("Choose who {} is first, in the check (↓ from the commandline), or leave those tracks out (x).", undecided.join(", "))).color(P.danger).small());
                }
                if ctrl_enter {
                    if can {
                        apply = true;
                    } else if !undecided.is_empty() {
                        self.notice = Some(format!("Not applied: choose who {} is first.", undecided.join(", ")));
                    } else if self.uploading > 0 {
                        self.notice = Some("An image is still uploading: Ctrl+Enter again once it's a change.".into());
                    } else if single && (!self.staged.is_empty() || self.engine.is_draft()) && self.cl.text.trim().is_empty() {
                        // (What was typed may have been staged just now: its preview comes
                        // next.) Applied once the preview's there, if it can be.
                        self.apply_when_ready = true;
                    } else if changing > 0 && (running || !current || loading) {
                        self.notice = Some("The preview isn't up to date yet: Ctrl+Enter again once it is.".into());
                    }
                }
                if apply {
                    self.engine.apply(shell, change, variant);
                }
                // The new things it makes, named.
                let mut new = Vec::new();
                let parsed_new = |id: &Uuid| change.ops.iter().any(|op| matches!(op, Op::ParseTitle(p) if p.names.iter().any(|n| n.credited == title_parse::Credited::New(Some(*id)))));
                for (id, name) in &self.names.new_artists {
                    if change.ops.iter().any(|op| matches!(op, Op::Credit { artist, .. } if artist == id)) || parsed_new(id) {
                        new.push(format!("artist “{name}”"));
                    }
                }
                for (id, title, kind) in &self.names.new_albums {
                    if change.ops.iter().any(|op| matches!(op, Op::Album { album, .. } if album == id)) {
                        new.push(format!("{kind} “{title}” (credited to the primary artists its tracks share)"));
                    }
                }
                for (id, name, kind) in &self.names.new_groupings {
                    if change.ops.iter().any(|op| matches!(op, Op::Group(g) if g == id)) {
                        new.push(format!("{kind} “{name}”"));
                    }
                }
                if model::single(&change.ops) {
                    let ready: Vec<&engine::Ready> = self.engine.plans.iter().filter_map(|p| if let PlanState::Ready(r) = &p.state { Some(&**r) } else { None }).collect();
                    let on_albums = ready.iter().filter(|r| !r.warnings.is_empty()).count();
                    let mut text = if single { "a single of its own".to_string() } else { format!("{} singles, one for each", thousands(ready.len())) };
                    if on_albums > 0 && !single {
                        text.push_str(&format!(" ({} of them on an album already)", thousands(on_albums)));
                    }
                    new.push(text);
                }
                if !new.is_empty() {
                    ui.label(RichText::new(format!("Makes: {}", new.join(", "))).color(P.muted).small());
                }
                if !change.along {
                    ui.label(RichText::new("Albums the tracks are alone on are left as they are (along:off).").color(P.muted).small());
                }
                if let Some(err) = preview_error {
                    ui.label(RichText::new(format!("The server couldn't check it: {err}")).color(P.danger));
                }
            }
            Phase::Applying => {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("Changing… {done} of {total}")));
            }
            Phase::Finished => {
                let failed = outcomes.values().filter(|o| matches!(o, Outcome::Failed(_))).count();
                let ok = outcomes.len() - failed;
                let text = if failed == 0 { format!("Done: {ok} tracks changed.") } else { format!("Done: {ok} changed, {failed} not (below).") };
                ui.label(RichText::new(text).color(if failed == 0 { P.accent } else { P.danger }));
                let (mut close, mut keep) = (false, false);
                ui.horizontal(|ui| {
                    close = ui.button("Close  (Esc)").on_hover_text("Done: the changes are cleared").clicked();
                    keep = ui.button("Keep editing").on_hover_text("The same changes, on the tracks as they are now").clicked();
                });
                if close {
                    self.close();
                    closed = true;
                } else if keep {
                    self.engine.forget();
                }
            }
        }
        ui.add_space(6.0);

        // ---- Each track ----
        let issues_before = |track: Uuid| -> Vec<String> {
            let done = &shell.flags.done;
            catalog.issues.get(&track).map(|f| f.iter().filter(|(id, _)| !done.contains(id)).map(|(_, k)| k.clone()).collect()).unwrap_or_default()
        };
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let each = |ui: &mut egui::Ui| {
            for plan in &self.engine.plans {
                let mut warnings: &[String] = &[];
                let (mark, color, lines, err): (&str, egui::Color32, Vec<String>, Option<String>) = match &plan.state {
                    PlanState::Loading => continue,
                    PlanState::Failed(err) => ("✗", P.danger, Vec::new(), Some(err.clone())),
                    PlanState::Ready(r) if !r.changes() => continue,
                    PlanState::Ready(r) => {
                        let answer = applied_answers.get(&plan.track).or_else(|| answers.get(&plan.track));
                        let mut lines = r.lines.clone();
                        if let Some(a) = answer {
                            for along in &a.along {
                                lines.extend(engine::along_lines(along, catalog));
                            }
                        }
                        let err = match outcomes.get(&plan.track) {
                            Some(Outcome::Failed(why)) => Some(why.clone()),
                            _ => answer.and_then(|a| a.error.as_ref()).filter(|e| e.code != "stale").map(|e| e.message.clone()),
                        };
                        let mark = match outcomes.get(&plan.track) {
                            Some(Outcome::Done) => "✓",
                            Some(Outcome::Failed(_)) => "✗",
                            None if err.is_some() => "✗",
                            None => "",
                        };
                        let color = if err.is_some() { P.danger } else if mark == "✓" { P.accent } else { P.text };
                        warnings = &r.warnings;
                        (mark, color, lines, err)
                    }
                };
                if !single {
                    ui.label(RichText::new(format!("{mark} {}", plan.title).trim().to_string()).font(font(13.5, semibold())).color(color));
                }
                let indent = if single { "" } else { "    " };
                for line in &lines {
                    ui.label(RichText::new(format!("{indent}{line}")).color(P.muted));
                }
                for w in warnings {
                    ui.label(RichText::new(format!("{indent}⚠ {w}")).color(P.text));
                }
                if let Some(err) = &err {
                    ui.label(RichText::new(format!("{indent}{err}")).color(P.danger));
                }
                // Made to match, and nothing to change: said so (once the server has).
                let answered = applied_answers.contains_key(&plan.track) || answers.contains_key(&plan.track);
                if change.sync && lines.is_empty() && err.is_none() && answered {
                    ui.label(RichText::new(format!("{indent}its albums match it already (or it's alone on none)")).color(P.faint));
                }
                // Its flags (with one track the rows say them).
                let answer = applied_answers.get(&plan.track).or_else(|| answers.get(&plan.track));
                if let Some(a) = answer.filter(|a| a.error.is_none() && !single) {
                    let before = issues_before(plan.track);
                    let name = |k: &String| vocab.review_kind_name(k).to_lowercase();
                    let fixed: Vec<String> = before.iter().filter(|k| !a.flags.contains(k)).map(name).collect();
                    let still: Vec<String> = a.flags.iter().filter(|k| before.contains(k)).map(name).collect();
                    let new: Vec<String> = a.flags.iter().filter(|k| !before.contains(k)).map(name).collect();
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(24.0);
                        if !fixed.is_empty() {
                            ui.label(RichText::new(format!("fixes: {}", fixed.join(", "))).color(P.accent).small());
                        }
                        if !new.is_empty() {
                            ui.label(RichText::new(format!("new: {}", new.join(", "))).color(P.danger).small());
                        }
                        if !still.is_empty() {
                            ui.label(RichText::new(format!("still: {}", still.join(", "))).color(P.faint).small());
                        }
                    });
                }
                ui.add_space(4.0);
            }
            if unchanged > 0 && !self.staged.is_empty() && !single {
                ui.label(RichText::new(format!("{} unchanged", thousands(unchanged))).color(P.faint));
            }
        };
        if single {
            each(ui);
        } else {
            egui::ScrollArea::vertical().id_salt(("track-editor-preview", self.id)).auto_shrink([false, false]).show(ui, each);
        }
        closed
    }
}

impl Here<'_> {
    fn ctx<'b>(&'b self, names: &'b Names) -> Ctx<'b> {
        Ctx {
            catalog: self.catalog,
            vocab: self.vocab,
            targets: self.targets,
            context_album: self.album,
            staged_album: self.staged_album,
            names,
            may_relate: self.may_relate,
            may_source: self.may_source,
            titles: self.titles,
            artist_names: self.artist_names,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowKey {
    Edit,
    Role,
    Alias,
    Remove,
    Earlier,
    Later,
    /// An ambiguous name's next candidate.
    Next,
    /// A cover made the main one.
    Main,
    /// The file came from this source.
    Origin,
    /// A cover's original, in full.
    View,
}

/// The library's details, as resolving asks for them (loaded in the background).
struct Look<'a>(&'a mut Shell);

impl resolve::Lookups for Look<'_> {
    fn aliases(&mut self, artist: Uuid) -> Option<Vec<AliasInfo>> {
        self.0.artist_details.get(artist).value.as_ref().map(|a| a.aliases.clone())
    }

    fn covers(&mut self, kind: PinKind, id: Uuid) -> Option<Vec<(Uuid, String)>> {
        let list = |covers: &[crate::details::Cover]| covers.iter().map(|c| (c.id, c.blob_sha256.clone())).collect();
        match kind {
            PinKind::Album => self.0.album_details.get(id).value.as_ref().map(|d| list(&d.covers)),
            _ => self.0.track_details.get(id).value.as_ref().map(|d| list(&d.covers)),
        }
    }
}

/// Whether the keys are the editor's (drawn on `layer`): not if something is over it. On a
/// page, any popup; in a popup (the bulk editor), another one opened on top of it (it sets
/// `modal_open` itself, so that alone can't say).
fn keys_free(ctx: &egui::Context, layer: egui::LayerId, palette_open: bool, modal_was_open: bool) -> bool {
    !palette_open
        && match layer.order {
            egui::Order::Foreground => ctx.memory(|m| m.areas().visible_layer_ids().iter().all(|l| l.order != egui::Order::Foreground || *l == layer)),
            _ => !modal_was_open,
        }
}

/// What the keys do in the list under the line.
const LIST_HELP: &str = "↑/↓ choose · Tab: take it and type the next part · Enter: take it and stage it";

/// Each field as the tracks have it, and as they will (with many tracks: how many differ).
fn state(ui: &mut egui::Ui, engine: &Engine, catalog: &Catalog, names: &Names) {
    let ready: Vec<&engine::Ready> = engine.plans.iter().filter_map(|p| if let PlanState::Ready(r) = &p.state { Some(&**r) } else { None }).collect();
    if ready.is_empty() {
        ui.label(RichText::new("Loading the tracks…").color(P.faint));
        return;
    }
    let new_names = names.stand_ins();
    let artist = |id: Uuid| catalog.artist(id).map(|a| a.name.clone()).or_else(|| new_names.get(&id).cloned()).unwrap_or_else(|| "(an artist)".into());
    let album = |id: Uuid| catalog.album(id).map(|a| a.title().to_string()).or_else(|| new_names.get(&id).cloned()).unwrap_or_else(|| "(an album)".into());
    let track = |id: Uuid| catalog.track_index(id).map(|i| catalog.track_at(i).title().to_string()).unwrap_or_else(|| "(a track)".into());
    let grouping = |id: Uuid| catalog.grouping(id).map(|g| g.name.clone()).or_else(|| new_names.get(&id).map(|n| format!("{n} (new)"))).unwrap_or_else(|| "(a grouping)".into());
    type Field<'a> = (&'static str, Box<dyn Fn(&model::Meta, &model::Facts, &model::Attached) -> String + 'a>);
    let fields: Vec<Field> = vec![
        ("title", Box::new(|m, _, _| m.title.clone())),
        (
            "artist",
            Box::new(|m, _, _| {
                m.credits.iter().map(|c| if c.role == "primary" { artist(c.artist_id) } else { format!("{} ({})", artist(c.artist_id), c.role) }).collect::<Vec<_>>().join(", ")
            }),
        ),
        ("date", Box::new(|m, _, _| m.release_date.clone().unwrap_or_default())),
        ("status", Box::new(|_, f, _| f.status.clone())),
        ("genre", Box::new(|m, _, _| m.genres.join(", "))),
        (
            "album",
            Box::new(|_, f, _| {
                f.albums
                    .iter()
                    .map(|a| match (a.disc_number, a.track_number) {
                        (Some(d), Some(n)) if d > 1 => format!("{} #{d}-{n}", album(a.album_id)),
                        (_, Some(n)) => format!("{} #{n}", album(a.album_id)),
                        _ => album(a.album_id),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        ),
        ("related", Box::new(|_, f, _| f.relations.iter().map(|r| format!("{} “{}”", r.kind.replace('_', " "), track(r.track_id))).collect::<Vec<_>>().join(", "))),
        ("comment", Box::new(|m, _, _| m.comment.clone().unwrap_or_default())),
        (
            "cover",
            Box::new(|_, _, a| match a.covers.len() {
                0 => String::new(),
                1 => "1 cover".into(),
                n => format!("{n} covers"),
            }),
        ),
        ("source", Box::new(|_, _, a| a.sources.iter().map(|s| s.label()).collect::<Vec<_>>().join(", "))),
        ("group", Box::new(|_, _, a| a.groups.iter().map(|g| grouping(*g)).collect::<Vec<_>>().join(", "))),
    ];
    let value_width = (ui.available_width() - 90.0).max(200.0);
    egui::Grid::new("track-editor-state-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        for (key, value) in &fields {
            let before = summary(ready.iter().map(|r| value(&r.base.meta, &r.base.facts, &r.base.attached)));
            let after = summary(ready.iter().map(|r| value(&r.meta, &r.facts, &r.attached)));
            ui.label(RichText::new(format!("{key}:")).color(P.faint));
            ui.vertical(|ui| {
                ui.set_min_width(value_width);
                ui.set_max_width(value_width);
                if before == after {
                    ui.label(RichText::new(&before).color(P.text));
                } else {
                    ui.label(RichText::new(&before).color(P.faint).strikethrough());
                    ui.label(RichText::new(&after).color(P.accent));
                }
            });
            ui.end_row();
        }
    });
}

/// The values many tracks have for a field, in a few words.
fn summary(values: impl Iterator<Item = String>) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut total = 0;
    for v in values {
        total += 1;
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let shown = |v: &str| if v.is_empty() { "—".to_string() } else { v.to_string() };
    match counts.len() {
        0 => "—".into(),
        1 => shown(&counts[0].0),
        2 | 3 => {
            counts.sort_by_key(|c| std::cmp::Reverse(c.1));
            counts.iter().map(|(v, n)| format!("{} ({n})", shown(v))).collect::<Vec<_>>().join(" · ")
        }
        n => format!("{n} different (of {total})"),
    }
}

/// The library's matches for what's typed (artists, albums, tracks), found in the
/// background like the palette's: typing never waits.
#[derive(Default)]
struct Lookup {
    want: Option<(SearchPart, String)>,
    have: Option<(SearchPart, String)>,
    hits: Vec<u32>,
    answer: Arc<Mutex<Option<((SearchPart, String), Vec<u32>)>>>,
}

impl Lookup {
    fn hits(&mut self, shell: &Shell, part: SearchPart, query: &str) -> Vec<u32> {
        if let Some((key, hits)) = self.answer.lock().unwrap().take()
            && Some(&key) == self.want.as_ref()
        {
            self.have = Some(key);
            self.hits = hits;
        }
        let key = (part, query.trim().to_lowercase());
        if key.1.is_empty() {
            return Vec::new();
        }
        if self.want.as_ref() != Some(&key) {
            self.want = Some(key.clone());
            if let Some(catalog) = shell.catalog.value.clone() {
                let (answer, ctx) = (self.answer.clone(), shell.ctx.clone());
                shell.runtime.spawn_blocking(move || {
                    let hits = Search::default().run_part(&catalog, &key.1, key.0, 8).into_iter().map(|h| h.0).collect();
                    *answer.lock().unwrap() = Some((key, hits));
                    ctx.request_repaint();
                });
            }
        }
        // Until the new answer: the last one, if it was for the same kind of thing.
        match &self.have {
            Some((p, _)) if *p == part => self.hits.clone(),
            _ => Vec::new(),
        }
    }

    /// The matches for exactly `query`, found now if they aren't yet.
    fn now(&mut self, catalog: &Catalog, part: SearchPart, query: &str) -> Vec<u32> {
        let key = (part, query.trim().to_lowercase());
        if key.1.is_empty() {
            return Vec::new();
        }
        if self.have.as_ref() != Some(&key) {
            self.hits = Search::default().run_part(catalog, &key.1, part, 8).into_iter().map(|h| h.0).collect();
            self.have = Some(key.clone());
            self.want = Some(key);
        }
        self.hits.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{keys_free, Verb};
    use crate::catalog::query::loosely_in;

    /// In the bulk editor's popup the keys are the editor's (Ctrl+Enter, the check's), until
    /// another popup opens over it.
    #[test]
    fn keys_in_a_popup_are_its_own() {
        use eframe::egui;
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(1200.0, 900.0));
        let mut layer = None;
        let frame = |over: bool, layer: &mut Option<egui::LayerId>| {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                crate::ui::modal::show(ui.ctx(), "bulk", "Change 3 tracks", egui::Vec2::new(600.0, 400.0), |ui| *layer = Some(ui.layer_id()));
                if over {
                    crate::ui::modal::show(ui.ctx(), "original", "Original", egui::Vec2::new(300.0, 200.0), |_| {});
                }
            });
            // No renderer here: the textures it would upload are let go.
            output.textures_delta.clear();
        };
        frame(false, &mut layer);
        frame(false, &mut layer);
        let layer = layer.unwrap();
        assert!(keys_free(&ctx, layer, false, true), "its own popup");
        assert!(!keys_free(&ctx, layer, true, true), "the palette is over it");
        frame(true, &mut Some(layer));
        assert!(!keys_free(&ctx, layer, false, true), "another popup over it");
        // On a page: any popup takes them.
        assert!(!keys_free(&ctx, egui::LayerId::background(), false, true));
        assert!(keys_free(&ctx, egui::LayerId::background(), false, false));
    }

    /// Plain words find the tools as the line lists them (by their label).
    #[test]
    fn words_find_the_tools() {
        let all = [Verb::MergeTwo, Verb::CreditedAs, Verb::MarkOriginal];
        let found = |typed: &str| all.iter().copied().filter(|v| loosely_in(typed, v.label())).collect::<Vec<_>>();
        assert_eq!(found("merge"), [Verb::MergeTwo]);
        assert_eq!(found("credited"), [Verb::CreditedAs]);
        assert!(found("og").contains(&Verb::MarkOriginal));
    }
}
