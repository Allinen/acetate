//! Editing a track (the track page in edit mode, and the Review page): the shown
//! variant's metadata (title, release date, genres, comment, credits), the track's facts
//! (status, albums, relations), and a new cover. Metadata and facts are saved apart
//! because the server keeps them apart: metadata is a variant's
//! (`PUT /api/tracks/{id}/variants/{vid}`), facts are the track's (`PUT …/facts`). A
//! save sends only the parts that changed, each with the revision the form was filled
//! from, so a change someone made in between is never silently undone: the server says
//! `stale` instead, and "Load theirs, keep mine" takes their version and puts back only
//! the fields you changed.
//!
//! Artists and albums are typed in place (matches from the library as you type). New
//! ones (an artist not in the library, a new single or album) and a pasted cover are
//! kept in the form and made when it's saved, in order: artists, albums (credited to the
//! track's primary artists), the metadata, the facts, the cover.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Key, Modifiers, RichText};
use serde::Deserialize;
use uuid::Uuid;

use super::credits::{self, CreditsEditor};
use super::inline_search::{Choice, InlineSearch};
use super::{capitalized, combo, PickKind, Picked, SaveState};
use crate::api::{Api, ApiError};
use crate::catalog::model::Cover;
use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::theme::{bold, font, PALETTE as P};
use crate::ui::{Action, Shell};

/// The fields other pages can put the cursor in (the Review page: the first one that
/// needs fixing).
pub const TITLE_ID: &str = "form-title";
pub const DATE_ID: &str = "form-date";
pub const ARTIST_ID: &str = "form-artist";
pub const ALBUM_ID: &str = "form-album";

/// The variant's metadata, as the form edits it (text as typed).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Meta {
    pub title: String,
    pub release_date: String,
    /// Comma-separated.
    pub genres: String,
    pub comment: String,
    pub credits: Vec<CreditRow>,
}

pub use super::credits::CreditRow;

/// The track's facts, as the form edits them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    pub status: String,
    pub albums: Vec<AlbumRow>,
    pub relations: Vec<RelationRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRow {
    /// None: a new album, made when the form is saved.
    pub album_id: Option<Uuid>,
    pub title: String,
    /// A new album's kind ("single", "album"…); None for existing ones.
    pub new_kind: Option<String>,
    /// As typed: empty means none.
    pub disc: String,
    pub number: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationRow {
    pub kind: String,
    pub track_id: Uuid,
    pub title: String,
}

/// A cover to add, from the clipboard or a dropped file: uploaded when the form is saved.
pub struct NewCover {
    bytes: Arc<Vec<u8>>,
    file_name: String,
    preview: egui::TextureHandle,
    /// On the track's first album (its tracks show it too), or on the track itself.
    on_album: bool,
}

/// Which row a picker's answer goes to (None: a new row).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Credit(Option<usize>),
    Album(Option<usize>),
    Relation(Option<usize>),
}

impl Target {
    fn kind(self) -> PickKind {
        match self {
            Target::Credit(_) => PickKind::Artist,
            Target::Album(_) => PickKind::Album,
            Target::Relation(_) => PickKind::Track,
        }
    }
}

/// How a save went, from the background: what was sent (with the new artists' and
/// albums' ids filled in), which parts were written (a part written before another
/// failed still counts), and the error, if any.
#[derive(Default)]
struct Outcome {
    sent: Option<(Meta, Facts)>,
    meta: Option<Uuid>,
    facts: Option<Uuid>,
    cover: bool,
    error: Option<ApiError>,
}

pub struct TrackForm {
    pub track: Uuid,
    variant: Uuid,
    /// The revisions the form was filled from (sent with a save).
    variant_base: Uuid,
    facts_base: Option<Uuid>,
    /// The details' revisions last looked at: when they change, the form follows (or,
    /// with changes of yours, says someone else changed the track).
    seen: (Uuid, Option<Uuid>),
    pub meta: Meta,
    pub facts: Facts,
    /// As filled from the server: what "changed" is measured against.
    meta_was: Meta,
    facts_was: Facts,
    pub cover: Option<NewCover>,
    reason: String,
    awaiting: Option<Target>,
    state: SaveState,
    /// A save finished without errors (taken by `take_saved`).
    saved: bool,
    answer: Arc<Mutex<Option<Outcome>>>,
    /// Esc was pressed with changes: a second Esc discards them.
    confirm_discard: bool,
    /// "Load theirs" was asked, and waits for the details to arrive.
    rebase_asked: bool,
    notice: Option<String>,
    credits_editor: CreditsEditor,
    album_search: InlineSearch,
    /// Put the cursor in this album row's track number (a new album was just added).
    focus_number: Option<usize>,
}

/// A variant's data as the server keeps it (only what the form edits).
#[derive(Deserialize, Default)]
#[serde(default)]
struct MetaData {
    title: String,
    release_date: Option<String>,
    genres: Vec<String>,
    comment: Option<String>,
    credits: Vec<CreditData>,
}

#[derive(Deserialize)]
struct CreditData {
    artist_id: Uuid,
    role: String,
    #[serde(default)]
    alias_id: Option<Uuid>,
}

#[derive(Deserialize)]
struct Written {
    revision_id: Uuid,
}

#[derive(Deserialize)]
struct Created {
    id: Uuid,
}

impl TrackForm {
    /// Filled from the track's details (its shown variant). None if it has no variant.
    pub fn from_detail(d: &TrackDetail, catalog: Option<&Catalog>) -> Option<TrackForm> {
        TrackForm::for_variant(d, catalog, None)
    }

    /// Filled from the track's details, editing the variant `variant` (None, or one the
    /// details don't have: the shown one).
    pub fn for_variant(d: &TrackDetail, catalog: Option<&Catalog>, variant: Option<Uuid>) -> Option<TrackForm> {
        let variant = variant
            .and_then(|id| d.variants.iter().find(|v| v.id == id))
            .or_else(|| d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)))
            .or(d.variants.first())?;
        let data: MetaData = serde_json::from_value(variant.data.clone()).unwrap_or_default();
        let credits = data
            .credits
            .into_iter()
            .map(|c| {
                let known = d.credits.iter().find(|k| k.artist_id == c.artist_id);
                let name = catalog
                    .and_then(|cat| cat.artist(c.artist_id))
                    .map(|a| a.name.clone())
                    .or_else(|| known.map(|k| k.artist_name.clone()))
                    .unwrap_or_else(|| "(an artist not in the library)".into());
                let alias = known.filter(|k| k.alias_id.is_some() && k.alias_id == c.alias_id).and_then(|k| k.alias.clone());
                CreditRow { artist_id: Some(c.artist_id), name, role: c.role, alias_id: c.alias_id, alias }
            })
            .collect();
        let meta = Meta {
            title: data.title,
            release_date: data.release_date.unwrap_or_default(),
            genres: data.genres.join(", "),
            comment: data.comment.unwrap_or_default(),
            credits,
        };
        let number = |n: Option<i16>| n.map(|n| n.to_string()).unwrap_or_default();
        let facts = Facts {
            status: d.status.clone(),
            albums: d
                .albums
                .iter()
                .map(|a| AlbumRow {
                    album_id: Some(a.album_id),
                    title: a.title.clone(),
                    new_kind: None,
                    disc: number(a.disc_number),
                    number: number(a.track_number),
                })
                .collect(),
            relations: d.relations.iter().map(|r| RelationRow { kind: r.kind.clone(), track_id: r.track_id, title: r.title.clone() }).collect(),
        };
        Some(TrackForm {
            track: d.id,
            variant: variant.id,
            variant_base: variant.revision_id,
            facts_base: d.external_revision_id,
            seen: (variant.revision_id, d.external_revision_id),
            meta_was: meta.clone(),
            facts_was: facts.clone(),
            meta,
            facts,
            cover: None,
            reason: String::new(),
            awaiting: None,
            state: SaveState::Idle,
            saved: false,
            answer: Arc::default(),
            confirm_discard: false,
            rebase_asked: false,
            notice: None,
            credits_editor: CreditsEditor::default(),
            album_search: InlineSearch::new(Part::Albums),
            focus_number: None,
        })
    }

    /// The variant being edited.
    pub fn variant(&self) -> Uuid {
        self.variant
    }

    /// Edits `variant` instead (its latest revision), keeping what's typed but not part
    /// of the track yet (the reason, a new cover). Only without changes: a changed form
    /// is saved or discarded first.
    pub fn switch_to(&mut self, d: &TrackDetail, catalog: Option<&Catalog>, variant: Uuid) {
        if self.fields_changed() || self.saving() {
            return;
        }
        if let Some(fresh) = TrackForm::for_variant(d, catalog, Some(variant)) {
            self.replace_keeping_input(fresh);
        }
    }

    pub fn dirty(&self) -> bool {
        self.meta != self.meta_was || self.facts != self.facts_was || self.cover.is_some()
    }

    /// Changes in the fields (a new cover aside: it's the track's, whichever variant).
    pub fn fields_changed(&self) -> bool {
        self.meta != self.meta_was || self.facts != self.facts_was
    }

    pub fn saving(&self) -> bool {
        matches!(self.state, SaveState::Saving)
    }

    /// A save finished without errors since this was last asked.
    pub fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    /// Puts `text` (a tag from the file) in the artist field, searching for it, with the
    /// cursor there: Enter credits the match.
    pub fn suggest_artist(&mut self, text: &str, shell: &Shell) {
        self.credits_editor.search.set_query(text, shell);
        shell.ctx.memory_mut(|m| m.request_focus(egui::Id::new(ARTIST_ID)));
    }

    /// Like `suggest_artist`, for the album field.
    pub fn suggest_album(&mut self, text: &str, shell: &Shell) {
        self.album_search.set_query(text, shell);
        shell.ctx.memory_mut(|m| m.request_focus(egui::Id::new(ALBUM_ID)));
    }

    /// Credits `to` where `from` is credited now (same role; the "credited as" is
    /// cleared: an alias is one artist's). False if `from` isn't credited.
    pub fn replace_artist(&mut self, from: Uuid, to: Uuid, name: String) -> bool {
        let mut replaced = false;
        for row in self.meta.credits.iter_mut().filter(|r| r.artist_id == Some(from)) {
            *row = CreditRow { artist_id: Some(to), name: name.clone(), alias_id: None, alias: None, ..row.clone() };
            replaced = true;
        }
        replaced
    }

    /// Whether this artist is credited (as the form has it).
    pub fn credits_artist(&self, id: Uuid) -> bool {
        self.meta.credits.iter().any(|r| r.artist_id == Some(id))
    }

    /// The field that fixes a missing-something flag (its kind), if there's one.
    pub fn field_for(kind: &str) -> Option<&'static str> {
        match kind {
            "missing_title" | "credits_in_title" => Some(TITLE_ID),
            "missing_artists" => Some(ARTIST_ID),
            "missing_album" => Some(ALBUM_ID),
            "missing_date" => Some(DATE_ID),
            _ => None,
        }
    }

    /// Takes `theirs` (the form filled from newer details) as the base, with the fields
    /// you changed put back on top. A field you left alone becomes theirs; a list you
    /// changed (credits, albums, relations) stays yours as a whole.
    pub fn rebase(&mut self, theirs: TrackForm) {
        fn pick<T: Clone + PartialEq>(mine: &T, was: &T, theirs: &T) -> T {
            if mine != was { mine.clone() } else { theirs.clone() }
        }
        let (m, w, t) = (&self.meta, &self.meta_was, &theirs.meta);
        let meta = Meta {
            title: pick(&m.title, &w.title, &t.title),
            release_date: pick(&m.release_date, &w.release_date, &t.release_date),
            genres: pick(&m.genres, &w.genres, &t.genres),
            comment: pick(&m.comment, &w.comment, &t.comment),
            credits: pick(&m.credits, &w.credits, &t.credits),
        };
        let (m, w, t) = (&self.facts, &self.facts_was, &theirs.facts);
        let facts = Facts {
            status: pick(&m.status, &w.status, &t.status),
            albums: pick(&m.albums, &w.albums, &t.albums),
            relations: pick(&m.relations, &w.relations, &t.relations),
        };
        let fresh = TrackForm { meta, facts, ..theirs };
        self.replace_keeping_input(fresh);
        self.notice = Some("Their changes are loaded, with yours on top. Check them, then save.".into());
    }

    /// Becomes `fresh`, keeping what's typed but not part of the track yet: the reason,
    /// the search fields, a new cover.
    fn replace_keeping_input(&mut self, fresh: TrackForm) {
        let reason = std::mem::take(&mut self.reason);
        let cover = self.cover.take();
        let credits_editor = std::mem::take(&mut self.credits_editor);
        let album_search = std::mem::replace(&mut self.album_search, InlineSearch::new(Part::Albums));
        *self = TrackForm { reason, cover, credits_editor, album_search, ..fresh };
    }

    /// Follows the details: a form without changes shows the newest; one with changes
    /// keeps them (and, if "Load theirs" was asked, is rebased now).
    fn follow(&mut self, d: &TrackDetail, catalog: Option<&Catalog>) {
        let Some(fresh) = TrackForm::for_variant(d, catalog, Some(self.variant)) else { return };
        if fresh.seen == self.seen || self.saving() {
            return;
        }
        self.seen = fresh.seen;
        let ours = (fresh.variant, fresh.variant_base, fresh.facts_base) == (self.variant, self.variant_base, self.facts_base);
        let unchanged = self.meta == self.meta_was && self.facts == self.facts_was;
        if unchanged {
            // Our own save come back (only names may be newer), or someone else's.
            let state = std::mem::take(&mut self.state);
            let saved = self.saved;
            self.replace_keeping_input(fresh);
            self.state = state;
            self.saved = saved;
        } else if !ours && (self.rebase_asked || fresh.variant != self.variant) {
            self.rebase(fresh);
        }
    }

    /// Someone else changed the track since the form was filled (as far as the details
    /// kept here know).
    fn behind(&self, d: &TrackDetail) -> bool {
        let variant = d.variants.iter().find(|v| v.id == self.variant).map(|v| v.revision_id);
        variant != Some(self.variant_base) || d.external_revision_id != self.facts_base
    }

    fn discard(&mut self) {
        self.meta = self.meta_was.clone();
        self.facts = self.facts_was.clone();
        self.cover = None;
        self.reason.clear();
        self.confirm_discard = false;
        self.state = SaveState::Idle;
        self.notice = None;
    }

    /// The variant's new data, as the server takes it.
    #[cfg(test)]
    fn meta_body(&self) -> serde_json::Value {
        meta_body(&self.meta, self.variant_base, &self.reason)
    }

    #[cfg(test)]
    fn facts_body(&self) -> Result<serde_json::Value, String> {
        let base = self.facts_base.ok_or("This track has no facts revision to build on yet, so they can't be edited here.")?;
        facts_body(&self.facts, base, &self.reason)
    }

    /// Saves what changed, in the background: new artists, new albums, the metadata, the
    /// facts, the cover, in that order (each needs the ones before).
    pub fn save(&mut self, shell: &Shell) {
        if self.saving() || !self.dirty() {
            return;
        }
        if self.meta.title.trim().is_empty() {
            self.state = SaveState::Failed("A track needs a title.".into());
            return;
        }
        let meta_changed = self.meta != self.meta_was;
        let facts_changed = self.facts != self.facts_was;
        let facts_base = match (facts_changed, self.facts_base) {
            (true, None) => {
                self.state = SaveState::Failed("This track has no facts revision to build on yet, so its albums can't be edited here.".into());
                return;
            }
            (_, base) => base,
        };
        // Numbers are checked now, not halfway through.
        if let Err(err) = check_numbers(&self.facts) {
            self.state = SaveState::Failed(err);
            return;
        }
        self.state = SaveState::Saving;
        self.confirm_discard = false;
        self.notice = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let (track, variant, variant_base, reason) = (self.track, self.variant, self.variant_base, self.reason.trim().to_string());
        let (meta, facts) = (self.meta.clone(), self.facts.clone());
        let cover = self.cover.as_ref().map(|c| (c.bytes.clone(), c.file_name.clone(), c.on_album));
        shell.runtime.spawn(async move {
            let outcome = save(&api, track, variant, variant_base, facts_base, reason, meta, facts, meta_changed, facts_changed, cover).await;
            *answer.lock().unwrap() = Some(outcome);
            ctx.request_repaint();
        });
    }

    /// Takes in how a save went.
    fn poll(&mut self, shell: &mut Shell) {
        let Some(outcome) = self.answer.lock().unwrap().take() else { return };
        // The new artists and albums have ids now (even if a later step failed): the
        // form's rows get them, so saving again doesn't make them twice.
        if let Some((sent_meta, sent_facts)) = &outcome.sent {
            credits::take_ids(&mut self.meta.credits, &sent_meta.credits);
            for row in self.facts.albums.iter_mut().filter(|r| r.album_id.is_none()) {
                if let Some(made) = sent_facts.albums.iter().find(|s| s.title == row.title && s.new_kind == row.new_kind) {
                    row.album_id = made.album_id;
                    if made.album_id.is_some() {
                        row.new_kind = None;
                    }
                }
            }
            if let Some(revision) = outcome.meta {
                self.variant_base = revision;
                self.meta_was = sent_meta.clone();
            }
            if let Some(revision) = outcome.facts {
                self.facts_base = Some(revision);
                let mut was = sent_facts.clone();
                was.albums.iter_mut().for_each(|a| a.new_kind = None);
                self.facts_was = was;
            }
        }
        if outcome.cover {
            self.cover = None;
        }
        self.state = match outcome.error {
            None => {
                self.reason.clear();
                self.saved = true;
                SaveState::Saved(Instant::now())
            }
            Some(err) if err.code == "stale" => SaveState::Stale(err.message),
            Some(err) => SaveState::Failed(err.message),
        };
        // The details (and lists) catch up: on a stale save, that's their version.
        shell.track_details.refresh(self.track);
        shell.act(Action::RefreshLibrary);
    }

    /// A pick arrived for the row that asked.
    fn take_pick(&mut self, picked: Picked, shell: &Shell) {
        let Some(target) = self.awaiting.filter(|t| t.kind() == picked.kind) else { return };
        self.awaiting = None;
        match target {
            Target::Credit(Some(i)) => {
                if let Some(row) = self.meta.credits.get_mut(i) {
                    *row = CreditRow { artist_id: picked.id, name: picked.name, alias_id: None, alias: None, ..row.clone() };
                }
            }
            Target::Credit(None) => self.add_artist(picked.id, picked.name, false),
            Target::Album(Some(i)) => {
                if let Some(row) = self.facts.albums.get_mut(i) {
                    row.album_id = picked.id;
                    row.title = picked.name;
                    row.new_kind = picked.id.is_none().then(|| "album".to_string());
                }
            }
            Target::Album(None) => self.add_album(picked.id, picked.name, picked.id.is_none().then(|| "album".to_string())),
            Target::Relation(Some(i)) => {
                if let (Some(row), Some(id)) = (self.facts.relations.get_mut(i), picked.id) {
                    row.track_id = id;
                    row.title = picked.name;
                }
            }
            Target::Relation(None) => {
                let Some(id) = picked.id else { return };
                let kind = shell.account.vocab.as_ref().and_then(|v| v.relation_kinds.first()).map(|k| k.kind.clone()).unwrap_or_default();
                self.facts.relations.push(RelationRow { kind, track_id: id, title: picked.name });
            }
        }
    }

    /// Credits an artist (None: a new one), see `credits::add_artist`.
    fn add_artist(&mut self, id: Option<Uuid>, name: String, featured: bool) {
        credits::add_artist(&mut self.meta.credits, id, name, featured);
    }

    /// Puts the track on an album (None: a new one of `new_kind`). A single gets disc 1,
    /// track 1; otherwise the track number is asked next.
    fn add_album(&mut self, id: Option<Uuid>, title: String, new_kind: Option<String>) {
        if id.is_some() && self.facts.albums.iter().any(|a| a.album_id == id) {
            return;
        }
        let single = new_kind.as_deref() == Some("single");
        let (disc, number) = if single { ("1".into(), "1".into()) } else { (String::new(), String::new()) };
        self.facts.albums.push(AlbumRow { album_id: id, title, new_kind, disc, number });
        if !single {
            self.focus_number = Some(self.facts.albums.len() - 1);
        }
    }

    fn ask(&mut self, shell: &mut Shell, target: Target) {
        self.awaiting = Some(target);
        // A track isn't related to itself.
        let except = matches!(target, Target::Relation(_)).then_some(self.track);
        shell.act(Action::Pick { kind: target.kind(), except });
    }

    /// Takes a cover from the clipboard (Ctrl+V with an image in it) or a file dropped on
    /// the window.
    fn take_cover(&mut self, ctx: &egui::Context) {
        let pasted = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)) || matches!(e, egui::Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command))
        });
        let mut found: Option<(Vec<u8>, String)> = None;
        if pasted {
            // Text in the clipboard is for the text field; only an image is taken here.
            if let Ok(image) = arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
                let rgba = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned());
                let mut png = Vec::new();
                if let Some(rgba) = rgba
                    && rgba.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok()
                {
                    found = Some((png, "pasted.png".into()));
                }
            }
        }
        for file in ctx.input(|i| i.raw.dropped_files.clone()) {
            let name = file.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match file.bytes() {
                Ok(bytes) => found = Some((bytes, name)),
                Err(err) => self.state = SaveState::Failed(format!("Couldn't read “{name}”: {err}")),
            }
        }
        let Some((bytes, file_name)) = found else { return };
        match image::load_from_memory(&bytes) {
            Ok(decoded) => {
                let thumb = decoded.thumbnail(320, 320).to_rgba8();
                let size = [thumb.width() as usize, thumb.height() as usize];
                let preview = ctx.load_texture("new-cover", egui::ColorImage::from_rgba_unmultiplied(size, &thumb), Default::default());
                let on_album = !self.facts.albums.is_empty();
                self.cover = Some(NewCover { bytes: Arc::new(bytes), file_name, preview, on_album });
                self.state = SaveState::Idle;
            }
            Err(_) => self.state = SaveState::Failed(format!("“{file_name}” isn't an image this can read (JPEG, PNG, WebP or GIF).")),
        }
    }

    /// The form. Ctrl+Enter saves; Esc (outside text fields) with changes asks, a second
    /// Esc discards them. `cover`: the cover the track shows now.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: Option<&Catalog>, d: &TrackDetail, cover: Option<Cover>) {
        self.poll(shell);
        self.follow(d, catalog);
        if let Some(picked) = shell.picker.take() {
            self.take_pick(picked, shell);
        }
        let ctx = ui.ctx().clone();
        if !shell.palette_open {
            self.take_cover(&ctx);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            self.save(shell);
        }
        if !shell.palette_open && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)) && self.dirty() {
            if self.confirm_discard {
                self.discard();
            } else {
                self.confirm_discard = true;
            }
        }

        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let before = (self.meta.clone(), self.facts.clone(), self.cover.is_some());

        let label = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(P.faint));
        };
        let text = |ui: &mut egui::Ui, id: &str, value: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(value).id(egui::Id::new(id)).hint_text(hint).desired_width(420.0));
        };

        // ---- The variant's metadata ----
        let title_was = self.meta.title.clone();
        egui::Grid::new("track-form-meta").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
            label(ui, "Title");
            text(ui, TITLE_ID, &mut self.meta.title, "");
            ui.end_row();
            label(ui, "Released");
            text(ui, DATE_ID, &mut self.meta.release_date, "as the source wrote it: 2019, 2019-05-12…");
            ui.end_row();
            label(ui, "Genres");
            text(ui, "form-genres", &mut self.meta.genres, "comma-separated");
            ui.end_row();
            label(ui, "Comment");
            text(ui, "form-comment", &mut self.meta.comment, "");
            ui.end_row();
        });
        // A new single is named after the track: it follows the title.
        if self.meta.title != title_was {
            for row in self.facts.albums.iter_mut().filter(|a| a.new_kind.as_deref() == Some("single") && a.title == title_was) {
                row.title = self.meta.title.clone();
            }
        }

        ui.add_space(10.0);
        heading(ui, "Artists");
        let field = egui::Id::new(ARTIST_ID);
        if let Some(i) = self.credits_editor.show(ui, shell, catalog, "track-form", field, &mut self.meta.credits, &vocab.credit_roles) {
            self.ask(shell, Target::Credit(Some(i)));
        }
        let mut action = None;

        // ---- The track's facts ----
        ui.add_space(14.0);
        heading(ui, "Albums and status");
        ui.horizontal(|ui| {
            label(ui, "Status");
            combo(ui, "status", &mut self.facts.status, &vocab.statuses, capitalized);
        });
        ui.add_space(4.0);
        let focus_number = self.focus_number.take();
        egui::Grid::new("track-form-albums").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            for (i, row) in self.facts.albums.iter_mut().enumerate() {
                match &mut row.new_kind {
                    // A new one: its title and kind are still to choose.
                    Some(kind) => {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut row.title).desired_width(220.0).hint_text("title"));
                            combo(ui, ("album-kind", i), kind, &vocab.album_kinds, |k| format!("new {k}"));
                        });
                    }
                    None => {
                        if ui.button(&row.title).on_hover_text("Choose another album").clicked() {
                            action = Some(RowAction::Pick(Target::Album(Some(i))));
                        }
                    }
                }
                ui.horizontal(|ui| {
                    label(ui, "disc");
                    ui.add(egui::TextEdit::singleline(&mut row.disc).desired_width(36.0));
                    label(ui, "track");
                    let number = ui.add(egui::TextEdit::singleline(&mut row.number).desired_width(36.0));
                    if focus_number == Some(i) {
                        number.request_focus();
                    }
                });
                if ui.small_button("×").on_hover_text("Take it off this album").clicked() {
                    action = Some(RowAction::Remove(i));
                }
                ui.end_row();
            }
        });
        let typed = self.album_search.query.trim().to_string();
        let title = self.meta.title.trim().to_string();
        let mut extras = vec![format!("New single “{title}” (disc 1, track 1)")];
        if !typed.is_empty() {
            extras.push(format!("New album “{typed}”"));
        }
        let hint = "Add to an album: type its title";
        match self.album_search.show(ui, shell, catalog, egui::Id::new(ALBUM_ID), hint, &extras) {
            Some((Choice::Hit(i), _)) => {
                if let Some(a) = catalog.and_then(|c| c.albums.get(i as usize)) {
                    self.add_album(Some(a.id), a.title.clone(), None);
                }
            }
            Some((Choice::Extra(0), _)) => self.add_album(None, title, Some("single".into())),
            Some((Choice::Extra(_), _)) => self.add_album(None, typed, Some("album".into())),
            None => {}
        }
        // Without typing, a new single is a button (the field lists it only while typing).
        let offer_single = self.album_search.query.trim().is_empty() && !self.facts.albums.iter().any(|a| a.new_kind.is_some());
        let tip = "An album of kind single, named after the track, with its artists; the track is disc 1, track 1";
        if offer_single && ui.button("New single from this track").on_hover_text(tip).clicked() {
            self.add_album(None, self.meta.title.trim().to_string(), Some("single".into()));
        }
        match action.take() {
            Some(RowAction::Pick(target)) => self.ask(shell, target),
            Some(RowAction::Remove(i)) => {
                self.facts.albums.remove(i);
            }
            _ => {}
        }

        // ---- Cover ----
        ui.add_space(14.0);
        heading(ui, "Cover");
        ui.horizontal(|ui| {
            let size = egui::Vec2::splat(96.0);
            match &self.cover {
                Some(new) => {
                    ui.add(egui::Image::from_texture(&new.preview).fit_to_exact_size(size).corner_radius(6));
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    shell.cover(ui, cover, self.track, rect, 6, 0.0);
                }
            }
            ui.vertical(|ui| {
                match (&mut self.cover, cover) {
                    (Some(new), _) => {
                        ui.label(RichText::new(format!("New: {} (added when saved)", new.file_name)).color(P.text));
                        if let Some(album) = self.facts.albums.first() {
                            ui.checkbox(&mut new.on_album, format!("On the album “{}” (its tracks show it too)", album.title));
                        } else {
                            new.on_album = false;
                        }
                        if ui.small_button("Don't add it").clicked() {
                            self.cover = None;
                        }
                    }
                    (None, Some(_)) => {
                        ui.label(RichText::new("Ctrl+V pastes an image to add, or drop an image file here.").color(P.faint));
                    }
                    (None, None) => {
                        ui.label(RichText::new("No cover. Ctrl+V pastes an image, or drop an image file here.").color(P.muted));
                    }
                }
            });
        });

        if shell.account.can("edit_relations") {
            ui.add_space(14.0);
            heading(ui, "Related tracks");
            egui::Grid::new("track-form-relations").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                for (i, row) in self.facts.relations.iter_mut().enumerate() {
                    // Each kind as the sentence it makes, in its direction ("This track
                    // is a remix of …" or "… is a remix of this track"); the chosen one
                    // with the other track's title in it.
                    let sentence = |kind: &str, other: &str| match vocab.relation_kinds.iter().find(|k| k.kind == kind) {
                        Some(k) => relation_sentence(&k.template, other),
                        None => format!("{} {other}", kind.replace('_', " ")),
                    };
                    let shown = sentence(&row.kind, &format!("“{}”", row.title));
                    egui::ComboBox::from_id_salt(("relation-kind", i)).selected_text(shown).width(420.0).show_ui(ui, |ui| {
                        for k in &vocab.relation_kinds {
                            if ui.selectable_label(row.kind == k.kind, sentence(&k.kind, "…")).clicked() {
                                row.kind = k.kind.clone();
                            }
                        }
                    });
                    if ui.button("Other track…").on_hover_text("Choose another track").clicked() {
                        action = Some(RowAction::Pick(Target::Relation(Some(i))));
                    }
                    if ui.small_button("×").on_hover_text("Remove").clicked() {
                        action = Some(RowAction::Remove(i));
                    }
                    ui.end_row();
                }
            });
            if ui.button("+ Add a related track").clicked() {
                action = Some(RowAction::Pick(Target::Relation(None)));
            }
            match action.take() {
                Some(RowAction::Pick(target)) => self.ask(shell, target),
                Some(RowAction::Remove(i)) => {
                    self.facts.relations.remove(i);
                }
                _ => {}
            }
        }

        if (self.meta.clone(), self.facts.clone(), self.cover.is_some()) != before {
            self.confirm_discard = false;
            if !self.saving() {
                self.state = SaveState::Idle;
            }
        }

        // ---- Saving ----
        ui.add_space(14.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with the change)").desired_width(560.0));
        ui.add_space(6.0);
        let dirty = self.dirty();
        ui.horizontal(|ui| {
            let saving = self.saving();
            if ui.add_enabled(dirty && !saving, egui::Button::new("Save  (Ctrl+Enter)")).clicked() {
                self.save(shell);
            }
            if ui.add_enabled(dirty && !saving, egui::Button::new("Discard")).clicked() {
                self.discard();
            }
            let (text, color) = match &self.state {
                SaveState::Saving => ("Saving…".to_string(), P.muted),
                SaveState::Saved(at) if at.elapsed().as_secs() < 5 && !dirty => ("Saved".to_string(), P.accent),
                SaveState::Failed(err) => (err.clone(), P.danger),
                SaveState::Stale(err) => (err.clone(), P.danger),
                _ if self.confirm_discard => ("Discard your changes? Esc again to discard them.".to_string(), P.danger),
                _ if dirty => ("Changed, not saved".to_string(), P.muted),
                _ => (String::new(), P.muted),
            };
            ui.label(RichText::new(text).color(color));
        });
        if let SaveState::Saved(at) = self.state
            && at.elapsed().as_secs() < 5
        {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(5));
        }
        // Someone else changed it: offer to take theirs with yours on top.
        let stale = matches!(self.state, SaveState::Stale(_));
        if dirty && (stale || self.behind(d)) {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Someone else changed this track since you started.").color(P.muted));
                if self.rebase_asked {
                    ui.label(RichText::new("Loading theirs…").color(P.faint));
                } else if ui.button("Load theirs, keep mine on top").clicked() {
                    if self.behind(d) {
                        if let Some(fresh) = TrackForm::from_detail(d, catalog) {
                            self.rebase(fresh);
                        }
                    } else {
                        self.rebase_asked = true;
                        shell.track_details.refresh(self.track);
                    }
                }
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted));
        }
        shell.form_dirty = self.dirty();
    }
}

/// The saving itself (see `TrackForm::save`), step by step; stops at the first error.
#[allow(clippy::too_many_arguments)]
async fn save(
    api: &Api,
    track: Uuid,
    variant: Uuid,
    variant_base: Uuid,
    facts_base: Option<Uuid>,
    reason: String,
    mut meta: Meta,
    mut facts: Facts,
    meta_changed: bool,
    facts_changed: bool,
    cover: Option<(Arc<Vec<u8>>, String, bool)>,
) -> Outcome {
    let mut outcome = Outcome::default();
    let result: Result<(), ApiError> = async {
        // New artists. What's made so far is kept in the outcome: on an error later, the
        // form still learns the new ids and won't make them again.
        let made = credits::make_new_artists(api, &mut meta.credits).await;
        outcome.sent = Some((meta.clone(), facts.clone()));
        made?;
        // New albums, credited to the track's primary artists; a single gets its date.
        let primary: Vec<serde_json::Value> = meta
            .credits
            .iter()
            .filter(|c| c.role == "primary")
            .map(|c| serde_json::json!({ "artist_id": c.artist_id, "role": "primary", "alias_id": c.alias_id }))
            .collect();
        for i in 0..facts.albums.len() {
            if facts.albums[i].album_id.is_some() {
                continue;
            }
            let row = &facts.albums[i];
            let kind = row.new_kind.clone().unwrap_or_else(|| "album".into());
            let date = Some(meta.release_date.trim()).filter(|d| !d.is_empty() && kind == "single");
            let body = serde_json::json!({ "title": row.title.trim(), "kind": kind, "release_date": date, "credits": primary });
            let created: Created = api.send_json(reqwest::Method::POST, "/api/albums", Some(body)).await?;
            facts.albums[i].album_id = Some(created.id);
            outcome.sent = Some((meta.clone(), facts.clone()));
        }
        outcome.sent = Some((meta.clone(), facts.clone()));
        if meta_changed {
            let body = meta_body(&meta, variant_base, &reason);
            let written: Written = api.send_json(reqwest::Method::PUT, &format!("/api/tracks/{track}/variants/{variant}"), Some(body)).await?;
            outcome.meta = Some(written.revision_id);
        }
        if let (true, Some(base)) = (facts_changed, facts_base) {
            let body = facts_body(&facts, base, &reason).map_err(|message| ApiError { status: None, code: "invalid".into(), message })?;
            let written: Written = api.send_json(reqwest::Method::PUT, &format!("/api/tracks/{track}/facts"), Some(body)).await?;
            outcome.facts = Some(written.revision_id);
        }
        if let Some((bytes, file_name, on_album)) = cover {
            let album = facts.albums.first().and_then(|a| a.album_id).filter(|_| on_album);
            let path = match album {
                Some(album) => format!("/api/albums/{album}/covers"),
                None => format!("/api/tracks/{track}/covers"),
            };
            let _: Created = api.upload(&path, (*bytes).clone(), &file_name).await?;
            outcome.cover = true;
        }
        Ok(())
    }
    .await;
    outcome.error = result.err();
    outcome
}

/// The variant's new data, as the server takes it.
fn meta_body(m: &Meta, base: Uuid, reason: &str) -> serde_json::Value {
    let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    let genres: Vec<String> = m.genres.split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect();
    let credits = credits::to_json(&m.credits);
    serde_json::json!({
        "base_revision_id": base,
        "reason": reason.trim(),
        "data": {
            "title": m.title.trim(),
            "release_date": text(&m.release_date),
            "genres": genres,
            "comment": text(&m.comment),
            "credits": credits,
        },
    })
}

/// A disc or track number as typed: empty is none; anything else must be a number.
fn number(text: &str) -> Result<Option<i16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<i16>().map(Some).map_err(|_| format!("Disc and track numbers are whole numbers (“{text}” isn't one)."))
}

fn check_numbers(facts: &Facts) -> Result<(), String> {
    for a in &facts.albums {
        number(&a.disc)?;
        number(&a.number)?;
    }
    Ok(())
}

/// The new facts, as the server takes them (every album has its id by now).
fn facts_body(facts: &Facts, base: Uuid, reason: &str) -> Result<serde_json::Value, String> {
    let mut albums = Vec::new();
    for a in &facts.albums {
        let id = a.album_id.ok_or_else(|| format!("The album “{}” wasn't made.", a.title))?;
        albums.push(serde_json::json!({ "album_id": id, "disc_number": number(&a.disc)?, "track_number": number(&a.number)? }));
    }
    let relations: Vec<serde_json::Value> = facts.relations.iter().map(|r| serde_json::json!({ "kind": r.kind, "track_id": r.track_id })).collect();
    Ok(serde_json::json!({
        "base_revision_id": base,
        "status": facts.status,
        "albums": albums,
        "relations": relations,
        "reason": reason.trim(),
    }))
}

enum RowAction {
    Pick(Target),
    Remove(usize),
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(font(14.0, bold())).color(P.text));
    ui.add_space(2.0);
}

/// "%A is a remix of %B" → "This track is a remix of `other`"; "%B is a remix of %A" →
/// "`other` is a remix of this track".
fn relation_sentence(template: &str, other: &str) -> String {
    capitalized(&template.replace("%A", "this track").replace("%B", other))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(variant_revision: Uuid, facts_revision: Uuid, title: &str, genres: &[&str], status: &str) -> TrackDetail {
        let variant = Uuid::from_u128(2);
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "title": title, "artist_display": "", "status": status,
            "primary_variant_id": variant, "external_revision_id": facts_revision,
            "file": { "id": Uuid::from_u128(3), "sha256": "", "uploaded_filename": "", "size_bytes": 0, "uploaded_at": "" },
            "variants": [{ "id": variant, "label": "", "revision_id": variant_revision,
                           "data": { "title": title, "genres": genres } }],
        }))
        .unwrap()
    }

    #[test]
    fn rebasing_keeps_only_the_fields_i_changed() {
        let (r1, f1, r2, f2) = (Uuid::from_u128(10), Uuid::from_u128(11), Uuid::from_u128(20), Uuid::from_u128(21));
        let mut mine = TrackForm::from_detail(&detail(r1, f1, "Old", &["rap"], "released"), None).unwrap();
        mine.meta.title = "My title".into();
        mine.reason = "why".into();
        // They changed the genres and the status, and the title too.
        let theirs = TrackForm::from_detail(&detail(r2, f2, "Their title", &["rap", "trap"], "unreleased"), None).unwrap();
        mine.rebase(theirs);
        assert_eq!(mine.meta.title, "My title");
        assert_eq!(mine.meta.genres, "rap, trap");
        assert_eq!(mine.facts.status, "unreleased");
        assert_eq!(mine.reason, "why");
        // The bases are theirs now, and the title is still a change against them.
        assert_eq!((mine.variant_base, mine.facts_base), (r2, Some(f2)));
        assert!(mine.dirty());
        assert_eq!(mine.meta_was.title, "Their title");
    }

    #[test]
    fn a_form_without_changes_follows_the_details() {
        let (r1, f1, r2) = (Uuid::from_u128(10), Uuid::from_u128(11), Uuid::from_u128(20));
        let mut form = TrackForm::from_detail(&detail(r1, f1, "Old", &[], "released"), None).unwrap();
        form.follow(&detail(r2, f1, "New", &[], "released"), None);
        assert_eq!(form.meta.title, "New");
        assert!(!form.dirty());

        // With a change of mine it stays as I left it (and says it's behind).
        form.meta.title = "Mine".into();
        let newer = detail(Uuid::from_u128(30), f1, "Newer", &[], "released");
        form.follow(&newer, None);
        assert_eq!(form.meta.title, "Mine");
        assert!(form.behind(&newer));
    }

    #[test]
    fn relations_read_as_sentences_both_ways() {
        assert_eq!(relation_sentence("%A is a slowed version of %B", "“X”"), "This track is a slowed version of “X”");
        assert_eq!(relation_sentence("%B is a slowed version of %A", "“X”"), "“X” is a slowed version of this track");
    }

    #[test]
    fn bodies_are_what_the_server_takes() {
        let mut form = TrackForm::from_detail(&detail(Uuid::from_u128(10), Uuid::from_u128(11), "T", &[], "released"), None).unwrap();
        form.meta.genres = " rap ,, trap ".into();
        form.meta.release_date = "  ".into();
        let body = form.meta_body();
        assert_eq!(body["data"]["genres"], serde_json::json!(["rap", "trap"]));
        assert!(body["data"]["release_date"].is_null());
        let row = AlbumRow { album_id: Some(Uuid::from_u128(5)), title: "A".into(), new_kind: None, disc: "".into(), number: "x".into() };
        form.facts.albums.push(row);
        assert!(form.facts_body().unwrap_err().contains("“x”"));
        assert!(check_numbers(&form.facts).is_err());
        form.facts.albums[0].number = " 3 ".into();
        assert_eq!(form.facts_body().unwrap()["albums"][0]["track_number"], 3);
    }

    #[test]
    fn artists_and_albums_are_added_in_place() {
        let mut form = TrackForm::from_detail(&detail(Uuid::from_u128(10), Uuid::from_u128(11), "Song", &[], "released"), None).unwrap();
        form.add_artist(Some(Uuid::from_u128(7)), "A".into(), false);
        form.add_artist(None, "New person".into(), false);
        form.add_artist(Some(Uuid::from_u128(7)), "A".into(), false); // already there
        let roles: Vec<&str> = form.meta.credits.iter().map(|c| c.role.as_str()).collect();
        assert_eq!(roles, ["primary", "featured"]);

        form.add_album(None, "Song".into(), Some("single".into()));
        assert_eq!((form.facts.albums[0].disc.as_str(), form.facts.albums[0].number.as_str()), ("1", "1"));
        assert_eq!(form.focus_number, None);
        form.add_album(Some(Uuid::from_u128(8)), "LP".into(), None);
        assert_eq!(form.focus_number, Some(1));

        // The ambiguous artist fixed: the other one credited, same role, alias cleared.
        form.meta.credits[0].alias_id = Some(Uuid::from_u128(70));
        assert!(form.replace_artist(Uuid::from_u128(7), Uuid::from_u128(9), "A (the other)".into()));
        assert_eq!(form.meta.credits[0].artist_id, Some(Uuid::from_u128(9)));
        assert_eq!((form.meta.credits[0].role.as_str(), form.meta.credits[0].alias_id), ("primary", None));
        assert!(!form.replace_artist(Uuid::from_u128(7), Uuid::from_u128(9), "x".into()));

    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN` (a user with `edit_metadata`) and `ACETATE_TEST_TRACK` (a track
/// to edit: its title is changed, then put back, as two revisions).
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;

    #[test]
    #[ignore]
    fn a_save_and_a_stale_one() {
        let (Ok(server), Ok(token), Ok(track)) =
            (std::env::var("ACETATE_TEST_SERVER"), std::env::var("ACETATE_TEST_TOKEN"), std::env::var("ACETATE_TEST_TRACK"))
        else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("acetate-form-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            // A reference (copied into each closure) instead of the Api itself.
            let (api, track) = (&api, track.as_str());
            let detail = move || async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap() };
            let put = move |path: String, body| async move { api.send_json::<Written>(reqwest::Method::PUT, &path, Some(body)).await };
            let form = TrackForm::from_detail(&detail().await, None).unwrap();
            let original = form.meta.title.clone();
            let variant_path = format!("/api/tracks/{track}/variants/{}", form.variant);

            // A save: a new revision.
            let mut mine = TrackForm::from_detail(&detail().await, None).unwrap();
            mine.meta.title = format!("{original} (form test)");
            mine.reason = "the desktop client's form test".into();
            let written = put(variant_path.clone(), mine.meta_body()).await.unwrap();
            assert_ne!(written.revision_id, form.variant_base);
            // Facts sent back as they were are taken too (and change nothing).
            put(format!("/api/tracks/{track}/facts"), form.facts_body().unwrap()).await.unwrap();

            // A save from the old base: stale.
            let mut late = form;
            late.meta.title = format!("{original} (late)");
            let err = put(variant_path.clone(), late.meta_body()).await.err().expect("a stale save is refused");
            assert_eq!(err.code, "stale", "{err}");

            // Load theirs, keep mine: my title on their base, which saves.
            late.rebase(TrackForm::from_detail(&detail().await, None).unwrap());
            assert_eq!(late.meta.title, format!("{original} (late)"));
            put(variant_path.clone(), late.meta_body()).await.unwrap();

            // Put back.
            let mut back = TrackForm::from_detail(&detail().await, None).unwrap();
            back.meta.title = original.clone();
            back.reason = "the desktop client's form test (putting it back)".into();
            put(variant_path, back.meta_body()).await.unwrap();
            assert_eq!(detail().await.title, original);
        });
    }

    /// A new artist, a new single and a new cover, made by one save. Makes an artist and
    /// an album: run it against a throwaway database.
    #[test]
    #[ignore]
    fn new_artist_single_and_cover_in_one_save() {
        let (Ok(server), Ok(token), Ok(track)) =
            (std::env::var("ACETATE_TEST_SERVER"), std::env::var("ACETATE_TEST_TOKEN"), std::env::var("ACETATE_TEST_TRACK"))
        else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("acetate-form-test-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let detail = || async { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.unwrap() };
            let mut form = TrackForm::from_detail(&detail().await, None).unwrap();
            let name = format!("Form test artist {}", std::process::id());
            // Its own names each run, and on no album but the new single.
            form.meta.title = format!("Form test single {}", std::process::id());
            form.facts.albums.clear();
            form.meta.release_date = "2026".into();
            form.add_artist(None, name.clone(), false);
            form.add_album(None, form.meta.title.clone(), Some("single".into()));
            let mut png = Vec::new();
            image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 100, 50, 255]))
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            let outcome = save(
                &api, form.track, form.variant, form.variant_base, form.facts_base, "form test".into(),
                form.meta.clone(), form.facts.clone(), true, true, Some((Arc::new(png), "cover.png".into(), true)),
            )
            .await;
            assert!(outcome.error.is_none(), "{:?}", outcome.error.map(|e| e.message));
            assert!(outcome.meta.is_some() && outcome.facts.is_some() && outcome.cover);

            let d = detail().await;
            assert_eq!(d.title, form.meta.title);
            // Credited after the primary artist the track already had: as featured.
            assert!(d.credits.iter().any(|c| c.artist_name == name && c.role == "featured"), "{:?}", d.credits);
            let single = d.albums.first().expect("on the new single");
            assert_eq!(d.albums.len(), 1);
            assert_eq!((&single.title, single.disc_number, single.track_number), (&form.meta.title, Some(1), Some(1)));
            let album: crate::details::AlbumDetail =
                api.send_json(reqwest::Method::GET, &format!("/api/albums/{}", single.album_id), None).await.unwrap();
            assert_eq!(album.covers.len(), 1, "the cover went on the single");
            // Credited to the track's primary artists (only).
            let primary: Vec<&str> = d.credits.iter().filter(|c| c.role == "primary").map(|c| c.artist_name.as_str()).collect();
            let credited: Vec<&str> = album.credits.iter().map(|c| c.artist_name.as_str()).collect();
            assert_eq!(credited, primary);
        });
    }
}
