//! Editing a track (the track page, in edit mode): the shown variant's metadata (title,
//! release date, genres, comment, credits) and the track's facts (status, albums,
//! relations). Two parts because the server keeps them apart: metadata is a variant's
//! (`PUT /api/tracks/{id}/variants/{vid}`), facts are the track's (`PUT …/facts`). A
//! save sends only the parts that changed, each with the revision the form was filled
//! from, so a change someone made in between is never silently undone: the server says
//! `stale` instead, and "Load theirs, keep mine" takes their version and puts back only
//! the fields you changed.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Key, Modifiers, RichText};
use serde::Deserialize;
use uuid::Uuid;

use super::{PickKind, Picked, SaveState};
use crate::api::ApiError;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::theme::{bold, font, medium, PALETTE as P};
use crate::ui::{Action, Shell};

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

#[derive(Debug, Clone, PartialEq)]
pub struct CreditRow {
    pub artist_id: Uuid,
    /// The artist's name, and the alias's (for showing).
    pub name: String,
    pub role: String,
    pub alias_id: Option<Uuid>,
    pub alias: Option<String>,
}

/// The track's facts, as the form edits them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    pub status: String,
    pub albums: Vec<AlbumRow>,
    pub relations: Vec<RelationRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRow {
    pub album_id: Uuid,
    pub title: String,
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

/// How a save went, from the background: the new revisions of the parts written (a part
/// written before another failed still counts), and the error, if any.
#[derive(Default)]
struct Outcome {
    meta: Option<Uuid>,
    facts: Option<Uuid>,
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
    reason: String,
    awaiting: Option<Target>,
    state: SaveState,
    /// What a save in progress sent (the fields may change while it runs).
    sending: Option<(Meta, Facts)>,
    answer: Arc<Mutex<Option<Outcome>>>,
    /// Esc was pressed with changes: a second Esc discards them.
    confirm_discard: bool,
    /// "Load theirs" was asked, and waits for the details to arrive.
    rebase_asked: bool,
    notice: Option<String>,
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

impl TrackForm {
    /// Filled from the track's details (its shown variant). None if it has no variant.
    pub fn from_detail(d: &TrackDetail, catalog: Option<&Catalog>) -> Option<TrackForm> {
        let variant = d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)).or(d.variants.first())?;
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
                CreditRow { artist_id: c.artist_id, name, role: c.role, alias_id: c.alias_id, alias }
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
                .map(|a| AlbumRow { album_id: a.album_id, title: a.title.clone(), disc: number(a.disc_number), number: number(a.track_number) })
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
            reason: String::new(),
            awaiting: None,
            state: SaveState::Idle,
            sending: None,
            answer: Arc::default(),
            confirm_discard: false,
            rebase_asked: false,
            notice: None,
        })
    }

    pub fn dirty(&self) -> bool {
        self.meta != self.meta_was || self.facts != self.facts_was
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
        let reason = std::mem::take(&mut self.reason);
        *self = TrackForm { meta, facts, reason, ..theirs };
        self.notice = Some("Their changes are loaded, with yours on top. Check them, then save.".into());
    }

    /// Follows the details: a form without changes shows the newest; one with changes
    /// keeps them (and, if "Load theirs" was asked, is rebased now).
    fn follow(&mut self, d: &TrackDetail, catalog: Option<&Catalog>) {
        let Some(fresh) = TrackForm::from_detail(d, catalog) else { return };
        if fresh.seen == self.seen || matches!(self.state, SaveState::Saving) {
            return;
        }
        self.seen = fresh.seen;
        let ours = (fresh.variant, fresh.variant_base, fresh.facts_base) == (self.variant, self.variant_base, self.facts_base);
        if ours {
            // Our own save, come back: only the names may be newer.
            if !self.dirty() {
                let reason = std::mem::take(&mut self.reason);
                let state = std::mem::take(&mut self.state);
                *self = TrackForm { reason, state, ..fresh };
            }
        } else if !self.dirty() {
            let state = std::mem::take(&mut self.state);
            *self = TrackForm { state, ..fresh };
        } else if self.rebase_asked || fresh.variant != self.variant {
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
        self.reason.clear();
        self.confirm_discard = false;
        self.state = SaveState::Idle;
        self.notice = None;
    }

    /// The variant's new data, as the server takes it.
    fn meta_body(&self) -> serde_json::Value {
        let m = &self.meta;
        let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let genres: Vec<String> = m.genres.split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect();
        let credits: Vec<serde_json::Value> =
            m.credits.iter().map(|c| serde_json::json!({ "artist_id": c.artist_id, "role": c.role, "alias_id": c.alias_id })).collect();
        serde_json::json!({
            "base_revision_id": self.variant_base,
            "reason": self.reason.trim(),
            "data": {
                "title": m.title.trim(),
                "release_date": text(&m.release_date),
                "genres": genres,
                "comment": text(&m.comment),
                "credits": credits,
            },
        })
    }

    /// The new facts, as the server takes them; an error for a number that isn't one.
    fn facts_body(&self) -> Result<serde_json::Value, String> {
        let base = self.facts_base.ok_or("This track has no facts revision to build on yet, so they can't be edited here.")?;
        let number = |text: &str| -> Result<Option<i16>, String> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            text.parse::<i16>().map(Some).map_err(|_| format!("Disc and track numbers are whole numbers (“{text}” isn't one)."))
        };
        let mut albums = Vec::new();
        for a in &self.facts.albums {
            albums.push(serde_json::json!({ "album_id": a.album_id, "disc_number": number(&a.disc)?, "track_number": number(&a.number)? }));
        }
        let relations: Vec<serde_json::Value> =
            self.facts.relations.iter().map(|r| serde_json::json!({ "kind": r.kind, "track_id": r.track_id })).collect();
        Ok(serde_json::json!({
            "base_revision_id": base,
            "status": self.facts.status,
            "albums": albums,
            "relations": relations,
            "reason": self.reason.trim(),
        }))
    }

    /// Sends the parts that changed, one after the other, in the background.
    fn save(&mut self, shell: &Shell) {
        if matches!(self.state, SaveState::Saving) || !self.dirty() {
            return;
        }
        if self.meta.title.trim().is_empty() {
            self.state = SaveState::Failed("A track needs a title.".into());
            return;
        }
        let meta = (self.meta != self.meta_was).then(|| self.meta_body());
        let facts = match (self.facts != self.facts_was).then(|| self.facts_body()).transpose() {
            Ok(facts) => facts,
            Err(err) => {
                self.state = SaveState::Failed(err);
                return;
            }
        };
        self.state = SaveState::Saving;
        self.confirm_discard = false;
        self.notice = None;
        self.sending = Some((self.meta.clone(), self.facts.clone()));
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let (track, variant) = (self.track, self.variant);
        shell.runtime.spawn(async move {
            let mut outcome = Outcome::default();
            let put = |path: String, body| {
                let api = api.clone();
                async move { api.send_json::<Written>(reqwest::Method::PUT, &path, Some(body)).await }
            };
            if let Some(body) = meta {
                match put(format!("/api/tracks/{track}/variants/{variant}"), body).await {
                    Ok(w) => outcome.meta = Some(w.revision_id),
                    Err(err) => outcome.error = Some(err),
                }
            }
            if let (Some(body), None) = (facts, &outcome.error) {
                match put(format!("/api/tracks/{track}/facts"), body).await {
                    Ok(w) => outcome.facts = Some(w.revision_id),
                    Err(err) => outcome.error = Some(err),
                }
            }
            *answer.lock().unwrap() = Some(outcome);
            ctx.request_repaint();
        });
    }

    /// Takes in how a save went.
    fn poll(&mut self, shell: &mut Shell) {
        let Some(outcome) = self.answer.lock().unwrap().take() else { return };
        let (sent_meta, sent_facts) = self.sending.take().unwrap_or_default();
        if let Some(revision) = outcome.meta {
            self.variant_base = revision;
            self.meta_was = sent_meta;
        }
        if let Some(revision) = outcome.facts {
            self.facts_base = Some(revision);
            self.facts_was = sent_facts;
        }
        self.state = match outcome.error {
            None => {
                self.reason.clear();
                SaveState::Saved(Instant::now())
            }
            Some(err) if err.code == "stale" => SaveState::Stale(err.message),
            Some(err) => SaveState::Failed(err.message),
        };
        // The details (and lists) catch up: on a stale save, that's their version.
        shell.track_details.refresh(self.track);
        if outcome.meta.is_some() || outcome.facts.is_some() {
            shell.act(Action::RefreshLibrary);
        }
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
            Target::Credit(None) => {
                let role = if self.meta.credits.iter().any(|c| c.role == "primary") { "featured" } else { "primary" };
                self.meta.credits.push(CreditRow { artist_id: picked.id, name: picked.name, role: role.into(), alias_id: None, alias: None });
            }
            Target::Album(Some(i)) => {
                if let Some(row) = self.facts.albums.get_mut(i) {
                    row.album_id = picked.id;
                    row.title = picked.name;
                }
            }
            Target::Album(None) => {
                self.facts.albums.push(AlbumRow { album_id: picked.id, title: picked.name, disc: String::new(), number: String::new() });
            }
            Target::Relation(Some(i)) => {
                if let Some(row) = self.facts.relations.get_mut(i) {
                    row.track_id = picked.id;
                    row.title = picked.name;
                }
            }
            Target::Relation(None) => {
                let kind = shell.account.vocab.as_ref().and_then(|v| v.relation_kinds.first()).map(|k| k.kind.clone()).unwrap_or_default();
                self.facts.relations.push(RelationRow { kind, track_id: picked.id, title: picked.name });
            }
        }
    }

    fn ask(&mut self, shell: &mut Shell, target: Target) {
        self.awaiting = Some(target);
        shell.act(Action::Pick(target.kind()));
    }

    /// The form. Ctrl+Enter saves; Esc (outside text fields) with changes asks, a second
    /// Esc discards them.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: Option<&Catalog>, d: &TrackDetail) {
        self.poll(shell);
        self.follow(d, catalog);
        if let Some(picked) = shell.picker.take() {
            self.take_pick(picked, shell);
        }
        let ctx = ui.ctx().clone();
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
            self.save(shell);
        }
        let palette_open = shell.palette_open;
        if !palette_open && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)) && self.dirty() {
            if self.confirm_discard {
                self.discard();
            } else {
                self.confirm_discard = true;
            }
        }

        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let before = (self.meta.clone(), self.facts.clone());

        let label = |ui: &mut egui::Ui, text: &str| {
            ui.label(RichText::new(text).color(P.faint));
        };
        let text = |ui: &mut egui::Ui, value: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(420.0));
        };

        // ---- The variant's metadata ----
        egui::Grid::new("track-form-meta").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
            label(ui, "Title");
            text(ui, &mut self.meta.title, "");
            ui.end_row();
            label(ui, "Released");
            text(ui, &mut self.meta.release_date, "as the source wrote it: 2019, 2019-05-12…");
            ui.end_row();
            label(ui, "Genres");
            text(ui, &mut self.meta.genres, "comma-separated");
            ui.end_row();
            label(ui, "Comment");
            text(ui, &mut self.meta.comment, "");
            ui.end_row();
        });

        ui.add_space(10.0);
        heading(ui, "Artists");
        let mut action = None;
        egui::Grid::new("track-form-credits").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            let count = self.meta.credits.len();
            for (i, row) in self.meta.credits.iter_mut().enumerate() {
                if ui.button(&row.name).on_hover_text("Choose another artist").clicked() {
                    action = Some(RowAction::Pick(Target::Credit(Some(i))));
                }
                combo(ui, ("credit-role", i), &mut row.role, &vocab.credit_roles, |r| capitalized(r));
                // Credited as: the artist's name, or one of its aliases.
                let aliases = shell.artist_details.get(row.artist_id).value.as_ref().map(|a| a.aliases.clone()).unwrap_or_default();
                let shown = row.alias.clone().map(|a| format!("as {a}")).unwrap_or_else(|| "as its name".into());
                egui::ComboBox::from_id_salt(("credit-alias", i)).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(row.alias_id.is_none(), format!("as {}", row.name)).clicked() {
                        row.alias_id = None;
                        row.alias = None;
                    }
                    for alias in &aliases {
                        if ui.selectable_label(row.alias_id == Some(alias.id), format!("as {}", alias.name)).clicked() {
                            row.alias_id = Some(alias.id);
                            row.alias = Some(alias.name.clone());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Earlier").clicked() {
                        action = Some(RowAction::Swap(i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < count, egui::Button::new("↓").small()).on_hover_text("Later").clicked() {
                        action = Some(RowAction::Swap(i, i + 1));
                    }
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
                        action = Some(RowAction::Remove(i));
                    }
                });
                ui.end_row();
            }
        });
        if ui.button("+ Add an artist").clicked() {
            action = Some(RowAction::Pick(Target::Credit(None)));
        }
        match action.take() {
            Some(RowAction::Pick(target)) => self.ask(shell, target),
            Some(RowAction::Swap(a, b)) => self.meta.credits.swap(a, b),
            Some(RowAction::Remove(i)) => {
                self.meta.credits.remove(i);
            }
            None => {}
        }
        ui.label(RichText::new("The artist text is made from these.").color(P.faint).small());

        // ---- The track's facts ----
        ui.add_space(14.0);
        heading(ui, "Albums and status");
        ui.horizontal(|ui| {
            label(ui, "Status");
            combo(ui, "status", &mut self.facts.status, &vocab.statuses, |s| capitalized(s));
        });
        ui.add_space(4.0);
        egui::Grid::new("track-form-albums").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            for (i, row) in self.facts.albums.iter_mut().enumerate() {
                if ui.button(&row.title).on_hover_text("Choose another album").clicked() {
                    action = Some(RowAction::Pick(Target::Album(Some(i))));
                }
                ui.horizontal(|ui| {
                    label(ui, "disc");
                    ui.add(egui::TextEdit::singleline(&mut row.disc).desired_width(36.0));
                    label(ui, "track");
                    ui.add(egui::TextEdit::singleline(&mut row.number).desired_width(36.0));
                });
                if ui.small_button("✕").on_hover_text("Take it off this album").clicked() {
                    action = Some(RowAction::Remove(i));
                }
                ui.end_row();
            }
        });
        if ui.button("+ Add to an album").clicked() {
            action = Some(RowAction::Pick(Target::Album(None)));
        }
        match action.take() {
            Some(RowAction::Pick(target)) => self.ask(shell, target),
            Some(RowAction::Remove(i)) => {
                self.facts.albums.remove(i);
            }
            _ => {}
        }

        if shell.account.can("edit_relations") {
            ui.add_space(14.0);
            heading(ui, "Related tracks");
            egui::Grid::new("track-form-relations").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                for (i, row) in self.facts.relations.iter_mut().enumerate() {
                    let phrase = |kind: &str| {
                        vocab.relation_kinds.iter().find(|k| k.kind == kind).map(|k| relation_phrase(&k.template)).unwrap_or_else(|| kind.replace('_', " "))
                    };
                    let kinds: Vec<String> = vocab.relation_kinds.iter().map(|k| k.kind.clone()).collect();
                    ui.horizontal(|ui| {
                        label(ui, "This track");
                        combo(ui, ("relation-kind", i), &mut row.kind, &kinds, phrase);
                    });
                    if ui.button(&row.title).on_hover_text("Choose another track").clicked() {
                        action = Some(RowAction::Pick(Target::Relation(Some(i))));
                    }
                    if ui.small_button("✕").on_hover_text("Remove").clicked() {
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

        if (self.meta.clone(), self.facts.clone()) != before {
            self.confirm_discard = false;
            if !matches!(self.state, SaveState::Saving) {
                self.state = SaveState::Idle;
            }
        }

        // ---- Saving ----
        ui.add_space(14.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with the change)").desired_width(560.0));
        ui.add_space(6.0);
        let dirty = self.dirty();
        ui.horizontal(|ui| {
            let saving = matches!(self.state, SaveState::Saving);
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
        if let SaveState::Saved(at) = self.state {
            if at.elapsed().as_secs() < 5 {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs(5));
            }
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

enum RowAction {
    Pick(Target),
    Swap(usize, usize),
    Remove(usize),
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(font(14.0, bold())).color(P.text));
    ui.add_space(2.0);
}

/// A drop-down of `options` (shown by `name`) for `value`. A value not among them (an
/// older list) is kept, and shown as it is.
fn combo(ui: &mut egui::Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut String, options: &[String], name: impl Fn(&str) -> String) {
    egui::ComboBox::from_id_salt(id).selected_text(RichText::new(name(value)).font(font(13.0, medium()))).show_ui(ui, |ui| {
        for option in options {
            if ui.selectable_label(value == option, name(option)).clicked() {
                *value = option.clone();
            }
        }
    });
}

/// "%A is a remix of %B" → "is a remix of".
fn relation_phrase(template: &str) -> String {
    template.replace("%A", "").replace("%B", "").trim().to_string()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(variant_revision: Uuid, facts_revision: Uuid, title: &str, genres: &[&str], status: &str) -> TrackDetail {
        let variant = Uuid::from_u128(2);
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "title": title, "artist_display": "", "status": status,
            "primary_variant_id": variant, "external_revision_id": facts_revision,
            "file": { "sha256": "", "uploaded_filename": "", "size_bytes": 0, "uploaded_at": "" },
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
        // They changed the genres and the status, and the title too.
        let theirs = TrackForm::from_detail(&detail(r2, f2, "Their title", &["rap", "trap"], "unreleased"), None).unwrap();
        mine.rebase(theirs);
        assert_eq!(mine.meta.title, "My title");
        assert_eq!(mine.meta.genres, "rap, trap");
        assert_eq!(mine.facts.status, "unreleased");
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
    fn bodies_are_what_the_server_takes() {
        let mut form = TrackForm::from_detail(&detail(Uuid::from_u128(10), Uuid::from_u128(11), "T", &[], "released"), None).unwrap();
        form.meta.genres = " rap ,, trap ".into();
        form.meta.release_date = "  ".into();
        let body = form.meta_body();
        assert_eq!(body["data"]["genres"], serde_json::json!(["rap", "trap"]));
        assert!(body["data"]["release_date"].is_null());
        form.facts.albums.push(AlbumRow { album_id: Uuid::from_u128(5), title: "A".into(), disc: "".into(), number: "x".into() });
        assert!(form.facts_body().unwrap_err().contains("“x”"));
        form.facts.albums[0].number = " 3 ".into();
        assert_eq!(form.facts_body().unwrap()["albums"][0]["track_number"], 3);
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
}
