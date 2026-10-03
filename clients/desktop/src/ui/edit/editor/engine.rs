//! The editor's work in the background: the targets' details loaded, each track's plan
//! (its new metadata, facts, covers and sources from the staged changes, and what
//! changes, in the history's words), the server's preview of it (`POST /api/tracks/edit` with
//! `preview`: the albums that change along, each track's flags after it, why a track
//! can't be written), and applying it.
//!
//! Applying sends what the preview showed, a few tracks at a time (for progress): the new
//! artists and albums with the first request, their real ids used after. Each track is
//! written whole or not at all (the server's savepoints). A track someone changed since
//! it was loaded (`stale`) is loaded again and the changes are made again on it, once.
//! Covers, sources, groupings and a single of its own go in the same request, each
//! track's all or nothing.
//!
//! A **draft** (`DraftOf`) is a track only a preview has made: a file that didn't import,
//! imported with the edit. Its details come with it (they can't be loaded), and its
//! preview and apply go to `POST /api/ingest-problems/{id}/import` instead, with the
//! draft's ids (the server matches them to the ones it makes). Applying it imports the
//! file; `Applied::made` is the track it became.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::model::{self, Attached, Base, Facts, Image, Meta, Op};
use super::resolve::Names;
use crate::api::Api;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::Shell;

const PREVIEW_CHUNK: usize = 200;
const APPLY_CHUNK: usize = 25;
/// The preview waits this long after the last change (typing settles first).
const SETTLE: Duration = Duration::from_millis(250);

/// A track only a preview has made (see the module's notes).
#[derive(Debug, Clone)]
pub struct DraftOf {
    /// The file that didn't import.
    pub problem: Uuid,
    /// The `draft` answered with its details: sent back with the edit.
    pub ids: Value,
    /// Sent with the edit too (`original_filename`).
    pub with: Value,
    /// Its open flags as the import leaves it, before the edit.
    pub flags: Vec<String>,
}

/// The answer of `POST /api/ingest-problems/{id}/import`.
#[derive(Debug, Clone, Deserialize)]
pub struct Imported {
    /// The new track (or one that has the file already).
    pub track_id: Option<Uuid>,
    /// The new track before the edit (None: a duplicate).
    #[serde(default)]
    pub track: Option<TrackDetail>,
    #[serde(default)]
    pub draft: Option<Value>,
    #[serde(default)]
    pub edited: Option<Answer>,
    /// What it lacks to be imported ("title", "artist").
    #[serde(default)]
    pub missing: Vec<String>,
}

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
    pub attached: Attached,
    /// What changes, in the history's words (empty: nothing).
    pub lines: Vec<String>,
    /// Allowed, but worth a look (a single for a track on an album already…).
    pub warnings: Vec<String>,
    /// Its albums are made to match it (whether they need it, the server says).
    pub sync: bool,
    /// It gets a single of its own.
    pub single: bool,
    /// A draft: applying it imports it, changed or not.
    pub draft: bool,
    /// The metadata goes to a new version named this (`Op::Version`; a draft's is renamed).
    pub version: Option<String>,
}

impl Ready {
    pub fn meta_changed(&self) -> bool {
        self.version.is_some() || self.meta != self.base.meta
    }

    pub fn facts_changed(&self) -> bool {
        self.facts != self.base.facts
    }

    pub fn covers_changed(&self) -> bool {
        self.attached.covers != self.base.attached.covers
    }

    pub fn sources_changed(&self) -> bool {
        self.attached.sources != self.base.attached.sources || self.attached.origin != self.base.attached.origin
    }

    pub fn groups_changed(&self) -> bool {
        self.attached.groups != self.base.attached.groups
    }

    /// Something to write: a revision, covers, sources, groupings, a single.
    pub fn writes(&self) -> bool {
        self.draft || self.meta_changed() || self.facts_changed() || self.covers_changed() || self.sources_changed() || self.groups_changed() || self.single
    }

    /// Something for the server: a revision, or its albums to match.
    pub fn sends(&self) -> bool {
        self.writes() || self.sync
    }

    pub fn changes(&self) -> bool {
        self.sends()
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
    /// How many of the track's new covers it gets.
    #[serde(default)]
    pub covers: usize,
}

#[derive(Debug, Deserialize)]
struct Edited {
    #[serde(default)]
    artists: HashMap<Uuid, Uuid>,
    #[serde(default)]
    albums: HashMap<Uuid, Uuid>,
    #[serde(default)]
    groupings: HashMap<Uuid, Uuid>,
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
    /// A draft applied: the track it became.
    pub made: Option<Uuid>,
}

pub struct Engine {
    loaded: Arc<Mutex<Loaded>>,
    pub plans: Vec<Plan>,
    pub plan_key: u64,
    changed_at: Instant,
    pub preview: Arc<Mutex<Preview>>,
    pub applied: Arc<Mutex<Applied>>,
    /// The target is a draft (one track).
    pub draft: Option<DraftOf>,
    /// The server said the draft's import comes out differently now: it's to be previewed
    /// again.
    pub draft_stale: Arc<std::sync::atomic::AtomicBool>,
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
            draft: None,
            draft_stale: Arc::default(),
        }
    }
}

/// What the edit is: the changes, the new things' names, whether albums go along, why.
#[derive(Clone)]
pub struct Change {
    pub ops: Vec<Op>,
    pub new_artists: Vec<(Uuid, String)>,
    pub new_albums: Vec<(Uuid, String, String)>,
    /// Stand-in id → (name, kind).
    pub new_groupings: Vec<(Uuid, String, String)>,
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
        Change { ops, new_artists: names.new_artists.clone(), new_albums: names.new_albums.clone(), new_groupings: names.new_groupings.clone(), along, sync, why }
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
        self.draft.as_ref().map(|d| d.with.to_string()).hash(&mut hasher);
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

    /// Edits `track`, a draft (its details as the preview made them).
    pub fn set_draft(&mut self, of: DraftOf, track: TrackDetail) {
        let mut loaded = self.loaded.lock().unwrap();
        loaded.details.clear();
        loaded.asked.clear();
        loaded.asked.insert(track.id);
        loaded.details.insert(track.id, Ok(Arc::new(track)));
        loaded.version += 1;
        self.draft = Some(of);
    }

    /// Everything loaded again (after applying: the tracks have changed). A draft's details
    /// can't be: they're kept (and planned again).
    pub fn forget(&mut self) {
        if self.draft.is_none() {
            *self.loaded.lock().unwrap() = Loaded::default();
        }
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
        let stand_ins = names.stand_ins();
        let single = model::single(ops);
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
                        // The names its credits can be under: its own aliases, and those chosen.
                        let mut stand_ins = stand_ins.clone();
                        stand_ins.extend(d.credits.iter().filter_map(|c| Some((c.alias_id?, c.alias.clone()?))));
                        stand_ins.extend(d.alias_names.iter().map(|(k, v)| (*k, v.clone())));
                        stand_ins.extend(names.aliases.iter().cloned());
                        match Base::for_variant(d, variant) {
                            None => PlanState::Failed("it has no version to edit".into()),
                            Some(mut base) => {
                                base.attached.groups = in_catalog.map(|t| t.groupings.clone()).unwrap_or_default();
                                // A new version starts from nothing: only what the changes say.
                                let version = model::version(ops);
                                let start = if version.is_some() { Meta::default() } else { base.meta.clone() };
                                let (meta, facts) = model::apply(ops, &start, &base.facts, position);
                                let attached = model::attach(ops, &base.attached);
                                let mut lines = Vec::new();
                                if let Some(v) = &version {
                                    lines.push(match self.draft.is_some() {
                                        true => format!("version: its version named “{v}”, with only what's given here"),
                                        false => format!("version: + “{v}”, with only what's given here (not the shown one: choose it after)"),
                                    });
                                }
                                if meta != start {
                                    lines.extend(diff(&start, &meta, catalog, &stand_ins));
                                }
                                if facts != base.facts {
                                    lines.extend(diff(&base.facts, &facts, catalog, &stand_ins));
                                }
                                lines.extend(attached_lines(&base.attached, &attached));
                                let grouping = |g: &Uuid| catalog.grouping(*g).map(|g| g.name.clone()).or_else(|| stand_ins.get(g).map(|n| format!("{n} (new)"))).unwrap_or_default();
                                for g in attached.groups.iter().filter(|g| !base.attached.groups.contains(g)) {
                                    lines.push(format!("grouping: + “{}” (and a single it's alone on)", grouping(g)));
                                }
                                for g in base.attached.groups.iter().filter(|g| !attached.groups.contains(g)) {
                                    lines.push(format!("grouping: − “{}” (and a single it's alone on)", grouping(g)));
                                }
                                let mut warnings = Vec::new();
                                if single {
                                    lines.push(format!("single: + “{}” of its own (disc 1, track 1; its primary artists, date and covers)", meta.title));
                                    if !facts.albums.is_empty() {
                                        warnings.push("it's on an album already: it gets a single too (unmark it to leave it out)".into());
                                    }
                                }
                                PlanState::Ready(Box::new(Ready { position, base, meta, facts, attached, lines, warnings, sync: model::sync(ops), single, draft: self.draft.is_some(), version }))
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
        if let Some(draft) = self.draft.clone() {
            let (api, preview, ctx, change, stale) = (shell.api.clone(), self.preview.clone(), shell.ctx.clone(), change.clone(), self.draft_stale.clone());
            shell.runtime.spawn(async move {
                let answer = import(&api, &draft, &ready[0], &change, true, &stale).await;
                let mut p = preview.lock().unwrap();
                if p.key == key {
                    match answer {
                        Ok((answer, _)) => {
                            p.answers.insert(answer.track_id, answer);
                        }
                        Err(err) => p.error = Some(err),
                    }
                    p.running = false;
                }
                ctx.request_repaint();
            });
            return;
        }
        let (api, preview, ctx, change) = (shell.api.clone(), self.preview.clone(), shell.ctx.clone(), change.clone());
        shell.runtime.spawn(async move {
            let mut answers = HashMap::new();
            let mut error = None;
            for chunk in ready.chunks(PREVIEW_CHUNK) {
                let (body, refused) = body(chunk, &change, &HashMap::new(), true, true);
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
        // Their groupings as the library had them (the details don't say), for a track
        // loaded again.
        let groups: HashMap<Uuid, Vec<Uuid>> = ready.iter().map(|(t, r)| (*t, r.base.attached.groups.clone())).collect();
        {
            let mut applied = self.applied.lock().unwrap();
            *applied = Applied { phase: Phase::Applying, total: ready.len(), ..Applied::default() };
        }
        if let (Some(draft), Some(write)) = (self.draft.clone(), writes.first().cloned()) {
            let (api, applied, ctx, change, stale) = (shell.api.clone(), self.applied.clone(), shell.ctx.clone(), change.clone(), self.draft_stale.clone());
            shell.runtime.spawn(async move {
                let track = write.0;
                match import(&api, &draft, &write, &change, false, &stale).await {
                    Ok((answer, made)) => {
                        let mut a = applied.lock().unwrap();
                        a.answers.insert(track, answer);
                        a.made = made;
                        drop(a);
                        record(&applied, track, Outcome::Done);
                    }
                    Err(err) => record(&applied, track, Outcome::Failed(err)),
                }
                let mut a = applied.lock().unwrap();
                a.done = a.total;
                a.phase = Phase::Finished;
                a.finished_unseen = true;
                ctx.request_repaint();
            });
            return;
        }
        let (api, applied, ctx, change) = (shell.api.clone(), self.applied.clone(), shell.ctx.clone(), change.clone());
        shell.runtime.spawn(async move {
            let mut ids: HashMap<Uuid, Uuid> = HashMap::new();
            let mut stale: Vec<(Uuid, usize)> = Vec::new();
            for (n, chunk) in writes.chunks(APPLY_CHUNK).enumerate() {
                let outcomes = send(&api, chunk, &change, &mut ids, &applied, n == 0).await;
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
                        Some(mut base) => {
                            base.attached.groups = groups.get(&track).cloned().unwrap_or_default();
                            let version = model::version(&change.ops);
                            let start = if version.is_some() { Meta::default() } else { base.meta.clone() };
                            let (meta, facts) = model::apply(&change.ops, &start, &base.facts, position);
                            let attached = model::attach(&change.ops, &base.attached);
                            let single = model::single(&change.ops);
                            Ok(Ready { position, base, meta, facts, attached, lines: Vec::new(), warnings: Vec::new(), sync: change.sync, single, draft: false, version })
                        }
                        None => Err("it has no version to edit".to_string()),
                    },
                    Err(err) => Err(err.message),
                };
                let outcome = match again {
                    Ok(r) if !r.sends() => Ok(()),
                    Ok(r) => match send(&api, &[(track, r)], &change, &mut ids, &applied, false).await.pop() {
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
            let mut a = applied.lock().unwrap();
            a.done = a.total;
            a.phase = Phase::Finished;
            a.finished_unseen = true;
            ctx.request_repaint();
        });
    }
}

/// A draft's edit sent to its import (`preview` or for real): the answer for the draft
/// (its id the draft's; what it still lacks said as its error), and the track it became.
/// `stale`: set when the draft doesn't fit the import anymore.
async fn import(api: &Api, draft: &DraftOf, write: &(Uuid, Ready), change: &Change, preview: bool, stale: &std::sync::atomic::AtomicBool) -> Result<(Answer, Option<Uuid>), String> {
    let body = draft_body(draft, write, change, preview)?;
    let path = format!("/api/ingest-problems/{}/import", draft.problem);
    let imported = api.send_json::<Imported>(reqwest::Method::POST, &path, Some(body)).await.map_err(|e| {
        if e.code == "conflict" {
            stale.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        e.message
    })?;
    let mut answer = imported.edited.unwrap_or(Answer { track_id: write.0, error: None, along: Vec::new(), flags: Vec::new() });
    answer.track_id = write.0;
    if answer.error.is_none() && !imported.missing.is_empty() {
        let message = format!("a track needs a title and an artist: this one has no {} yet", imported.missing.join(" and no "));
        answer.error = Some(Failure { code: "invalid".into(), message });
    }
    Ok((answer, imported.track_id))
}

/// The request importing a draft with its edit: the edit's one track as `track`, the draft's
/// ids, and what it's sent with.
fn draft_body(draft: &DraftOf, write: &(Uuid, Ready), change: &Change, preview: bool) -> Result<Value, String> {
    let (mut body, refused) = body(std::slice::from_ref(write), change, &HashMap::new(), preview, true);
    if let Some((_, why)) = refused.into_iter().next() {
        return Err(why);
    }
    let entry = body["tracks"].as_array_mut().and_then(|t| t.pop()).unwrap_or_else(|| json!({}));
    let Value::Object(mut body) = body else { return Err("not an edit".into()) };
    body.remove("tracks");
    body.insert("track".into(), entry);
    body.insert("draft".into(), draft.ids.clone());
    if let Some(with) = draft.with.as_object() {
        body.extend(with.clone());
    }
    Ok(Value::Object(body))
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
/// `first`: the edit's first request (the accounts given to artists go with it).
async fn send(api: &Api, chunk: &[(Uuid, Ready)], change: &Change, ids: &mut HashMap<Uuid, Uuid>, applied: &Arc<Mutex<Applied>>, first: bool) -> Vec<(Uuid, usize, Result<(), String>)> {
    let (body, refused) = body(chunk, change, ids, false, first);
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
            ids.extend(edited.groupings);
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
/// `accounts`: with the accounts given to artists (once per edit: the first request).
fn body(chunk: &[(Uuid, Ready)], change: &Change, ids: &HashMap<Uuid, Uuid>, preview: bool, accounts: bool) -> (Value, Vec<(Uuid, String)>) {
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
            entry["meta"] = match (&r.version, r.draft) {
                (Some(label), false) => json!({ "new_variant": label, "data": meta }),
                (Some(label), true) => json!({ "variant_id": r.base.variant, "base_revision_id": r.base.variant_base, "data": meta, "label": label }),
                (None, _) => json!({ "variant_id": r.base.variant, "base_revision_id": r.base.variant_base, "data": meta }),
            };
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
        if r.covers_changed() {
            let base: Vec<Uuid> = r.base.attached.covers.iter().filter_map(|p| if let Image::Own(id) = p.image { Some(id) } else { None }).collect();
            let covers: Vec<Value> = r
                .attached
                .covers
                .iter()
                .map(|p| match &p.image {
                    Image::Own(id) => json!({ "id": id }),
                    Image::CopyOf(id) => json!({ "copy_of": id }),
                    Image::Staged => json!({ "staged": p.sha }),
                })
                .collect();
            entry["covers"] = json!({ "base": base, "covers": covers });
        }
        if r.sources_changed() {
            let (before, after) = (&r.base.attached, &r.attached);
            let add: Vec<Value> = after.sources.iter().filter(|s| s.new).map(|s| json!({ "id": s.id, "platform": s.platform, "data": s.data })).collect();
            let correct: Vec<Value> =
                after.sources.iter().filter(|s| s.corrected && !s.new).map(|s| json!({ "source_id": s.id, "platform": s.platform, "data": s.data })).collect();
            let trash: Vec<Uuid> = before.sources.iter().filter(|b| !after.sources.iter().any(|s| s.id == b.id)).map(|b| b.id).collect();
            let origin = (after.origin != before.origin).then_some(after.origin).flatten();
            entry["sources"] = json!({ "add": add, "correct": correct, "trash": trash, "origin": origin });
        }
        if r.groups_changed() {
            let (before, after) = (&r.base.attached.groups, &r.attached.groups);
            let add: Vec<Uuid> = after.iter().filter(|g| !before.contains(g)).map(|g| {
                used.insert(*g);
                real(*g)
            }).collect();
            let remove: Vec<Uuid> = before.iter().filter(|g| !after.contains(g)).copied().collect();
            entry["groups"] = json!({ "add": add, "remove": remove });
        }
        if r.single {
            entry["single"] = json!(true);
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
    // Accounts: of artists there, or new ones made (one not credited after all isn't made,
    // and its account goes with it).
    let artist_sources: Vec<Value> = if accounts {
        model::accounts(&change.ops)
            .into_iter()
            .filter(|(artist, _, _)| used.contains(artist) || !change.new_artists.iter().any(|(id, _)| id == artist))
            .map(|(artist, platform, data)| json!({ "artist_id": real(artist), "platform": platform, "data": data }))
            .collect()
    } else {
        Vec::new()
    };
    let new_artists: Vec<Value> =
        change.new_artists.iter().filter(|(id, _)| used.contains(id) && !ids.contains_key(id)).map(|(id, name)| json!({ "id": id, "name": name })).collect();
    let new_groupings: Vec<Value> = change
        .new_groupings
        .iter()
        .filter(|(id, _, _)| used.contains(id) && !ids.contains_key(id))
        .map(|(id, name, kind)| json!({ "id": id, "name": name, "kind": kind }))
        .collect();
    let body = json!({
        "tracks": tracks,
        "new_artists": new_artists,
        "new_albums": new_albums,
        "new_groupings": new_groupings,
        "artist_sources": artist_sources,
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
    let mut lines = crate::ui::views::history::changes_named(Some(&along.before), &along.after, catalog, &HashMap::new());
    match along.covers {
        0 => {}
        1 => lines.push("cover: + the new one".into()),
        n => lines.push(format!("covers: + the {n} new ones")),
    }
    lines.into_iter().map(|line| format!("album “{title}” along · {line}")).collect()
}

/// What changes in a track's covers and sources, in words.
pub fn attached_lines(before: &Attached, after: &Attached) -> Vec<String> {
    let mut lines = Vec::new();
    let what = |p: &model::Pic| match p.image {
        Image::Own(_) => "it",
        Image::CopyOf(_) => "a copy of another's",
        Image::Staged => "a new image",
    };
    for (n, p) in after.covers.iter().enumerate() {
        if !before.covers.iter().any(|b| b.sha == p.sha) {
            let place = if n == 0 { "the main one".to_string() } else { format!("#{}", n + 1) };
            lines.push(format!("cover: + {} as {place}", what(p)));
        }
    }
    for (n, b) in before.covers.iter().enumerate() {
        if !after.covers.iter().any(|p| p.sha == b.sha) {
            lines.push(format!("cover: − #{} (to the trash)", n + 1));
        }
    }
    let kept_before: Vec<&String> = before.covers.iter().map(|p| &p.sha).filter(|sha| after.covers.iter().any(|p| p.sha == **sha)).collect();
    let kept_after: Vec<&String> = after.covers.iter().map(|p| &p.sha).filter(|sha| before.covers.iter().any(|p| p.sha == **sha)).collect();
    if kept_before != kept_after {
        let main = after.covers.first().and_then(|p| before.covers.iter().position(|b| b.sha == p.sha)).map(|n| format!(" (the main one: what was #{})", n + 1)).unwrap_or_default();
        lines.push(format!("covers: in a new order{main}"));
    }
    for s in &after.sources {
        if s.new {
            lines.push(format!("source: + {}", s.label()));
        } else if s.corrected {
            lines.push(format!("source: {} corrected (a new snapshot)", s.label()));
        }
    }
    for b in before.sources.iter().filter(|b| !after.sources.iter().any(|s| s.id == b.id)) {
        lines.push(format!("source: − {} (to the trash)", b.label()));
    }
    if after.origin != before.origin {
        let named = after.origin.and_then(|o| after.sources.iter().find(|s| s.id == o)).map(|s| s.label()).unwrap_or_else(|| "(a source it won't have)".into());
        lines.push(format!("the file came from: → {named}"));
    }
    lines
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
            attached: Attached::default(),
        };
        let mut meta = base.meta.clone();
        meta.credits = primary.iter().map(|n| Credit { artist_id: id(*n), role: "primary".into(), alias_id: None }).collect();
        let mut facts = base.facts.clone();
        if let Some(a) = album {
            facts.albums.push(Membership { album_id: id(a), disc_number: None, track_number: None });
        }
        let attached = base.attached.clone();
        Ready { position, base, meta, facts, attached, lines: vec![], warnings: vec![], sync: false, single: false, draft: false, version: None }
    }

    #[test]
    fn requests_name_new_things_once_and_use_their_real_ids_after() {
        // A new artist (9) on both tracks, a new EP (8) both go on; only 9 credited on both.
        let change = Change { ops: vec![], new_artists: vec![(id(9), "New".into())], new_albums: vec![(id(8), "EP".into(), "ep".into())], new_groupings: vec![], along: true, sync: false, why: "why".into() };
        let chunk = vec![(id(1), ready(0, &[9, 5], Some(8))), (id(2), ready(1, &[9], Some(8)))];
        let (body, refused) = body(&chunk, &change, &HashMap::new(), true, true);
        assert!(refused.is_empty());
        assert_eq!(body["new_artists"], json!([{ "id": id(9), "name": "New" }]));
        assert_eq!(body["new_albums"][0]["credits"], json!([{ "artist_id": id(9), "role": "primary", "alias_id": null }]));
        assert_eq!(body["tracks"][0]["facts"]["base_revision_id"], json!(id(300)));
        assert_eq!(body["tracks"][0]["meta"]["base_revision_id"], json!(id(200)));
        assert_eq!(body["preview"], json!(true));

        // Once made: not again, and the tracks use the real ids.
        let made = HashMap::from([(id(9), id(19)), (id(8), id(18))]);
        let (body, _) = super::body(&chunk[1..], &change, &made, false, true);
        assert_eq!(body["new_artists"], json!([]));
        assert_eq!(body["new_albums"], json!([]));
        assert_eq!(body["tracks"][0]["meta"]["data"]["credits"][0]["artist_id"], json!(id(19)));
        assert_eq!(body["tracks"][0]["facts"]["albums"][0]["album_id"], json!(id(18)));
    }

    /// `along:on` with nothing else changed: the track is still sent, to be made to match.
    #[test]
    fn imported_metadata_goes_to_a_version_with_its_accounts() {
        let account = |artist: u128| Op::Account { artist: id(artist), platform: "soundcloud".into(), label: "soundcloud x".into(), data: json!({ "artist_id": 42 }) };
        // 9: a new artist, credited; 8: a new one not credited (taken back); 7: one there.
        let ops = vec![Op::Version("SoundCloud".into()), account(9), account(8), account(7), account(9)];
        let change = Change { ops, new_artists: vec![(id(9), "Up".into()), (id(8), "Gone".into())], new_albums: vec![], new_groupings: vec![], along: true, sync: false, why: String::new() };
        let mut r = ready(0, &[9], None);
        r.version = Some("SoundCloud".into());
        let (b, _) = body(&[(id(1), r.clone())], &change, &HashMap::new(), true, true);
        assert_eq!(b["tracks"][0]["meta"]["new_variant"], json!("SoundCloud"));
        assert!(b["tracks"][0]["meta"].get("variant_id").is_none());
        let accounts: Vec<Value> = b["artist_sources"].as_array().unwrap().iter().map(|a| a["artist_id"].clone()).collect();
        assert_eq!(accounts, [json!(id(9)), json!(id(7))]);
        assert_eq!(b["new_artists"], json!([{ "id": id(9), "name": "Up" }]));
        // Not with a later request.
        let (b, _) = body(&[(id(1), r.clone())], &change, &HashMap::new(), false, false);
        assert_eq!(b["artist_sources"], json!([]));
        // A draft: its version, renamed.
        r.draft = true;
        let (b, _) = body(&[(id(1), r)], &change, &HashMap::new(), true, true);
        assert_eq!((b["tracks"][0]["meta"]["label"].clone(), b["tracks"][0]["meta"]["variant_id"].clone()), (json!("SoundCloud"), json!(id(100))));
    }

    #[test]
    fn a_draft_goes_to_its_import_with_its_ids() {
        let change = Change { ops: vec![], new_artists: vec![(id(9), "New".into())], new_albums: vec![], new_groupings: vec![], along: true, sync: false, why: String::new() };
        let mut r = ready(0, &[9], None);
        r.draft = true;
        r.single = true;
        let draft = DraftOf { problem: id(1), ids: json!({ "ids": {}, "shape": "ab" }), with: json!({ "original_filename": "a.wav" }), flags: vec![] };
        let body = draft_body(&draft, &(id(5), r), &change, true).unwrap();
        assert_eq!(body["track"]["track_id"], json!(id(5)));
        assert_eq!(body["track"]["meta"]["data"]["credits"][0]["artist_id"], json!(id(9)));
        assert_eq!(body["track"]["single"], json!(true));
        assert_eq!(body["new_artists"], json!([{ "id": id(9), "name": "New" }]));
        assert_eq!(body["draft"], json!({ "ids": {}, "shape": "ab" }));
        assert_eq!((body["original_filename"].clone(), body["preview"].clone()), (json!("a.wav"), json!(true)));
        assert!(body.get("tracks").is_none());
        // Nothing changed: still imported, as it is.
        let mut plain = ready(0, &[], None);
        plain.draft = true;
        assert!(plain.sends());
        let body = draft_body(&draft, &(id(5), plain), &change, false).unwrap();
        assert_eq!(body["track"], json!({ "track_id": id(5), "along": true, "sync": false }));
    }

    #[test]
    fn a_track_to_match_is_sent_with_nothing_else() {
        let change = Change { ops: vec![Op::Sync], new_artists: vec![], new_albums: vec![], new_groupings: vec![], along: true, sync: true, why: String::new() };
        let mut r = ready(0, &[], None);
        r.meta = r.base.meta.clone();
        r.sync = true;
        assert!(!r.writes() && r.sends() && r.changes());
        let (body, refused) = body(&[(id(1), r)], &change, &HashMap::new(), true, true);
        assert!(refused.is_empty());
        let track = &body["tracks"][0];
        assert_eq!((track["sync"].clone(), track["along"].clone()), (json!(true), json!(true)));
        assert!(track.get("meta").is_none() && track.get("facts").is_none());
    }

    #[test]
    fn covers_and_sources_go_as_the_server_takes_them() {
        use crate::ui::edit::editor::model::{Pic, Src};
        let change = Change { ops: vec![], new_artists: vec![], new_albums: vec![], new_groupings: vec![], along: true, sync: false, why: String::new() };
        let mut r = ready(0, &[], None);
        r.meta = r.base.meta.clone();
        let src = |n: u128, new: bool| Src { id: id(n), new, platform: "custom".into(), name: "x".into(), data: json!({ "label": "x" }), corrected: false };
        r.base.attached = Attached { covers: vec![Pic { image: Image::Own(id(1)), sha: "a".into() }], sources: vec![src(5, false)], origin: Some(id(5)), groups: vec![] };
        // A staged image as main, a copy, its own last; the old source to the trash, a new one the origin.
        r.attached = Attached {
            covers: vec![Pic { image: Image::Staged, sha: "b".into() }, Pic { image: Image::CopyOf(id(2)), sha: "c".into() }, Pic { image: Image::Own(id(1)), sha: "a".into() }],
            sources: vec![src(6, true)],
            origin: Some(id(6)),
            groups: vec![],
        };
        assert!(r.writes() && !r.meta_changed() && !r.facts_changed());
        let (body, _) = body(&[(id(9), r.clone())], &change, &HashMap::new(), true, true);
        let track = &body["tracks"][0];
        assert_eq!(track["covers"], json!({ "base": [id(1)], "covers": [{ "staged": "b" }, { "copy_of": id(2) }, { "id": id(1) }] }));
        assert_eq!(track["sources"]["add"], json!([{ "id": id(6), "platform": "custom", "data": { "label": "x" } }]));
        assert_eq!((track["sources"]["trash"].clone(), track["sources"]["origin"].clone()), (json!([id(5)]), json!(id(6))));
        let lines = attached_lines(&r.base.attached, &r.attached);
        assert_eq!(lines[0], "cover: + a new image as the main one");
        assert!(lines.contains(&"source: − custom “x” (to the trash)".to_string()), "{lines:?}");
        // Nothing changed: nothing sent of them.
        r.attached = r.base.attached.clone();
        let (body, _) = super::body(&[(id(9), r)], &change, &HashMap::new(), true, true);
        assert!(body["tracks"][0].get("covers").is_none() && body["tracks"][0].get("sources").is_none());
    }

    #[test]
    fn groupings_and_a_single_go_with_the_track() {
        let change = Change { ops: vec![], new_artists: vec![], new_albums: vec![], new_groupings: vec![(id(7), "Early".into(), "era".into())], along: true, sync: false, why: String::new() };
        let mut r = ready(0, &[], None);
        r.meta = r.base.meta.clone();
        r.base.attached.groups = vec![id(5)];
        r.attached.groups = vec![id(7)];
        r.single = true;
        let (body, _) = body(&[(id(9), r.clone())], &change, &HashMap::new(), true, true);
        let track = &body["tracks"][0];
        assert_eq!((track["groups"].clone(), track["single"].clone()), (json!({ "add": [id(7)], "remove": [id(5)] }), json!(true)));
        assert_eq!(body["new_groupings"], json!([{ "id": id(7), "name": "Early", "kind": "era" }]));
        // Made already: its real id, and not made again.
        let (body, _) = super::body(&[(id(9), r)], &change, &HashMap::from([(id(7), id(17))]), false, true);
        assert_eq!((body["tracks"][0]["groups"]["add"].clone(), body["new_groupings"].clone()), (json!([id(17)]), json!([])));
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
