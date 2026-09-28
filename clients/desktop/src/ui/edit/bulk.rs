//! Editing many tracks at once (the marked rows of a track table, in edit mode): set or
//! add artists, put them on an album (replacing their albums, or as well; numbered in the
//! order shown, or not), set the status, the release date, the genres. Only the parts
//! ticked apply.
//!
//! Nothing is written before a preview: each marked track's details are loaded, the
//! change is worked out on them, and every track is listed with exactly what changes (in
//! the history's words), or "unchanged". Applying writes, per track, one revision for its
//! metadata and one for its facts, where they change, with the shared reason, a few
//! tracks at a time, with progress. A track someone changed meanwhile is loaded again and
//! the change redone on it, once. Failures are listed. New artists and a new album are
//! made first, when applying (not at the preview).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};
use serde_json::{json, Value};
use uuid::Uuid;

use super::credits::{self, CreditRow, CreditsEditor};
use super::inline_search::{Choice, InlineSearch};
use super::{capitalized, combo};
use crate::api::{Api, ApiError};
use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::{Action, Shell};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArtistMode {
    /// Their credits become exactly these.
    Set,
    /// These are credited too (ones already credited in that role aren't twice).
    Add,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlbumMode {
    /// On this album only (their other albums are dropped).
    Only,
    /// On this album as well.
    Also,
}

/// What to change (None: leave it). Ids are real (new artists and albums made already),
/// or stand-ins for the preview (see `Names`).
#[derive(Debug, Clone, Default)]
pub struct Change {
    pub artists: Option<(ArtistMode, Vec<Value>)>,
    /// The album, and whether to number the tracks in order (disc 1, 1, 2, 3…).
    pub album: Option<(AlbumMode, Uuid, bool)>,
    pub status: Option<String>,
    /// Empty: no date.
    pub date: Option<String>,
    pub genres: Option<Vec<String>>,
}

/// A track's metadata (its shown version's data) and facts, with this change: the new
/// ones. `position`: its place among the marked tracks (for numbering).
pub fn apply(change: &Change, meta: &Value, facts: &Value, position: usize) -> (Value, Value) {
    let (mut meta, mut facts) = (meta.clone(), facts.clone());
    if let Some((mode, credits)) = &change.artists {
        let now = meta.get("credits").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        let credits = match mode {
            ArtistMode::Set => credits.clone(),
            ArtistMode::Add => {
                let mut all = now;
                for c in credits {
                    if !all.iter().any(|x| x["artist_id"] == c["artist_id"] && x["role"] == c["role"]) {
                        all.push(c.clone());
                    }
                }
                all
            }
        };
        meta["credits"] = Value::Array(credits);
    }
    if let Some(date) = &change.date {
        meta["release_date"] = if date.trim().is_empty() { Value::Null } else { json!(date.trim()) };
    }
    if let Some(genres) = &change.genres {
        meta["genres"] = json!(genres);
    }
    if let Some(status) = &change.status {
        facts["status"] = json!(status);
    }
    if let Some((mode, album, number)) = &change.album {
        let mut albums = facts.get("albums").and_then(|a| a.as_array()).cloned().unwrap_or_default();
        let on = albums.iter().position(|a| a["album_id"] == json!(album));
        let mut entry = on.map(|i| albums[i].clone()).unwrap_or_else(|| json!({ "album_id": album }));
        if *number {
            entry["disc_number"] = json!(1);
            entry["track_number"] = json!(position + 1);
        }
        match (mode, on) {
            (AlbumMode::Only, _) => albums = vec![entry],
            (AlbumMode::Also, Some(i)) => albums[i] = entry,
            (AlbumMode::Also, None) => albums.push(entry),
        }
        facts["albums"] = Value::Array(albums);
    }
    (normalized(&meta), normalized(&facts))
}

/// Without empty fields (none, "", []), at every level: "no genres" and "genres: []"
/// are the same, and so are a credit without an alias and one with `"alias_id": null`.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, v)| !(v.is_null() || v.as_str() == Some("") || v.as_array().is_some_and(|a| a.is_empty())))
                .map(|(k, v)| (k.clone(), normalized(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        other => other.clone(),
    }
}

/// A track's facts as the server takes them, from its details.
pub fn facts_of(d: &TrackDetail) -> Value {
    let albums: Vec<Value> =
        d.albums.iter().map(|a| json!({ "album_id": a.album_id, "disc_number": a.disc_number, "track_number": a.track_number })).collect();
    let relations: Vec<Value> = d.relations.iter().map(|r| json!({ "kind": r.kind, "track_id": r.track_id })).collect();
    normalized(&json!({ "status": d.status, "albums": albums, "relations": relations }))
}

/// A track's shown version: (its id, its revision, its data).
fn shown_variant(d: &TrackDetail) -> Option<(Uuid, Uuid, Value)> {
    let v = d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)).or(d.variants.first())?;
    Some((v.id, v.revision_id, normalized(&v.data)))
}

/// One marked track in the preview.
#[derive(Debug, Clone)]
pub struct Item {
    pub track: Uuid,
    pub title: String,
    /// What changes, in words (empty: nothing).
    pub lines: Vec<String>,
    pub state: ItemState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemState {
    Unchanged,
    Pending,
    Done,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Phase {
    #[default]
    Editing,
    Loading,
    Preview,
    Applying,
    Finished,
}

/// The background work and its progress, shared with the tasks.
#[derive(Default)]
struct Job {
    phase: Phase,
    items: Vec<Item>,
    /// Each marked track's details as loaded for the preview (None: couldn't be).
    details: Vec<Option<TrackDetail>>,
    done: usize,
    error: Option<String>,
}

/// What the new artists and album are called (for the preview, before they exist): their
/// stand-in ids, and names.
type Names = HashMap<Uuid, String>;

pub struct BulkEditor {
    artists_on: bool,
    artist_mode: ArtistMode,
    credits: Vec<CreditRow>,
    credits_editor: CreditsEditor,
    album_on: bool,
    album_mode: AlbumMode,
    album_search: InlineSearch,
    /// The album chosen: an existing one (id), or a new one (None) of `new_kind`.
    album: Option<(Option<Uuid>, String)>,
    new_kind: String,
    number: bool,
    status_on: bool,
    status: String,
    date_on: bool,
    date: String,
    genres_on: bool,
    genres: String,
    reason: String,
    /// The marked tracks the preview was made for (a different marking: made again).
    tracks: Vec<Uuid>,
    job: Arc<Mutex<Job>>,
}

impl Default for BulkEditor {
    fn default() -> Self {
        BulkEditor {
            artists_on: false,
            artist_mode: ArtistMode::Set,
            credits: Vec::new(),
            credits_editor: CreditsEditor::default(),
            album_on: false,
            album_mode: AlbumMode::Only,
            album_search: InlineSearch::new(Part::Albums),
            album: None,
            new_kind: "album".into(),
            number: true,
            status_on: false,
            status: "released".into(),
            date_on: false,
            date: String::new(),
            genres_on: false,
            genres: String::new(),
            reason: String::new(),
            tracks: Vec::new(),
            job: Arc::default(),
        }
    }
}

impl BulkEditor {
    fn anything(&self) -> bool {
        (self.artists_on && !self.credits.is_empty()) || (self.album_on && self.album.is_some()) || self.status_on || self.date_on || self.genres_on
    }

    /// The change, with stand-in ids for what doesn't exist yet (new artists, a new
    /// album), and what those are called.
    fn change(&self) -> (Change, Names) {
        let mut names = Names::new();
        let mut stand_in = |name: &str| {
            let id = Uuid::from_u128(0xFFFF_0000 + names.len() as u128);
            names.insert(id, name.to_string());
            id
        };
        let artists = self.artists_on.then(|| {
            let rows: Vec<Value> = self
                .credits
                .iter()
                .map(|c| {
                    let id = c.artist_id.unwrap_or_else(|| stand_in(&c.name));
                    json!({ "artist_id": id, "role": c.role, "alias_id": c.alias_id })
                })
                .collect();
            (self.artist_mode, rows)
        });
        let album = match (&self.album, self.album_on) {
            (Some((id, title)), true) => Some((self.album_mode, id.unwrap_or_else(|| stand_in(title)), self.number)),
            _ => None,
        };
        let genres = self.genres_on.then(|| self.genres.split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect());
        let change = Change {
            artists,
            album,
            status: self.status_on.then(|| self.status.clone()),
            date: self.date_on.then(|| self.date.clone()),
            genres,
        };
        (change, names)
    }

    /// Loads the marked tracks' details and works out the preview.
    fn preview(&mut self, shell: &Shell, catalog: &Arc<Catalog>, marked: Vec<Uuid>) {
        self.tracks = marked.clone();
        {
            let mut job = self.job.lock().unwrap();
            *job = Job { phase: Phase::Loading, ..Job::default() };
        }
        let (change, names) = self.change();
        let (api, job, ctx, catalog) = (shell.api.clone(), self.job.clone(), shell.ctx.clone(), catalog.clone());
        shell.runtime.spawn(async move {
            let limit = Arc::new(tokio::sync::Semaphore::new(8));
            let mut tasks = Vec::new();
            for id in marked.clone() {
                let (api, limit, job, ctx) = (api.clone(), limit.clone(), job.clone(), ctx.clone());
                tasks.push(tokio::spawn(async move {
                    let _permit = limit.acquire().await;
                    let d = api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await;
                    job.lock().unwrap().done += 1;
                    ctx.request_repaint();
                    d
                }));
            }
            let mut details = Vec::new();
            for task in tasks {
                details.push(task.await.ok().and_then(|d| d.ok()));
            }
            let items = marked
                .iter()
                .zip(&details)
                .enumerate()
                .map(|(n, (id, d))| match d {
                    None => Item { track: *id, title: "(couldn't be loaded)".into(), lines: vec![], state: ItemState::Failed("couldn't load it".into()) },
                    Some(d) => {
                        let Some((_, _, meta)) = shown_variant(d) else {
                            return Item { track: *id, title: d.title.clone(), lines: vec![], state: ItemState::Failed("no version to edit".into()) };
                        };
                        let facts = facts_of(d);
                        let (new_meta, new_facts) = apply(&change, &meta, &facts, n);
                        let mut lines = Vec::new();
                        for (old, new) in [(&meta, &new_meta), (&facts, &new_facts)] {
                            if old != new {
                                lines.extend(crate::ui::views::history::changes_named(Some(old), new, &catalog, &names));
                            }
                        }
                        let state = if lines.is_empty() { ItemState::Unchanged } else { ItemState::Pending };
                        Item { track: *id, title: d.title.clone(), lines, state }
                    }
                })
                .collect();
            let mut job = job.lock().unwrap();
            job.items = items;
            job.details = details;
            job.phase = Phase::Preview;
            ctx.request_repaint();
        });
    }

    /// Writes it: new artists and album first, then each changed track.
    fn apply(&mut self, shell: &Shell) {
        let (mut rows, reason) = (self.credits.clone(), self.reason.trim().to_string());
        let album = self.album.clone().filter(|_| self.album_on);
        let new_kind = self.new_kind.clone();
        let (change_template, _) = self.change();
        {
            let mut job = self.job.lock().unwrap();
            job.phase = Phase::Applying;
            job.done = 0;
            job.error = None;
        }
        let (api, job, ctx) = (shell.api.clone(), self.job.clone(), shell.ctx.clone());
        let artists_on = self.artists_on;
        shell.runtime.spawn(async move {
            // New artists and album, then the change with their real ids.
            let made: Result<Change, ApiError> = async {
                let mut change = change_template;
                if artists_on {
                    credits::make_new_artists(&api, &mut rows).await?;
                    let mode = change.artists.as_ref().map(|a| a.0).unwrap_or(ArtistMode::Set);
                    change.artists = Some((mode, credits::to_json(&rows)));
                }
                if let (Some((None, title)), Some((mode, _, number))) = (&album, change.album) {
                    #[derive(serde::Deserialize)]
                    struct Created {
                        id: Uuid,
                    }
                    let primary: Vec<Value> = rows.iter().filter(|c| c.role == "primary").map(|c| json!({ "artist_id": c.artist_id, "role": "primary", "alias_id": c.alias_id })).collect();
                    let body = json!({ "title": title.trim(), "kind": new_kind, "credits": if artists_on { primary } else { vec![] } });
                    let created: Created = api.send_json(reqwest::Method::POST, "/api/albums", Some(body)).await?;
                    change.album = Some((mode, created.id, number));
                }
                Ok(change)
            }
            .await;
            let change = match made {
                Ok(change) => Arc::new(change),
                Err(err) => {
                    let mut job = job.lock().unwrap();
                    job.error = Some(format!("Nothing was changed: {}", err.message));
                    job.phase = Phase::Preview;
                    ctx.request_repaint();
                    return;
                }
            };
            let todo: Vec<(usize, Uuid, TrackDetail)> = {
                let job = job.lock().unwrap();
                job.items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.state == ItemState::Pending)
                    .filter_map(|(n, i)| Some((n, i.track, job.details[n].clone()?)))
                    .collect()
            };
            let limit = Arc::new(tokio::sync::Semaphore::new(4));
            let mut tasks = Vec::new();
            for (n, track, detail) in todo {
                let (api, limit, job, ctx, change, reason) = (api.clone(), limit.clone(), job.clone(), ctx.clone(), change.clone(), reason.clone());
                tasks.push(tokio::spawn(async move {
                    let _permit = limit.acquire().await;
                    let mut result = write_one(&api, track, &detail, &change, n, &reason).await;
                    // Changed meanwhile: loaded again, the change redone on it, once.
                    if result.as_ref().is_err_and(|e| e.code == "stale") {
                        result = match api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await {
                            Ok(fresh) => write_one(&api, track, &fresh, &change, n, &reason).await,
                            Err(err) => Err(err),
                        };
                    }
                    let mut job = job.lock().unwrap();
                    job.items[n].state = match result {
                        Ok(()) => ItemState::Done,
                        Err(err) => ItemState::Failed(err.message),
                    };
                    job.done += 1;
                    ctx.request_repaint();
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
            job.lock().unwrap().phase = Phase::Finished;
            ctx.request_repaint();
        });
    }

    /// The panel. `marked`: the marked tracks, in the order shown.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, marked: &[Uuid]) {
        let (phase, done, total, error) = {
            let job = self.job.lock().unwrap();
            (job.phase, job.done, job.items.len().max(self.tracks.len()), job.error.clone())
        };
        // A different marking than the preview's: back to editing.
        if matches!(phase, Phase::Preview) && self.tracks != marked {
            self.job.lock().unwrap().phase = Phase::Editing;
        }
        let ctx = ui.ctx().clone();
        let ctrl_enter = !shell.palette_open && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));

        ui.label(RichText::new(format!("Change {} tracks", marked.len())).font(font(15.0, semibold())));
        ui.label(RichText::new("Tick what to change; the rest stays as each track has it.").color(P.faint).small());
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let editing = matches!(phase, Phase::Editing | Phase::Preview | Phase::Finished);
        let before = (self.change_key(), self.reason.clone());
        ui.add_enabled_ui(editing, |ui| {
            // ---- Artists ----
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.artists_on, "Artists");
                if self.artists_on {
                    ui.selectable_value(&mut self.artist_mode, ArtistMode::Set, "set to these (replacing theirs)");
                    ui.selectable_value(&mut self.artist_mode, ArtistMode::Add, "add these");
                }
            });
            if self.artists_on {
                let field = egui::Id::new("bulk-artist");
                // (Choosing another artist for a row: remove it and add the other.)
                let _ = self.credits_editor.show(ui, shell, Some(catalog), "bulk", field, &mut self.credits, &vocab.credit_roles);
            }
            // ---- Album ----
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.album_on, "Album");
                if self.album_on {
                    ui.selectable_value(&mut self.album_mode, AlbumMode::Only, "on it only (replacing their albums)");
                    ui.selectable_value(&mut self.album_mode, AlbumMode::Also, "on it as well");
                }
            });
            if self.album_on {
                match self.album.clone() {
                    Some((id, title)) => {
                        ui.horizontal(|ui| {
                            let new = if id.is_none() { " (new)" } else { "" };
                            ui.label(RichText::new(format!("“{title}”{new}")).strong());
                            if id.is_none() {
                                combo(ui, "bulk-album-kind", &mut self.new_kind, &vocab.album_kinds, |k| format!("new {k}"));
                            }
                            if ui.small_button("Another album").clicked() {
                                self.album = None;
                            }
                        });
                    }
                    None => {
                        let typed = self.album_search.query.trim().to_string();
                        let extras = if typed.is_empty() { vec![] } else { vec![format!("New album “{typed}”")] };
                        match self.album_search.show(ui, shell, Some(catalog), egui::Id::new("bulk-album"), "The album: type its title", &extras) {
                            Some((Choice::Hit(i), _)) => {
                                if let Some(a) = catalog.albums.get(i as usize) {
                                    self.album = Some((Some(a.id), a.title.clone()));
                                }
                            }
                            Some((Choice::Extra(_), _)) => self.album = Some((None, typed)),
                            None => {}
                        }
                    }
                }
                ui.checkbox(&mut self.number, "Number them in the order shown (disc 1, tracks 1, 2, 3…)");
            }
            // ---- Status, date, genres ----
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.status_on, "Status");
                if self.status_on {
                    combo(ui, "bulk-status", &mut self.status, &vocab.statuses, capitalized);
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.date_on, "Release date");
                if self.date_on {
                    ui.add(egui::TextEdit::singleline(&mut self.date).hint_text("as written; empty: none").desired_width(200.0));
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.genres_on, "Genres");
                if self.genres_on {
                    ui.add(egui::TextEdit::singleline(&mut self.genres).hint_text("comma-separated; empty: none").desired_width(300.0));
                }
            });
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with each change)").desired_width(420.0));
        });
        // Any change to the change: the preview is out of date.
        if (self.change_key(), self.reason.clone()) != before && matches!(phase, Phase::Preview | Phase::Finished) {
            self.job.lock().unwrap().phase = Phase::Editing;
        }

        // ---- Preview, applying ----
        let phase = self.job.lock().unwrap().phase;
        ui.add_space(6.0);
        match phase {
            Phase::Editing => {
                let can = self.anything();
                if (ui.add_enabled(can, egui::Button::new("Preview  (Ctrl+Enter)")).clicked() || (ctrl_enter && can)) && !marked.is_empty() {
                    self.preview(shell, catalog, marked.to_vec());
                }
                if !can {
                    ui.label(RichText::new("Tick something to change.").color(P.faint).small());
                }
            }
            Phase::Loading => {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("Loading the tracks… {done} of {total}")));
            }
            Phase::Preview | Phase::Applying | Phase::Finished => {
                let (items, done_now) = {
                    let job = self.job.lock().unwrap();
                    (job.items.clone(), job.done)
                };
                let changing = items.iter().filter(|i| i.state != ItemState::Unchanged).count();
                let unchanged = items.len() - changing;
                match phase {
                    Phase::Preview => {
                        ui.horizontal(|ui| {
                            let text = format!("Apply to {changing} tracks  (Ctrl+Enter)");
                            if (ui.add_enabled(changing > 0, egui::Button::new(text)).clicked() || (ctrl_enter && changing > 0)) && error.is_none() {
                                self.apply(shell);
                            }
                            if ui.button("Back to the change").clicked() {
                                self.job.lock().unwrap().phase = Phase::Editing;
                            }
                            if unchanged > 0 {
                                ui.label(RichText::new(format!("{unchanged} unchanged")).color(P.faint));
                            }
                        });
                    }
                    Phase::Applying => {
                        ui.add(egui::ProgressBar::new(done_now as f32 / changing.max(1) as f32).text(format!("Changing… {done_now} of {changing}")));
                    }
                    _ => {
                        let failed = items.iter().filter(|i| matches!(i.state, ItemState::Failed(_))).count();
                        let text = if failed == 0 { format!("Done: {changing} tracks changed.") } else { format!("Done: {} changed, {failed} not (below).", changing - failed) };
                        ui.label(RichText::new(text).color(if failed == 0 { P.accent } else { P.danger }));
                        if ui.button("Close").clicked() {
                            *self = BulkEditor::default();
                            shell.act(Action::RefreshLibrary);
                            return;
                        }
                    }
                }
                if let Some(err) = &error {
                    ui.label(RichText::new(err).color(P.danger));
                }
                egui::ScrollArea::vertical().id_salt("bulk-preview").max_height(320.0).show(ui, |ui| {
                    for item in &items {
                        let (mark, color) = match &item.state {
                            ItemState::Unchanged => ("unchanged", P.faint),
                            ItemState::Pending => ("", P.text),
                            ItemState::Done => ("✓", P.accent),
                            ItemState::Failed(_) => ("✗", P.danger),
                        };
                        ui.label(RichText::new(format!("{mark} {}", item.title)).font(font(13.5, semibold())).color(color));
                        for line in &item.lines {
                            ui.label(RichText::new(format!("    {line}")).color(P.muted));
                        }
                        if let ItemState::Failed(err) = &item.state {
                            ui.label(RichText::new(format!("    {err}")).color(P.danger));
                        }
                    }
                });
                if phase == Phase::Finished {
                    // The library catches up once, at the end.
                    ui.ctx().request_repaint();
                }
            }
        }
    }

    /// Everything the change depends on, to notice it changing.
    fn change_key(&self) -> String {
        format!(
            "{}{:?}{:?}{}{:?}{:?}{}{}{}{}{}{}{}{}",
            self.artists_on, self.artist_mode, self.credits, self.album_on, self.album_mode, self.album, self.new_kind, self.number, self.status_on,
            self.status, self.date_on, self.date, self.genres_on, self.genres
        )
    }
}

/// One track: its metadata and facts with the change, written where they differ.
async fn write_one(api: &Api, track: Uuid, d: &TrackDetail, change: &Change, n: usize, reason: &str) -> Result<(), ApiError> {
    let Some((variant, base, meta)) = shown_variant(d) else { return Ok(()) };
    let facts = facts_of(d);
    let (new_meta, new_facts) = apply(change, &meta, &facts, n);
    if new_meta != meta {
        let body = json!({ "base_revision_id": base, "data": new_meta, "reason": reason });
        api.send_json::<Value>(reqwest::Method::PUT, &format!("/api/tracks/{track}/variants/{variant}"), Some(body)).await?;
    }
    if new_facts != facts {
        let Some(base) = d.external_revision_id else {
            return Err(ApiError { status: None, code: "invalid".into(), message: "it has no facts revision to build on".into() });
        };
        let mut body = new_facts.clone();
        body["base_revision_id"] = json!(base);
        body["reason"] = json!(reason);
        if body.get("status").is_none() {
            body["status"] = json!(d.status);
        }
        api.send_json::<Value>(reqwest::Method::PUT, &format!("/api/tracks/{track}/facts"), Some(body)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn artists_set_or_added() {
        let meta = json!({ "title": "T", "credits": [{ "artist_id": a(1), "role": "primary" }] });
        let facts = json!({ "status": "released" });
        let bo = json!({ "artist_id": a(2), "role": "featured", "alias_id": null });
        let set = Change { artists: Some((ArtistMode::Set, vec![bo.clone()])), ..Default::default() };
        // (An empty alias is left out, as the stored data has it.)
        assert_eq!(apply(&set, &meta, &facts, 0).0["credits"], json!([{ "artist_id": a(2), "role": "featured" }]));
        let add = Change { artists: Some((ArtistMode::Add, vec![bo.clone()])), ..Default::default() };
        let (added, _) = apply(&add, &meta, &facts, 0);
        assert_eq!(added["credits"].as_array().unwrap().len(), 2);
        // Added again: not twice.
        assert_eq!(apply(&add, &added, &facts, 0).0, added);
    }

    #[test]
    fn albums_numbered_in_order_replacing_or_as_well() {
        let meta = json!({ "title": "T" });
        let facts = json!({ "status": "released", "albums": [{ "album_id": a(8), "track_number": 5 }] });
        let only = Change { album: Some((AlbumMode::Only, a(9), true)), ..Default::default() };
        let (_, f) = apply(&only, &meta, &facts, 2);
        assert_eq!(f["albums"], json!([{ "album_id": a(9), "disc_number": 1, "track_number": 3 }]));
        let also = Change { album: Some((AlbumMode::Also, a(9), false)), ..Default::default() };
        let (_, f) = apply(&also, &meta, &facts, 2);
        assert_eq!(f["albums"].as_array().unwrap().len(), 2);
        assert_eq!(f["albums"][1], json!({ "album_id": a(9) }));
        // On it already: renumbered in place.
        let renumber = Change { album: Some((AlbumMode::Also, a(8), true)), ..Default::default() };
        let (_, f) = apply(&renumber, &meta, &facts, 0);
        assert_eq!(f["albums"], json!([{ "album_id": a(8), "track_number": 1, "disc_number": 1 }]));
    }

    #[test]
    fn empty_is_none_and_unchanged_is_unchanged() {
        let meta = json!({ "title": "T", "release_date": "2020" });
        let facts = json!({ "status": "released" });
        let clear = Change { date: Some("  ".into()), genres: Some(vec![]), ..Default::default() };
        let (m, _) = apply(&clear, &meta, &facts, 0);
        assert_eq!(m, json!({ "title": "T" }));
        let same = Change { status: Some("released".into()), genres: Some(vec![]), ..Default::default() };
        let (m, f) = apply(&same, &normalized(&meta), &facts, 0);
        assert_eq!((m, f), (normalized(&meta), facts));
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_TRACKS` (two track ids, comma-separated; they get
/// a status, genres and a new album: a throwaway database).
#[cfg(test)]
mod against_a_server {
    use super::*;

    #[test]
    #[ignore]
    fn two_tracks_changed_at_once() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_TRACKS"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(tracks)] = vars else { return };
        let tracks: Vec<Uuid> = tracks.split(',').map(|t| t.parse().unwrap()).collect();
        let dir = std::env::temp_dir().join(format!("acetate-bulk-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let api = &api;
            let detail = |id: Uuid| async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await.unwrap() };
            #[derive(serde::Deserialize)]
            struct Created {
                id: Uuid,
            }
            let album: Created = api
                .send_json(reqwest::Method::POST, "/api/albums", Some(json!({ "title": format!("Bulk test {}", std::process::id()), "kind": "ep" })))
                .await
                .unwrap();
            let change = Change {
                status: Some("demo".into()),
                genres: Some(vec!["bulk test".into()]),
                album: Some((AlbumMode::Also, album.id, true)),
                ..Default::default()
            };
            for (n, id) in tracks.iter().enumerate() {
                write_one(api, *id, &detail(*id).await, &change, n, "bulk test").await.unwrap();
            }
            for (n, id) in tracks.iter().enumerate() {
                let d = detail(*id).await;
                assert_eq!(d.status, "demo");
                let on = d.albums.iter().find(|a| a.album_id == album.id).expect("on the new album");
                assert_eq!((on.disc_number, on.track_number), (Some(1), Some(n as i16 + 1)), "numbered in order");
            }
            // The same change again: nothing to write (the preview would say "unchanged").
            let d = detail(tracks[0]).await;
            let (_, _, meta) = shown_variant(&d).unwrap();
            let facts = facts_of(&d);
            assert_eq!(apply(&change, &meta, &facts, 0), (meta, facts));
            // Back to released.
            let back = Change { status: Some("released".into()), ..Default::default() };
            for id in &tracks {
                write_one(api, *id, &detail(*id).await, &back, 0, "bulk test (putting it back)").await.unwrap();
            }
        });
    }
}
