//! Editing many tracks at once (the marked rows of a track table, in edit mode): set or
//! add artists, put them on an album (replacing their albums, or as well; numbered in the
//! order shown, or not), set the status, the release date, the genres; put them in a grouping (a
//! collective, an era…). Only the parts ticked apply.
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
use crate::catalog::search::{Part, Search};
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::theme::{font, semibold, PALETTE as P};
use crate::ui::Shell;

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
    /// Their numbers on one album (the tracks on it; others aren't changed).
    pub numbering: Option<Numbering>,
    /// A grouping to put them in (and its name, for the preview). Not a revision: the
    /// tracks not in it yet are put in it once they're written.
    pub grouping: Option<(Uuid, String)>,
}

/// Numbers on one album: the disc to set, and the track numbers in the order given,
/// from `renumber` (None: left as they are).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Numbering {
    pub album: Uuid,
    pub disc: Option<i16>,
    pub renumber: Option<i16>,
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
    if let Some(numbering) = &change.numbering
        && let Some(albums) = facts.get_mut("albums").and_then(|a| a.as_array_mut())
    {
        for entry in albums.iter_mut().filter(|a| a["album_id"] == json!(numbering.album)) {
            if let Some(disc) = numbering.disc {
                entry["disc_number"] = json!(disc);
            }
            if let Some(start) = numbering.renumber {
                entry["track_number"] = json!(start as usize + position);
            }
        }
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
    /// It goes in the grouping (it isn't in it yet).
    pub joins_grouping: bool,
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
    /// Applying finished and the library hasn't been asked to catch up yet.
    finished_unseen: bool,
}

/// A part of the change (for "Don't change: …").
#[derive(Debug, Clone, Copy, PartialEq)]
enum ChangePart {
    Artists,
    Album,
    Status,
    Date,
    Genres,
    Disc,
    Renumber,
    Grouping,
}

/// What an entry of "What to change" does.
#[derive(Debug, Clone, PartialEq)]
enum Pick {
    Artists(ArtistMode),
    Album(AlbumMode),
    Status(String),
    /// Its field (false), or none (true).
    Date(bool),
    Genres(bool),
    Disc,
    Renumber,
    Untick(ChangePart),
    // Typed with their values (`key:value`):
    DateValue(String),
    GenresValue(String),
    DiscValue(i16),
    RenumberFrom(i16),
    /// An artist to credit (as featured, or as primary if there's none yet).
    Credit { id: Uuid, name: String, featured: bool },
    SetAlbum { id: Uuid, title: String },
    /// Put them in this grouping.
    SetGrouping { id: Uuid, name: String },
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
    disc_on: bool,
    disc: i16,
    renumber_on: bool,
    renumber_from: i16,
    /// Put them in a grouping: which one.
    grouping_on: bool,
    grouping: Option<(Uuid, String)>,
    /// The album the list is of (an album's page), for its numbers.
    context_album: Option<Uuid>,
    reason: String,
    /// "What to change": typed to find a part of the change, Enter takes it.
    picker: String,
    picker_selected: usize,
    /// A number field to put the cursor in when it's drawn next ("disc", "renumber").
    focus_next: Option<&'static str>,
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
            disc_on: false,
            disc: 1,
            renumber_on: false,
            renumber_from: 1,
            grouping_on: false,
            grouping: None,
            context_album: None,
            reason: String::new(),
            picker: String::new(),
            picker_selected: 0,
            focus_next: Some("picker"),
            tracks: Vec::new(),
            job: Arc::default(),
        }
    }
}

impl BulkEditor {
    fn anything(&self) -> bool {
        (self.artists_on && !self.credits.is_empty())
            || (self.album_on && self.album.is_some())
            || self.status_on
            || self.date_on
            || self.genres_on
            || (self.numbering_album().is_some() && (self.disc_on || self.renumber_on))
            || (self.grouping_on && self.grouping.is_some())
    }

    /// The album whose numbers change: the list's (an album's page), else the existing
    /// album chosen above.
    fn numbering_album(&self) -> Option<Uuid> {
        self.context_album.or_else(|| self.album.as_ref().filter(|_| self.album_on).and_then(|a| a.0))
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
        let numbering = self.numbering_album().filter(|_| self.disc_on || self.renumber_on).map(|album| Numbering {
            album,
            disc: self.disc_on.then_some(self.disc),
            renumber: self.renumber_on.then_some(self.renumber_from),
        });
        let change = Change {
            artists,
            album,
            status: self.status_on.then(|| self.status.clone()),
            date: self.date_on.then(|| self.date.clone()),
            genres,
            numbering,
            grouping: self.grouping.clone().filter(|_| self.grouping_on),
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
                    None => Item { track: *id, title: "(couldn't be loaded)".into(), lines: vec![], state: ItemState::Failed("couldn't load it".into()), joins_grouping: false },
                    Some(d) => {
                        let Some((_, _, meta)) = shown_variant(d) else {
                            return Item { track: *id, title: d.title.clone(), lines: vec![], state: ItemState::Failed("no version to edit".into()), joins_grouping: false };
                        };
                        let facts = facts_of(d);
                        let (new_meta, new_facts) = apply(&change, &meta, &facts, n);
                        let mut lines = Vec::new();
                        for (old, new) in [(&meta, &new_meta), (&facts, &new_facts)] {
                            if old != new {
                                lines.extend(crate::ui::views::history::changes_named(Some(old), new, &catalog, &names));
                            }
                        }
                        // Into the grouping, if it isn't in it yet (as the library has it).
                        let joins_grouping = change.grouping.as_ref().is_some_and(|(g, _)| {
                            catalog.track_index(*id).is_some_and(|i| !catalog.tracks[i as usize].groupings.contains(g))
                        });
                        if let (true, Some((_, name))) = (joins_grouping, &change.grouping) {
                            lines.push(format!("grouping: + “{name}” (and a single it's alone on)"));
                        }
                        let state = if lines.is_empty() { ItemState::Unchanged } else { ItemState::Pending };
                        Item { track: *id, title: d.title.clone(), lines, state, joins_grouping }
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
            // The grouping: the ones written that go in it, in one go.
            if let Some((grouping, name)) = &change.grouping {
                let joining: Vec<Uuid> = job.lock().unwrap().items.iter().filter(|i| i.joins_grouping && i.state == ItemState::Done).map(|i| i.track).collect();
                if !joining.is_empty() {
                    let body = json!({ "track_ids": joining, "reason": reason });
                    if let Err(err) = api.send_json::<Value>(reqwest::Method::POST, &format!("/api/groupings/{grouping}/add"), Some(body)).await {
                        job.lock().unwrap().error = Some(format!("Their other changes were made, but they couldn't be put in “{name}”: {}", err.message));
                    }
                }
            }
            {
                let mut job = job.lock().unwrap();
                job.phase = Phase::Finished;
                job.finished_unseen = true;
            }
            ctx.request_repaint();
        });
    }

    /// What's in the popup: the change on the left, the preview (and progress) on the
    /// right. `marked`: the marked tracks, in the order shown; `album`: the album the list
    /// is of, if it is. Returns true when it's done and asks to close.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, marked: &[Uuid], album: Option<Uuid>) -> bool {
        self.context_album = album;
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

        let mut closed = false;
        // `/` (outside a field): back to "What to change".
        let slash = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "/")));
        if slash {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/")));
            self.focus_next = Some("picker");
        }
        let before = (self.change_key(), self.reason.clone());
        let (height, full) = (ui.available_height(), ui.available_width());
        let left_width = (full * 0.45).floor();
        ui.horizontal_top(|ui| {
            // ---- The change ----
            ui.allocate_ui_with_layout(egui::Vec2::new(left_width, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_height(height);
                ui.label(RichText::new("Tick what to change; the rest stays as each track has it.").color(P.faint).small());
                ui.add_space(6.0);
                if matches!(phase, Phase::Editing | Phase::Preview | Phase::Finished) {
                    self.picker(ui, shell);
                    ui.add_space(8.0);
                }
                egui::ScrollArea::vertical().id_salt("bulk-form").auto_shrink([false, false]).show(ui, |ui| {
                    let vocab = shell.account.vocab.clone().unwrap_or_default();
                    let editing = matches!(phase, Phase::Editing | Phase::Preview | Phase::Finished);
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
                        // ---- A grouping ----
                        if let Some(c) = &shell.catalog.value
                            && !c.groupings.is_empty()
                        {
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.grouping_on, "Put in a grouping");
                                if self.grouping_on {
                                    let shown = self.grouping.as_ref().map(|g| g.1.clone()).unwrap_or_else(|| "choose one…".into());
                                    egui::ComboBox::from_id_salt("bulk-grouping").selected_text(shown).show_ui(ui, |ui| {
                                        for g in &c.groupings {
                                            if ui.selectable_label(self.grouping.as_ref().is_some_and(|x| x.0 == g.id), format!("{} · {}", g.name, g.kind)).clicked() {
                                                self.grouping = Some((g.id, g.name.clone()));
                                            }
                                        }
                                    });
                                }
                            });
                        }
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.status_on, "Status");
                            if self.status_on {
                                combo(ui, "bulk-status", &mut self.status, &vocab.statuses, capitalized);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.date_on, "Release date");
                            if self.date_on {
                                ui.add(egui::TextEdit::singleline(&mut self.date).id(egui::Id::new("bulk-date")).hint_text("as written; empty: none").desired_width(200.0));
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.genres_on, "Genres");
                            if self.genres_on {
                                ui.add(egui::TextEdit::singleline(&mut self.genres).id(egui::Id::new("bulk-genres")).hint_text("comma-separated; empty: none").desired_width(300.0));
                            }
                        });
                        // ---- Numbers on an album ----
                        match self.numbering_album() {
                            Some(album) => {
                                let title = catalog.album(album).map(|a| a.title.clone()).unwrap_or_else(|| "the album".into());
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut self.disc_on, format!("Disc on “{title}”"));
                                    if self.disc_on {
                                        let field = ui.add(egui::DragValue::new(&mut self.disc).range(1..=99));
                                        if self.focus_next == Some("disc") {
                                            self.focus_next = None;
                                            field.request_focus();
                                        }
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut self.renumber_on, format!("Track numbers on “{title}”, in the order shown, from"));
                                    if self.renumber_on {
                                        let field = ui.add(egui::DragValue::new(&mut self.renumber_from).range(1..=999));
                                        if self.focus_next == Some("renumber") {
                                            self.focus_next = None;
                                            field.request_focus();
                                        }
                                    }
                                });
                            }
                            None => {
                                ui.label(RichText::new("Disc and track numbers: on an album's page, or with an existing album chosen above.").color(P.faint).small());
                            }
                        }
                        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with each change)").desired_width(420.0));
                    });
                });
            });
                // Any change to the change: the preview is out of date.
                if (self.change_key(), self.reason.clone()) != before && matches!(phase, Phase::Preview | Phase::Finished) {
                    self.job.lock().unwrap().phase = Phase::Editing;
                }

            ui.add_space(16.0);
            // ---- The preview, and applying it ----
            ui.allocate_ui_with_layout(egui::Vec2::new(full - left_width - 16.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_height(height);
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
                                        closed = true;
                                        return;
                                    }
                                }
                            }
                            if let Some(err) = &error {
                                ui.label(RichText::new(err).color(P.danger));
                            }
                            egui::ScrollArea::vertical().id_salt("bulk-preview").auto_shrink([false, false]).show(ui, |ui| {
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
            });
        });
        closed
    }

    /// The popup opened: the cursor goes to "What to change".
    pub fn opened(&mut self) {
        self.focus_next = Some("picker");
    }

    /// Everything "What to change" offers: (what it says, what it does), the ticked parts'
    /// "Don't change" first.
    fn picks(&self, shell: &Shell) -> Vec<(String, Pick)> {
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let mut picks: Vec<(String, Pick)> = Vec::new();
        for (on, name, part) in [
            (self.artists_on, "artists", ChangePart::Artists),
            (self.album_on, "album", ChangePart::Album),
            (self.status_on, "status", ChangePart::Status),
            (self.date_on, "release date", ChangePart::Date),
            (self.genres_on, "genres", ChangePart::Genres),
            (self.disc_on, "disc", ChangePart::Disc),
            (self.renumber_on, "track numbers", ChangePart::Renumber),
            (self.grouping_on, "grouping", ChangePart::Grouping),
        ] {
            if on {
                picks.push((format!("Don't change: {name}"), Pick::Untick(part)));
            }
        }
        picks.push(("Artists: set to… (replacing theirs)".into(), Pick::Artists(ArtistMode::Set)));
        picks.push(("Artists: add…".into(), Pick::Artists(ArtistMode::Add)));
        picks.push(("Album: on it only… (replacing their albums)".into(), Pick::Album(AlbumMode::Only)));
        picks.push(("Album: on it as well…".into(), Pick::Album(AlbumMode::Also)));
        for status in &vocab.statuses {
            picks.push((format!("Status: {status}"), Pick::Status(status.clone())));
        }
        picks.push(("Release date…".into(), Pick::Date(false)));
        picks.push(("Release date: none".into(), Pick::Date(true)));
        picks.push(("Genres…".into(), Pick::Genres(false)));
        picks.push(("Genres: none".into(), Pick::Genres(true)));
        if self.numbering_album().is_some() {
            picks.push(("Disc number…".into(), Pick::Disc));
            picks.push(("Track numbers, in the order shown, from…".into(), Pick::Renumber));
        }
        if let Some(c) = &shell.catalog.value {
            for g in &c.groupings {
                picks.push((format!("Grouping: into “{}” ({})", g.name, g.kind), Pick::SetGrouping { id: g.id, name: g.name.clone() }));
            }
        }
        picks
    }

    /// Takes a pick: ticks its part, and puts the cursor where its value goes.
    fn take(&mut self, ctx: &egui::Context, pick: Pick) {
        let focus = |id: &str| ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
        match pick {
            Pick::Artists(mode) => {
                self.artists_on = true;
                self.artist_mode = mode;
                focus("bulk-artist");
            }
            Pick::Album(mode) => {
                self.album_on = true;
                self.album_mode = mode;
                if self.album.is_none() {
                    focus("bulk-album");
                }
            }
            Pick::Status(status) => {
                self.status_on = true;
                self.status = status;
            }
            Pick::Date(none) => {
                self.date_on = true;
                if none {
                    self.date.clear();
                } else {
                    focus("bulk-date");
                }
            }
            Pick::Genres(none) => {
                self.genres_on = true;
                if none {
                    self.genres.clear();
                } else {
                    focus("bulk-genres");
                }
            }
            Pick::Disc => {
                self.disc_on = true;
                self.focus_next = Some("disc");
            }
            Pick::Renumber => {
                self.renumber_on = true;
                self.focus_next = Some("renumber");
            }
            Pick::DateValue(date) => {
                self.date_on = true;
                self.date = date;
            }
            Pick::GenresValue(genres) => {
                self.genres_on = true;
                self.genres = genres;
            }
            Pick::DiscValue(disc) => {
                self.disc_on = true;
                self.disc = disc;
            }
            Pick::RenumberFrom(from) => {
                self.renumber_on = true;
                self.renumber_from = from;
            }
            Pick::Credit { id, name, featured } => {
                self.artists_on = true;
                credits::add_artist(&mut self.credits, Some(id), name, featured);
            }
            Pick::SetAlbum { id, title } => {
                self.album_on = true;
                self.album = Some((Some(id), title));
            }
            Pick::SetGrouping { id, name } => {
                self.grouping_on = true;
                self.grouping = Some((id, name));
            }
            Pick::Untick(part) => match part {
                ChangePart::Artists => self.artists_on = false,
                ChangePart::Album => self.album_on = false,
                ChangePart::Status => self.status_on = false,
                ChangePart::Date => self.date_on = false,
                ChangePart::Genres => self.genres_on = false,
                ChangePart::Disc => self.disc_on = false,
                ChangePart::Renumber => self.renumber_on = false,
                ChangePart::Grouping => self.grouping_on = false,
            },
        }
    }

    /// What typed `key:value` parts would do: each with its pick, or why it can't (None).
    fn keyed_picks(&self, shell: &Shell, query: &crate::catalog::query::Query) -> Vec<(String, Option<Pick>)> {
        use crate::catalog::query::{loosely_in, Key};
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let catalog = shell.catalog.value.clone();
        let best = |part: Part, text: &str| -> Option<u32> {
            let catalog = catalog.as_deref()?;
            Search::default().run_part(catalog, text, part, 1).first().map(|h| h.0)
        };
        let none = |v: &str| v.is_empty() || v.eq_ignore_ascii_case("none");
        let mut picks = Vec::new();
        for (key, value) in &query.fields {
            let value = value.trim();
            let pick = match key {
                Key::Status => {
                    let found = vocab
                        .statuses
                        .iter()
                        .find(|s| s.eq_ignore_ascii_case(value))
                        .or_else(|| vocab.statuses.iter().find(|s| s.to_lowercase().starts_with(&value.to_lowercase())))
                        .or_else(|| vocab.statuses.iter().find(|s| loosely_in(value, s)));
                    match found {
                        Some(status) => (format!("status → {status}"), Some(Pick::Status(status.clone()))),
                        None => (format!("no status like “{value}”"), None),
                    }
                }
                Key::Year if none(value) => ("release date → none".into(), Some(Pick::Date(true))),
                Key::Year => (format!("release date → {value}"), Some(Pick::DateValue(value.to_string()))),
                Key::Genre if none(value) => ("genres → none".into(), Some(Pick::Genres(true))),
                Key::Genre => (format!("genres → {value}"), Some(Pick::GenresValue(value.to_string()))),
                Key::Disc | Key::Number if self.numbering_album().is_none() => {
                    ("disc and track numbers: on an album's page, or with an album chosen".into(), None)
                }
                Key::Disc => match value.parse::<i16>() {
                    Ok(disc) if disc > 0 => (format!("disc → {disc}"), Some(Pick::DiscValue(disc))),
                    _ => (format!("disc: “{value}” isn't a number"), None),
                },
                Key::Number => match value.parse::<i16>() {
                    Ok(from) if from > 0 => (format!("track numbers from {from}, in the order shown"), Some(Pick::RenumberFrom(from))),
                    _ => (format!("track numbers: “{value}” isn't a number"), None),
                },
                Key::Artist | Key::Featured if value.is_empty() => ("artists (type a name after the colon)".into(), None),
                Key::Artist | Key::Featured => {
                    let featured = *key == Key::Featured;
                    match best(Part::Artists, value).and_then(|i| catalog.as_deref().map(|c| &c.artists[i as usize])) {
                        Some(a) => {
                            let role = if featured { " (featured)" } else { "" };
                            (format!("credit “{}”{role}", a.name), Some(Pick::Credit { id: a.id, name: a.name.clone(), featured }))
                        }
                        None => (format!("no artist like “{value}”"), None),
                    }
                }
                Key::Album if value.is_empty() => ("album (type a title after the colon)".into(), None),
                Key::Album => match best(Part::Albums, value).and_then(|i| catalog.as_deref().map(|c| &c.albums[i as usize])) {
                    Some(a) => (format!("album → “{}” ({})", a.title, a.artist_display), Some(Pick::SetAlbum { id: a.id, title: a.title.clone() })),
                    None => (format!("no album like “{value}”"), None),
                },
                Key::Group if value.is_empty() => ("grouping (type its name after the colon)".into(), None),
                Key::Group => {
                    let found = shell.catalog.value.as_ref().and_then(|c| {
                        let lower = value.to_lowercase();
                        c.groupings.iter().find(|g| g.name.to_lowercase() == lower).or_else(|| c.groupings.iter().find(|g| loosely_in(value, &g.name))).map(|g| (g.id, g.name.clone()))
                    });
                    match found {
                        Some((id, name)) => (format!("grouping → into “{name}”"), Some(Pick::SetGrouping { id, name })),
                        None => (format!("no grouping like “{value}”"), None),
                    }
                }
                other => (format!("{}: not something to change here", other.name()), None),
            };
            picks.push(pick);
        }
        picks
    }

    /// "What to change": typed `key:value` parts (Enter does all of them), or a word or two
    /// to find a part (letters in order, spaces and punctuation don't matter).
    fn picker(&mut self, ui: &mut egui::Ui, shell: &Shell) {
        use crate::catalog::query::{loosely_in, Query};
        let id = egui::Id::new("bulk-picker");
        let ctx = ui.ctx().clone();
        // Asked for until it holds: the click that opened the popup ends outside the
        // field, and a field clicked outside of lets go of the cursor, that same frame.
        if self.focus_next == Some("picker") {
            if ctx.memory(|m| m.has_focus(id)) && !ctx.input(|i| i.pointer.any_released()) {
                self.focus_next = None;
            } else {
                ctx.memory_mut(|m| m.request_focus(id));
            }
        }
        let focused = ctx.memory(|m| m.has_focus(id));
        let (up, down, enter) = if focused {
            ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::Tab),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                )
            })
        } else {
            (false, false, false)
        };
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.picker)
                .id(id)
                .hint_text("What to change: status:demo  date:2020  genres:rap  disc:2  artist:…  album:…  (or a word, then Enter)")
                .desired_width(f32::INFINITY),
        );
        if response.changed() {
            self.picker_selected = 0;
        }
        let typed = self.picker.trim().to_string();
        if typed.is_empty() {
            ui.label(RichText::new("key:value sets it at once (several too); a word finds what to change · / comes back here · Ctrl+Enter previews, then applies").color(P.faint).small());
            return;
        }
        let query = Query::parse(&typed);
        let mut chosen: Vec<Pick> = Vec::new();
        if !query.fields.is_empty() {
            // `key:value`: all of it, on Enter.
            let picks = self.keyed_picks(shell, &query);
            egui::Frame::NONE.fill(P.bg).corner_radius(8).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.label(RichText::new("Enter:").color(P.faint).small());
                for (label, pick) in &picks {
                    let color = if pick.is_some() { P.accent } else { P.danger };
                    ui.label(RichText::new(label).color(color));
                }
            });
            if enter {
                chosen = picks.into_iter().filter_map(|(_, p)| p).collect();
            }
        } else {
            let found: Vec<(String, Pick)> = self.picks(shell).into_iter().filter(|(label, _)| loosely_in(&typed, label)).collect();
            if found.is_empty() {
                ui.label(RichText::new("Nothing like that to change").color(P.faint).small());
                return;
            }
            if down {
                self.picker_selected = (self.picker_selected + 1).min(found.len() - 1);
            }
            if up {
                self.picker_selected = self.picker_selected.saturating_sub(1);
            }
            self.picker_selected = self.picker_selected.min(found.len() - 1);
            if enter {
                chosen.push(found[self.picker_selected].1.clone());
            }
            egui::Frame::NONE.fill(P.bg).corner_radius(8).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (n, (label, pick)) in found.iter().enumerate() {
                    if ui.selectable_label(n == self.picker_selected, label).clicked() {
                        chosen = vec![pick.clone()];
                    }
                }
            });
        }
        if !chosen.is_empty() {
            self.picker.clear();
            self.picker_selected = 0;
            // (A pick with a field of its own takes the cursor; the others keep it here.)
            ctx.memory_mut(|m| m.request_focus(id));
            for pick in chosen {
                self.take(&ctx, pick);
            }
        }
    }

    /// Whether applying just finished (once): the list's owner then refreshes the library,
    /// whether the popup is open or not.
    pub fn take_finished(&self) -> bool {
        std::mem::take(&mut self.job.lock().unwrap().finished_unseen)
    }

    /// Everything the change depends on, to notice it changing.
    fn change_key(&self) -> String {
        format!(
            "{}{:?}{:?}{}{:?}{:?}{}{}{}{}{}{}{}{}{}{}{}{}{:?}{}{:?}",
            self.artists_on, self.artist_mode, self.credits, self.album_on, self.album_mode, self.album, self.new_kind, self.number, self.status_on,
            self.status, self.date_on, self.date, self.genres_on, self.genres, self.disc_on, self.disc, self.renumber_on, self.renumber_from,
            self.context_album, self.grouping_on, self.grouping
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
    fn discs_and_numbers_on_one_album() {
        let meta = json!({ "title": "T" });
        let facts = json!({ "status": "released", "albums": [
            { "album_id": a(8), "disc_number": 1, "track_number": 5 },
            { "album_id": a(9), "track_number": 2 },
        ]});
        let change = Change { numbering: Some(Numbering { album: a(8), disc: Some(2), renumber: Some(1) }), ..Default::default() };
        let (_, f) = apply(&change, &meta, &facts, 3);
        assert_eq!(f["albums"][0], json!({ "album_id": a(8), "disc_number": 2, "track_number": 4 }));
        assert_eq!(f["albums"][1], json!({ "album_id": a(9), "track_number": 2 }), "the other album is left");
        // Not on it: nothing.
        let elsewhere = Change { numbering: Some(Numbering { album: a(7), disc: Some(2), renumber: None }), ..Default::default() };
        assert_eq!(apply(&elsewhere, &meta, &facts, 0).1, facts);
    }

    #[test]
    fn a_pick_ticks_its_part() {
        let ctx = egui::Context::default();
        let mut editor = BulkEditor::default();
        editor.take(&ctx, Pick::Status("demo".into()));
        assert!(editor.status_on && editor.status == "demo");
        editor.genres = "rap".into();
        editor.take(&ctx, Pick::Genres(true));
        assert!(editor.genres_on && editor.genres.is_empty(), "genres: none");
        editor.take(&ctx, Pick::Disc);
        assert!(editor.disc_on && editor.focus_next == Some("disc"));
        editor.take(&ctx, Pick::Untick(ChangePart::Status));
        assert!(!editor.status_on);
        let (change, _) = editor.change();
        assert_eq!((change.status, change.genres), (None, Some(vec![])));
    }

    #[test]
    fn typed_values_set_at_once() {
        let ctx = egui::Context::default();
        let mut editor = BulkEditor::default();
        editor.take(&ctx, Pick::DateValue("2020".into()));
        editor.take(&ctx, Pick::GenresValue("rap, trap".into()));
        editor.take(&ctx, Pick::Credit { id: a(3), name: "Qlvarn".into(), featured: false });
        editor.take(&ctx, Pick::Credit { id: a(4), name: "Bo".into(), featured: true });
        let (change, _) = editor.change();
        assert_eq!(change.date.as_deref(), Some("2020"));
        assert_eq!(change.genres, Some(vec!["rap".to_string(), "trap".to_string()]));
        let (_, credits) = change.artists.unwrap();
        let roles: Vec<&str> = credits.iter().map(|c| c["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["primary", "featured"]);
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
            // Disc 2 on that album, numbered from 5.
            let numbers = Change { numbering: Some(Numbering { album: album.id, disc: Some(2), renumber: Some(5) }), ..Default::default() };
            for (n, id) in tracks.iter().enumerate() {
                write_one(api, *id, &detail(*id).await, &numbers, n, "bulk test (numbers)").await.unwrap();
            }
            for (n, id) in tracks.iter().enumerate() {
                let d = detail(*id).await;
                let on = d.albums.iter().find(|a| a.album_id == album.id).unwrap();
                assert_eq!((on.disc_number, on.track_number), (Some(2), Some(n as i16 + 5)));
            }
            // Back to released.
            let back = Change { status: Some("released".into()), ..Default::default() };
            for id in &tracks {
                write_one(api, *id, &detail(*id).await, &back, 0, "bulk test (putting it back)").await.unwrap();
            }
        });
    }
}
