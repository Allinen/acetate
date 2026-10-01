//! The editor's work in the background: the targets' details loaded, each track's plan
//! (its new metadata and facts from the staged changes, and what changes, in the
//! history's words), the server's preview of it (`POST /api/tracks/edit` with
//! `preview`: the albums that change along, each track's flags after it, why a track
//! can't be written), and applying it.
//!
//! Applying sends what the preview showed, a few tracks at a time (for progress): the new
//! artists and albums with the first request, their real ids used after. Each track is
//! written whole or not at all (the server's savepoints). A track someone changed since
//! it was loaded (`stale`) is loaded again and the changes are made again on it, once.
//! Groupings last (they aren't revisions).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::model::{self, Base, Facts, Meta, Op};
use super::resolve::Names;
use crate::api::Api;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::Shell;

const PREVIEW_CHUNK: usize = 200;
const APPLY_CHUNK: usize = 25;
/// The preview waits this long after the last change (typing settles first).
const SETTLE: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Loaded {
    details: HashMap<Uuid, Result<Arc<TrackDetail>, String>>,
    asked: HashSet<Uuid>,
    /// How many times each track was loaded again after a stale answer.
    reloaded: HashMap<Uuid, u32>,
    version: u64,
}

/// One target, as the edit would leave it.
pub struct Plan {
    pub track: Uuid,
    pub title: String,
    pub state: PlanState,
}

pub enum PlanState {
    Loading,
    Failed(String),
    Ready(Box<Ready>),
}

#[derive(Clone)]
pub struct Ready {
    pub position: usize,
    pub base: Base,
    pub meta: Meta,
    pub facts: Facts,
    /// What changes, in the history's words (empty: nothing).
    pub lines: Vec<String>,
    /// The groupings it goes in (it isn't in them yet).
    pub joins: Vec<Uuid>,
    /// Its albums are made to match it (whether they need it, the server says).
    pub sync: bool,
}

impl Ready {
    pub fn meta_changed(&self) -> bool {
        self.meta != self.base.meta
    }

    pub fn facts_changed(&self) -> bool {
        self.facts != self.base.facts
    }

    /// A revision to write.
    pub fn writes(&self) -> bool {
        self.meta_changed() || self.facts_changed()
    }

    /// Something for the server: a revision, or its albums to match.
    pub fn sends(&self) -> bool {
        self.writes() || self.sync
    }

    pub fn changes(&self) -> bool {
        self.sends() || !self.joins.is_empty()
    }
}

/// The server's answer for a track.
#[derive(Debug, Clone, Deserialize)]
pub struct Answer {
    pub track_id: Uuid,
    #[serde(default)]
    pub error: Option<Failure>,
    #[serde(default)]
    pub along: Vec<Along>,
    #[serde(default)]
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Along {
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Deserialize)]
struct Edited {
    #[serde(default)]
    artists: HashMap<Uuid, Uuid>,
    #[serde(default)]
    albums: HashMap<Uuid, Uuid>,
    tracks: Vec<Answer>,
}

#[derive(Default)]
pub struct Preview {
    /// The plans it's for.
    pub key: u64,
    pub running: bool,
    pub answers: HashMap<Uuid, Answer>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Phase {
    #[default]
    Editing,
    Applying,
    Finished,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Done,
    Failed(String),
}

#[derive(Default)]
pub struct Applied {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
    pub outcomes: HashMap<Uuid, Outcome>,
    pub answers: HashMap<Uuid, Answer>,
    /// Finished, and the library hasn't been asked to catch up yet.
    pub finished_unseen: bool,
}

pub struct Engine {
    loaded: Arc<Mutex<Loaded>>,
    pub plans: Vec<Plan>,
    pub plan_key: u64,
    changed_at: Instant,
    pub preview: Arc<Mutex<Preview>>,
    pub applied: Arc<Mutex<Applied>>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine {
            loaded: Arc::default(),
            plans: Vec::new(),
            plan_key: 0,
            changed_at: Instant::now(),
            preview: Arc::default(),
            applied: Arc::default(),
        }
    }
}

/// What the edit is: the changes, the new things' names, whether albums go along, why.
#[derive(Clone)]
pub struct Change {
    pub ops: Vec<Op>,
    pub new_artists: Vec<(Uuid, String)>,
    pub new_albums: Vec<(Uuid, String, String)>,
    pub along: bool,
    /// Albums it's alone on made to match, changed or not.
    pub sync: bool,
    pub why: String,
}

impl Change {
    pub fn of(ops: Vec<Op>, names: &Names) -> Change {
        let along = model::along(&ops);
        let why = model::why(&ops);
        let sync = model::sync(&ops);
        Change { ops, new_artists: names.new_artists.clone(), new_albums: names.new_albums.clone(), along, sync, why }
    }
}

impl Engine {
    /// Loads what's missing, makes the plans again when anything they depend on changed, and
    /// asks for the server's preview once they're all there.
    /// `variant`: the version edited (one track; None: the shown one).
    pub fn update(&mut self, shell: &Shell, catalog: &Catalog, targets: &[Uuid], change: &Change, names: &Names, variant: Option<Uuid>) {
        self.load(shell, targets);
        let version = self.loaded.lock().unwrap().version;
        let mut hasher = std::hash::DefaultHasher::new();
        (targets, format!("{:?}", change.ops), names.version, version, shell.generation, change.along, change.sync, &change.why, variant).hash(&mut hasher);
        let key = hasher.finish();
        if key != self.plan_key {
            self.plans = self.plan(catalog, targets, &change.ops, names, variant);
            self.plan_key = key;
            self.changed_at = Instant::now();
        }
        if self.applied.lock().unwrap().phase != Phase::Editing {
            return;
        }
        // Answers that say a track changed meanwhile: it's loaded again (and planned again).
        let stale: Vec<Uuid> = {
            let preview = self.preview.lock().unwrap();
            if preview.key == key && !preview.running {
                preview.answers.values().filter(|a| a.error.as_ref().is_some_and(|e| e.code == "stale")).map(|a| a.track_id).collect()
            } else {
                Vec::new()
            }
        };
        if !stale.is_empty() {
            self.reload(&stale);
            return;
        }
        let settled = !self.plans.iter().any(|p| matches!(p.state, PlanState::Loading));
        let (asked, running) = {
            let preview = self.preview.lock().unwrap();
            (preview.key == key, preview.running)
        };
        if !settled || asked || running {
            return;
        }
        let wait = SETTLE.saturating_sub(self.changed_at.elapsed());
        if !wait.is_zero() {
            shell.ctx.request_repaint_after(wait);
            return;
        }
        self.ask_preview(shell, key, change);
    }

    /// The preview is for the plans as they are, and finished.
    pub fn preview_current(&self) -> bool {
        let preview = self.preview.lock().unwrap();
        preview.key == self.plan_key && !preview.running
    }

    /// Everything loaded again (after applying: the tracks have changed).
    pub fn forget(&mut self) {
        *self.loaded.lock().unwrap() = Loaded::default();
        *self.preview.lock().unwrap() = Preview::default();
        *self.applied.lock().unwrap() = Applied::default();
        self.plan_key = 0;
    }

    fn reload(&mut self, tracks: &[Uuid]) {
        let mut loaded = self.loaded.lock().unwrap();
        for t in tracks {
            let count = loaded.reloaded.entry(*t).or_default();
            *count += 1;
            // Twice and still stale: say so rather than loading forever.
            if *count > 2 {
                loaded.details.insert(*t, Err("someone keeps changing it; try again later".into()));
                continue;
            }
            loaded.details.remove(t);
            loaded.asked.remove(t);
        }
        loaded.version += 1;
    }

    fn load(&self, shell: &Shell, targets: &[Uuid]) {
        let missing: Vec<Uuid> = {
            let mut loaded = self.loaded.lock().unwrap();
            let missing: Vec<Uuid> = targets.iter().filter(|t| !loaded.details.contains_key(t) && !loaded.asked.contains(t)).copied().collect();
            loaded.asked.extend(missing.iter().copied());
            missing
        };
        if missing.is_empty() {
            return;
        }
        let (api, loaded, ctx) = (shell.api.clone(), self.loaded.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let limit = Arc::new(tokio::sync::Semaphore::new(8));
            let mut tasks = Vec::new();
            for id in missing {
                let (api, limit, loaded, ctx) = (api.clone(), limit.clone(), loaded.clone(), ctx.clone());
                tasks.push(tokio::spawn(async move {
                    let _permit = limit.acquire().await;
                    let d = api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await;
                    let mut loaded = loaded.lock().unwrap();
                    loaded.details.insert(id, d.map(Arc::new).map_err(|e| e.message));
                    loaded.version += 1;
                    ctx.request_repaint();
                }));
            }
            for task in tasks {
                let _ = task.await;
            }
        });
    }

    /// A target's plan, once it's loaded.
    pub fn ready(&self, track: Uuid) -> Option<&Ready> {
        self.plans.iter().find(|p| p.track == track).and_then(|p| if let PlanState::Ready(r) = &p.state { Some(&**r) } else { None })
    }

    /// The loaded details of a target.
    pub fn detail(&self, track: Uuid) -> Option<Arc<TrackDetail>> {
        self.loaded.lock().unwrap().details.get(&track).and_then(|d| d.as_ref().ok().cloned())
    }

    /// How many targets are loaded.
    pub fn loaded_count(&self, targets: &[Uuid]) -> usize {
        let loaded = self.loaded.lock().unwrap();
        targets.iter().filter(|t| loaded.details.contains_key(t)).count()
    }

    fn plan(&self, catalog: &Catalog, targets: &[Uuid], ops: &[Op], names: &Names, variant: Option<Uuid>) -> Vec<Plan> {
        let loaded = self.loaded.lock().unwrap();
        let groupings = model::groupings(ops);
        let stand_ins = names.stand_ins();
        targets
            .iter()
            .enumerate()
            .map(|(position, &track)| {
                let in_catalog = catalog.track_index(track).map(|i| &catalog.tracks[i as usize]);
                let mut title = in_catalog.map(|t| t.title.clone()).unwrap_or_default();
                let state = match loaded.details.get(&track) {
                    None => PlanState::Loading,
                    Some(Err(err)) => PlanState::Failed(format!("couldn't load it: {err}")),
                    Some(Ok(d)) => {
                        if title.is_empty() {
                            title = d.title.clone();
                        }
                        match Base::for_variant(d, variant) {
                            None => PlanState::Failed("it has no version to edit".into()),
                            Some(base) => {
                                let (meta, facts) = model::apply(ops, &base.meta, &base.facts, position);
                                let mut lines = Vec::new();
                                if meta != base.meta {
                                    lines.extend(diff(&base.meta, &meta, catalog, &stand_ins));
                                }
                                if facts != base.facts {
                                    lines.extend(diff(&base.facts, &facts, catalog, &stand_ins));
                                }
                                let joins: Vec<Uuid> = groupings.iter().filter(|g| in_catalog.is_none_or(|t| !t.groupings.contains(g))).copied().collect();
                                for g in &joins {
                                    let name = catalog.grouping(*g).map(|g| g.name.clone()).unwrap_or_default();
                                    lines.push(format!("grouping: + “{name}” (and a single it's alone on)"));
                                }
                                PlanState::Ready(Box::new(Ready { position, base, meta, facts, lines, joins, sync: model::sync(ops) }))
                            }
                        }
                    }
                };
                Plan { track, title, state }
            })
            .collect()
    }

    fn ask_preview(&self, shell: &Shell, key: u64, change: &Change) {
        let ready: Vec<(Uuid, Ready)> = self.plans.iter().filter_map(|p| match &p.state {
            PlanState::Ready(r) if r.sends() => Some((p.track, (**r).clone())),
            _ => None,
        }).collect();
        {
            let mut preview = self.preview.lock().unwrap();
            *preview = Preview { key, running: !ready.is_empty(), ..Preview::default() };
        }
        if ready.is_empty() {
            return;
        }
        let (api, preview, ctx, change) = (shell.api.clone(), self.preview.clone(), shell.ctx.clone(), change.clone());
        shell.runtime.spawn(async move {
            let mut answers = HashMap::new();
            let mut error = None;
            for chunk in ready.chunks(PREVIEW_CHUNK) {
                let (body, refused) = body(chunk, &change, &HashMap::new(), true);
                for (track, why) in refused {
                    answers.insert(track, refused_answer(track, why));
                }
                match api.send_json::<Edited>(reqwest::Method::POST, "/api/tracks/edit", Some(body)).await {
                    Ok(edited) => answers.extend(edited.tracks.into_iter().map(|a| (a.track_id, a))),
                    Err(err) => {
                        error = Some(err.message);
                        break;
                    }
                }
            }
            let mut p = preview.lock().unwrap();
            if p.key == key {
                p.answers = answers;
                p.error = error;
                p.running = false;
            }
            ctx.request_repaint();
        });
    }

    /// Applies the plans (the preview's), in the background.
    pub fn apply(&self, shell: &Shell, change: &Change, variant: Option<Uuid>) {
        let ready: Vec<(Uuid, Ready)> = self.plans.iter().filter_map(|p| match &p.state {
            PlanState::Ready(r) if r.changes() => Some((p.track, (**r).clone())),
            _ => None,
        }).collect();
        let writes: Vec<(Uuid, Ready)> = ready.iter().filter(|(_, r)| r.sends()).cloned().collect();
        let joins: Vec<(Uuid, Vec<Uuid>)> = ready.iter().filter(|(_, r)| !r.joins.is_empty()).map(|(t, r)| (*t, r.joins.clone())).collect();
        {
            let mut applied = self.applied.lock().unwrap();
            *applied = Applied { phase: Phase::Applying, total: ready.len(), ..Applied::default() };
        }
        let (api, applied, ctx, change) = (shell.api.clone(), self.applied.clone(), shell.ctx.clone(), change.clone());
        shell.runtime.spawn(async move {
            let mut ids: HashMap<Uuid, Uuid> = HashMap::new();
            let mut stale: Vec<(Uuid, usize)> = Vec::new();
            for chunk in writes.chunks(APPLY_CHUNK) {
                let outcomes = send(&api, chunk, &change, &mut ids, &applied).await;
                for (track, position, outcome) in outcomes {
                    match outcome {
                        Err(code) if code == "stale" => stale.push((track, position)),
                        Err(message) => record(&applied, track, Outcome::Failed(message)),
                        Ok(()) => record(&applied, track, Outcome::Done),
                    }
                }
                ctx.request_repaint();
            }
            // Changed meanwhile: loaded again, the changes made again on it, once.
            for (track, position) in stale {
                let again = match api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await {
                    Ok(d) => match Base::for_variant(&d, variant) {
                        Some(base) => {
                            let (meta, facts) = model::apply(&change.ops, &base.meta, &base.facts, position);
                            Ok(Ready { position, base, meta, facts, lines: Vec::new(), joins: Vec::new(), sync: change.sync })
                        }
                        None => Err("it has no version to edit".to_string()),
                    },
                    Err(err) => Err(err.message),
                };
                let outcome = match again {
                    Ok(r) if !r.sends() => Ok(()),
                    Ok(r) => match send(&api, &[(track, r)], &change, &mut ids, &applied).await.pop() {
                        Some((_, _, outcome)) => outcome,
                        None => Err("no answer".into()),
                    },
                    Err(err) => Err(err),
                };
                record(&applied, track, match outcome {
                    Ok(()) => Outcome::Done,
                    Err(code) if code == "stale" => Outcome::Failed("someone changed it again meanwhile; try again".into()),
                    Err(message) => Outcome::Failed(message),
                });
                ctx.request_repaint();
            }
            // Groupings: the tracks that go in each, written (or with nothing else to write).
            let mut by_grouping: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
            for (track, groupings) in &joins {
                let failed = matches!(applied.lock().unwrap().outcomes.get(track), Some(Outcome::Failed(_)));
                if !failed {
                    for g in groupings {
                        by_grouping.entry(*g).or_default().push(*track);
                    }
                }
            }
            for (grouping, tracks) in by_grouping {
                let body = json!({ "track_ids": tracks, "reason": change.why });
                let result = api.send_json::<Value>(reqwest::Method::POST, &format!("/api/groupings/{grouping}/add"), Some(body)).await;
                for track in &tracks {
                    let mut a = applied.lock().unwrap();
                    match &result {
                        Ok(_) => {
                            a.outcomes.entry(*track).or_insert(Outcome::Done);
                        }
                        Err(err) => {
                            a.outcomes.insert(*track, Outcome::Failed(format!("its other changes were made, but it couldn't go in the grouping: {}", err.message)));
                        }
                    }
                }
            }
            let mut a = applied.lock().unwrap();
            a.done = a.total;
            a.phase = Phase::Finished;
            a.finished_unseen = true;
            ctx.request_repaint();
        });
    }
}

fn record(applied: &Arc<Mutex<Applied>>, track: Uuid, outcome: Outcome) {
    let mut a = applied.lock().unwrap();
    a.outcomes.insert(track, outcome);
    a.done = a.outcomes.len().min(a.total);
}

fn refused_answer(track: Uuid, why: String) -> Answer {
    Answer { track_id: track, error: Some(Failure { code: "invalid".into(), message: why }), along: Vec::new(), flags: Vec::new() }
}

/// One request applying `chunk`; each track's outcome (Err: the error's code if it's
/// `stale`, else its message). New things made by it go in `ids`.
async fn send(api: &Api, chunk: &[(Uuid, Ready)], change: &Change, ids: &mut HashMap<Uuid, Uuid>, applied: &Arc<Mutex<Applied>>) -> Vec<(Uuid, usize, Result<(), String>)> {
    let (body, refused) = body(chunk, change, ids, false);
    let mut out: Vec<(Uuid, usize, Result<(), String>)> = Vec::new();
    for (track, why) in refused {
        let position = chunk.iter().find(|c| c.0 == track).map_or(0, |c| c.1.position);
        out.push((track, position, Err(why)));
    }
    if body["tracks"].as_array().is_some_and(|t| t.is_empty()) {
        return out;
    }
    match api.send_json::<Edited>(reqwest::Method::POST, "/api/tracks/edit", Some(body)).await {
        Ok(edited) => {
            ids.extend(edited.artists);
            ids.extend(edited.albums);
            for answer in edited.tracks {
                let position = chunk.iter().find(|c| c.0 == answer.track_id).map_or(0, |c| c.1.position);
                let outcome = match &answer.error {
                    Some(f) if f.code == "stale" => Err("stale".to_string()),
                    Some(f) => Err(f.message.clone()),
                    None => Ok(()),
                };
                applied.lock().unwrap().answers.insert(answer.track_id, answer.clone());
                out.push((answer.track_id, position, outcome));
            }
        }
        Err(err) => {
            for (track, r) in chunk {
                if !out.iter().any(|o| o.0 == *track) {
                    out.push((*track, r.position, Err(err.message.clone())));
                }
            }
        }
    }
    out
}

/// The request for `chunk`: each track's new metadata and facts (stand-ins replaced by
/// `ids` where made already), and the new things not made yet. Also the tracks refused
/// before asking (and why).
fn body(chunk: &[(Uuid, Ready)], change: &Change, ids: &HashMap<Uuid, Uuid>, preview: bool) -> (Value, Vec<(Uuid, String)>) {
    let real = |id: Uuid| ids.get(&id).copied().unwrap_or(id);
    let mut tracks = Vec::new();
    let mut refused = Vec::new();
    let mut used: HashSet<Uuid> = HashSet::new();
    for (track, r) in chunk {
        if r.facts_changed() && r.base.facts_base.is_none() {
            refused.push((*track, "it has no facts revision to build on yet, so its status, albums and related tracks can't change".into()));
            continue;
        }
        let mut entry = json!({ "track_id": track, "along": change.along, "sync": change.sync });
        if r.meta_changed() {
            let mut meta = r.meta.clone();
            for c in &mut meta.credits {
                used.insert(c.artist_id);
                c.artist_id = real(c.artist_id);
            }
            entry["meta"] = json!({ "variant_id": r.base.variant, "base_revision_id": r.base.variant_base, "data": meta });
        }
        if r.facts_changed() {
            let mut facts = r.facts.clone();
            for m in &mut facts.albums {
                used.insert(m.album_id);
                m.album_id = real(m.album_id);
            }
            let mut f = serde_json::to_value(&facts).unwrap_or_default();
            f["base_revision_id"] = json!(r.base.facts_base);
            entry["facts"] = f;
        }
        tracks.push(entry);
    }
    // New albums: credited to the primary artists all their tracks share.
    let wanted: Vec<&(Uuid, String, String)> = change.new_albums.iter().filter(|(id, _, _)| used.contains(id) && !ids.contains_key(id)).collect();
    let mut credited: Vec<Uuid> = Vec::new();
    let new_albums: Vec<Value> = wanted
        .into_iter()
        .map(|(id, title, kind)| {
            let on: Vec<&Ready> = chunk.iter().map(|c| &c.1).filter(|r| r.facts.albums.iter().any(|m| m.album_id == *id)).collect();
            let credits: Vec<Value> = on
                .first()
                .map(|first| {
                    first
                        .meta
                        .credits
                        .iter()
                        .filter(|c| c.role == "primary" && on.iter().all(|r| r.meta.credits.iter().any(|o| o.artist_id == c.artist_id && o.role == "primary")))
                        .map(|c| {
                            credited.push(c.artist_id);
                            json!({ "artist_id": real(c.artist_id), "role": "primary", "alias_id": c.alias_id })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let date = on.first().and_then(|r| r.meta.release_date.clone()).filter(|d| kind == "single" && on.iter().all(|r| r.meta.release_date.as_ref() == Some(d)));
            json!({ "id": id, "title": title, "kind": kind, "release_date": date, "credits": credits })
        })
        .collect();
    used.extend(credited);
    let new_artists: Vec<Value> =
        change.new_artists.iter().filter(|(id, _)| used.contains(id) && !ids.contains_key(id)).map(|(id, name)| json!({ "id": id, "name": name })).collect();
    let body = json!({
        "tracks": tracks,
        "new_artists": new_artists,
        "new_albums": new_albums,
        "reason": change.why,
        "preview": preview,
    });
    (body, refused)
}

/// What changed between two (metadata or facts), in the history's words.
fn diff<T: serde::Serialize>(before: &T, after: &T, catalog: &Catalog, names: &HashMap<Uuid, String>) -> Vec<String> {
    let (Ok(before), Ok(after)) = (serde_json::to_value(before), serde_json::to_value(after)) else { return Vec::new() };
    crate::ui::views::history::changes_named(Some(&before), &after, catalog, names)
}

/// The words for an album changed along: "album “X”: title: “A” → “B”".
pub fn along_lines(along: &Along, catalog: &Catalog) -> Vec<String> {
    let title = along.before.get("title").and_then(|t| t.as_str()).unwrap_or("its album");
    crate::ui::views::history::changes_named(Some(&along.before), &along.after, catalog, &HashMap::new())
        .into_iter()
        .map(|line| format!("album “{title}” along · {line}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::edit::editor::model::{Credit, Membership};

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn ready(position: usize, primary: &[u128], album: Option<u128>) -> Ready {
        let base = Base {
            variant: id(100 + position as u128),
            variant_base: id(200 + position as u128),
            meta: Meta { title: "T".into(), ..Default::default() },
            facts_base: Some(id(300 + position as u128)),
            facts: Facts { status: "released".into(), albums: vec![], relations: vec![] },
        };
        let mut meta = base.meta.clone();
        meta.credits = primary.iter().map(|n| Credit { artist_id: id(*n), role: "primary".into(), alias_id: None }).collect();
        let mut facts = base.facts.clone();
        if let Some(a) = album {
            facts.albums.push(Membership { album_id: id(a), disc_number: None, track_number: None });
        }
        Ready { position, base, meta, facts, lines: vec![], joins: vec![], sync: false }
    }

    #[test]
    fn requests_name_new_things_once_and_use_their_real_ids_after() {
        // A new artist (9) on both tracks, a new EP (8) both go on; only 9 credited on both.
        let change = Change { ops: vec![], new_artists: vec![(id(9), "New".into())], new_albums: vec![(id(8), "EP".into(), "ep".into())], along: true, sync: false, why: "why".into() };
        let chunk = vec![(id(1), ready(0, &[9, 5], Some(8))), (id(2), ready(1, &[9], Some(8)))];
        let (body, refused) = body(&chunk, &change, &HashMap::new(), true);
        assert!(refused.is_empty());
        assert_eq!(body["new_artists"], json!([{ "id": id(9), "name": "New" }]));
        assert_eq!(body["new_albums"][0]["credits"], json!([{ "artist_id": id(9), "role": "primary", "alias_id": null }]));
        assert_eq!(body["tracks"][0]["facts"]["base_revision_id"], json!(id(300)));
        assert_eq!(body["tracks"][0]["meta"]["base_revision_id"], json!(id(200)));
        assert_eq!(body["preview"], json!(true));

        // Once made: not again, and the tracks use the real ids.
        let made = HashMap::from([(id(9), id(19)), (id(8), id(18))]);
        let (body, _) = super::body(&chunk[1..], &change, &made, false);
        assert_eq!(body["new_artists"], json!([]));
        assert_eq!(body["new_albums"], json!([]));
        assert_eq!(body["tracks"][0]["meta"]["data"]["credits"][0]["artist_id"], json!(id(19)));
        assert_eq!(body["tracks"][0]["facts"]["albums"][0]["album_id"], json!(id(18)));
    }

    /// `along:on` with nothing else changed: the track is still sent, to be made to match.
    #[test]
    fn a_track_to_match_is_sent_with_nothing_else() {
        let change = Change { ops: vec![Op::Sync], new_artists: vec![], new_albums: vec![], along: true, sync: true, why: String::new() };
        let mut r = ready(0, &[], None);
        r.meta = r.base.meta.clone();
        r.sync = true;
        assert!(!r.writes() && r.sends() && r.changes());
        let (body, refused) = body(&[(id(1), r)], &change, &HashMap::new(), true);
        assert!(refused.is_empty());
        let track = &body["tracks"][0];
        assert_eq!((track["sync"].clone(), track["along"].clone()), (json!(true), json!(true)));
        assert!(track.get("meta").is_none() && track.get("facts").is_none());
    }

    #[test]
    fn reads_the_servers_answer() {
        let answer = json!({
            "artists": { id(9).to_string(): id(19) }, "albums": {},
            "tracks": [
                { "track_id": id(1), "meta": { "revision_id": id(2), "changed": true }, "flags": ["missing_cover"],
                  "along": [{ "album_id": id(8), "before": { "title": "Old", "kind": "single" }, "after": { "title": "New", "kind": "single" } }] },
                { "track_id": id(3), "error": { "code": "stale", "message": "Someone else changed this…" }, "flags": [] }
            ]
        });
        let edited: Edited = serde_json::from_value(answer).unwrap();
        assert_eq!(edited.artists[&id(9)], id(19));
        assert_eq!(edited.tracks[0].flags, ["missing_cover"]);
        assert_eq!(edited.tracks[0].along[0].after["title"], "New");
        assert_eq!(edited.tracks[1].error.as_ref().unwrap().code, "stale");
    }
}
