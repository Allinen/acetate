//! One edit, from open to applied, with no UI: the tracks being changed, the changes staged
//! (each typed in the editor's line, whatever form or commandline it came from), the
//! server's preview of them and applying. The phone's editor is a thin skin over this; the
//! desktop's `TrackEditor` does the same job drawn with egui.
//!
//! * `stage(text)` turns a line into staged changes, all of it or none of it, saying why.
//! * `drive()` keeps the engine going (details loaded, plans made, the server's preview
//!   asked for): call it after anything changed and whenever the waker rang.
//! * `report()` is everything a screen shows: the rows, the chips, each track's changes and
//!   flags, and whether it can be applied.
//! * `chooser()` is a snapshot that lists the choices for a line being typed, off the
//!   session's lock (finding artists and albums takes a moment).
//! * Covers from the clipboard or a file, `parse:` (credits in titles) and importing
//!   metadata need more of a UI than a phone has yet: they're refused with a word saying so.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

use super::engine::{self, Change, Engine, Env, Outcome, Phase, PlanState};
use super::model::{self, Op};
use super::panel::{self, Row};
use super::parse::{self, K};
use super::resolve::{self, Candidate, Ctx, Lookups, Names, Pick, PinKind, Want};
use crate::account::Vocab;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::details::{AlbumDetail, AliasInfo, ArtistDetail, TrackDetail};

/// A staged change: what it was typed as, what it does.
#[derive(Debug, Clone)]
struct Staged {
    text: String,
    label: String,
    ops: Vec<Op>,
    warning: Option<String>,
    /// Its text can be typed back into the line (an uploaded image's can't).
    editable: bool,
}

/// A staged change as a screen shows it.
#[derive(Debug, Clone)]
pub struct Chip {
    pub text: String,
    pub label: String,
    pub warning: Option<String>,
}

/// The library's details, as resolving asks for them: fetched in the background the first
/// time, answered from what's there after.
#[derive(Clone)]
pub struct Library {
    env: Env,
    aliases: Arc<Mutex<HashMap<Uuid, Option<Vec<AliasInfo>>>>>,
    covers: Arc<Mutex<HashMap<(PinKind, Uuid), Option<Vec<(Uuid, String)>>>>>,
}

impl Library {
    fn new(env: Env) -> Library {
        Library { env, aliases: Arc::default(), covers: Arc::default() }
    }

    /// Whether anything was asked for and hasn't come (the choices are worth asking again).
    pub fn waiting(&self) -> bool {
        self.aliases.lock().unwrap().values().any(Option::is_none) || self.covers.lock().unwrap().values().any(Option::is_none)
    }
}

impl Lookups for Library {
    fn aliases(&mut self, artist: Uuid) -> Option<Vec<AliasInfo>> {
        let mut map = self.aliases.lock().unwrap();
        if let Some(found) = map.get(&artist) {
            return found.clone();
        }
        map.insert(artist, None);
        let (map, env) = (self.aliases.clone(), self.env.clone());
        self.env.runtime.spawn(async move {
            let found = env.api.send_json::<ArtistDetail>(reqwest::Method::GET, &format!("/api/artists/{artist}"), None).await.map(|a| a.aliases).unwrap_or_default();
            map.lock().unwrap().insert(artist, Some(found));
            env.waker.changed();
        });
        None
    }

    fn covers(&mut self, kind: PinKind, id: Uuid) -> Option<Vec<(Uuid, String)>> {
        let mut map = self.covers.lock().unwrap();
        if let Some(found) = map.get(&(kind, id)) {
            return found.clone();
        }
        map.insert((kind, id), None);
        let (map, env) = (self.covers.clone(), self.env.clone());
        self.env.runtime.spawn(async move {
            let list = |covers: &[crate::details::Cover]| covers.iter().map(|c| (c.id, c.blob_sha256.clone())).collect::<Vec<_>>();
            let found = match kind {
                PinKind::Album => env.api.send_json::<AlbumDetail>(reqwest::Method::GET, &format!("/api/albums/{id}"), None).await.map(|d| list(&d.covers)),
                _ => env.api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await.map(|d| list(&d.covers)),
            };
            map.lock().unwrap().insert((kind, id), Some(found.unwrap_or_default()));
            env.waker.changed();
        });
        None
    }
}

/// What a track's changes look like, for a screen.
#[derive(Debug, Clone, Default)]
pub struct TrackReport {
    pub track: Option<Uuid>,
    pub title: String,
    /// "✓" done, "✗" failed, "" otherwise.
    pub mark: String,
    /// What changes, in the history's words.
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    /// Flags (by name) the edit fixes, leaves, and raises.
    pub fixed: Vec<String>,
    pub still: Vec<String>,
    pub new: Vec<String>,
}

/// The edit applied and done with: how it went.
#[derive(Debug, Clone, Default)]
pub struct Finish {
    pub changed: usize,
    pub failed: usize,
    /// Why, for each that failed.
    pub errors: Vec<String>,
    /// The tracks that were written.
    pub tracks: Vec<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
    /// Targets still being loaded.
    pub loading: bool,
    /// The server hasn't checked the changes as they are now.
    pub checking: bool,
    /// Tracks the changes change, and tracks they can't be written to.
    pub changing: usize,
    pub refused: usize,
    pub can_apply: bool,
    /// Why not, when there's something to say.
    pub blocked: Option<String>,
    pub preview_error: Option<String>,
    /// New things the changes make ("artist “Ann”").
    pub makes: Vec<String>,
    pub tracks: Vec<TrackReport>,
}

pub struct Session {
    env: Env,
    catalog: Arc<Catalog>,
    vocab: Arc<Vocab>,
    pub targets: Vec<Uuid>,
    may_relate: bool,
    may_source: bool,
    names: Names,
    staged: Vec<Staged>,
    engine: Engine,
    library: Library,
    /// Applied and not looked at yet.
    finish: Option<Finish>,
    /// The revisions the targets had when applying began (for undoing).
    before: HashMap<Uuid, Vec<(String, Option<Uuid>, Uuid)>>,
    /// Applying was asked for before the preview was there (a single track only).
    apply_when_ready: bool,
}

impl Session {
    pub fn new(env: Env, catalog: Arc<Catalog>, vocab: Arc<Vocab>, targets: Vec<Uuid>, may_relate: bool, may_source: bool) -> Session {
        let library = Library::new(env.clone());
        Session {
            env,
            catalog,
            vocab,
            targets,
            may_relate,
            may_source,
            names: Names::default(),
            staged: Vec::new(),
            engine: Engine::default(),
            library,
            finish: None,
            before: HashMap::new(),
            apply_when_ready: false,
        }
    }

    fn ops(&self) -> Vec<Op> {
        self.staged.iter().flat_map(|s| s.ops.iter().cloned()).collect()
    }

    fn change(&self) -> Change {
        Change::of(self.ops(), &self.names)
    }

    fn staged_album(ops: &[Op]) -> Option<Uuid> {
        ops.iter().rev().find_map(|op| if let Op::Album { album, .. } = op { Some(*album) } else { None })
    }

    fn ctx<'a>(&'a self, names: &'a Names, staged_album: Option<Uuid>) -> Ctx<'a> {
        Ctx {
            catalog: &self.catalog,
            vocab: &self.vocab,
            targets: self.targets.len(),
            context_album: None,
            staged_album,
            names,
            may_relate: self.may_relate,
            may_source: self.may_source,
            titles: None,
            artist_names: None,
        }
    }

    pub fn has_changes(&self) -> bool {
        !self.staged.is_empty()
    }

    pub fn chips(&self) -> Vec<Chip> {
        self.staged.iter().map(|s| Chip { text: s.text.clone(), label: s.label.clone(), warning: s.warning.clone() }).collect()
    }

    /// `text` staged: all of it, or (something in it can't be) none of it, saying why.
    pub fn stage(&mut self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let ops = self.ops();
        let (units, _) = resolve::resolve(text, &self.ctx(&self.names, Self::staged_album(&ops)), &mut self.library.clone());
        if units.is_empty() {
            return Err("Nothing to change there: key:value (artist:…, status:…), or a word to find what to change.".into());
        }
        if let Some(err) = units.iter().find_map(|u| u.result.as_ref().err()) {
            return Err(err.clone());
        }
        let mut staged = Vec::new();
        for u in units {
            let Ok(mut r) = u.result else { continue };
            if r.page.is_some() {
                return Err("That one belongs to a track's page.".into());
            }
            match r.want {
                Some(Want::Paste { .. }) | Some(Want::File { .. }) => return Err("Adding a cover isn't here yet.".into()),
                Some(Want::Correct(_)) => return Err("Correcting a source isn't here yet.".into()),
                None => {}
            }
            if r.ops.iter().any(|op| matches!(op, Op::ParseTitle(_))) {
                return Err("Credits in titles aren't here yet (the desktop has them).".into());
            }
            if let Some((id, kind)) = &r.new_kind {
                self.names.set_kind(*id, kind);
            }
            // The aliases it credits, named (for the preview's words).
            for op in &mut r.ops {
                if let Op::Credit { artist, alias: Some(Some(alias)), .. } = op {
                    let known = self.library.aliases.lock().unwrap().get(artist).cloned().flatten();
                    if let Some(a) = known.and_then(|list| list.into_iter().find(|x| x.id == *alias)) {
                        self.names.alias(a.id, &a.name);
                    }
                }
            }
            staged.push(Staged { text: u.text, label: r.label, ops: r.ops, warning: r.warning, editable: true });
        }
        self.staged.extend(staged);
        Ok(())
    }

    /// An image uploaded (see `media::stage_cover`) becomes a cover when the edit is applied:
    /// a change like the typed ones (`name`: what it's called in words).
    pub fn add_staged_cover(&mut self, sha: String, name: &str) {
        let what = format!("the image “{name}”");
        let warning = (self.targets.len() > 1).then(|| format!("each of the {} tracks gets this cover", self.targets.len()));
        let op = Op::Cover { image: super::model::Image::Staged, sha, at: None };
        self.staged.push(Staged { text: format!("cover: {what}"), label: format!("cover: + {what}"), ops: vec![op], warning, editable: false });
    }

    /// Takes back the staged change in this place.
    pub fn unstage(&mut self, n: usize) -> Option<String> {
        (n < self.staged.len()).then(|| self.staged.remove(n)).filter(|s| s.editable).map(|s| s.text)
    }

    /// A choice made in a list: what its text stands for from now on.
    pub fn take(&mut self, c: &Candidate) {
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
    }

    /// A snapshot to list choices from (see `Chooser`).
    pub fn chooser(&self) -> Chooser {
        let ops = self.ops();
        Chooser {
            catalog: self.catalog.clone(),
            vocab: self.vocab.clone(),
            targets: self.targets.len(),
            staged_album: Self::staged_album(&ops),
            names: self.names.clone(),
            may_relate: self.may_relate,
            may_source: self.may_source,
            library: self.library.clone(),
        }
    }

    /// Keeps the engine going.
    pub fn drive(&mut self) {
        let change = self.change();
        self.engine.update(&self.env, &self.catalog, &self.targets, &change, &self.names, None);
        // Applying was waiting for the preview.
        if self.apply_when_ready && self.engine.applied.lock().unwrap().phase == Phase::Editing && self.engine.preview_current() && !self.loading() {
            self.apply_when_ready = false;
            if self.refused() == 0 && self.changing() > 0 {
                self.begin_apply(&change);
            }
        }
        // Applying finished: noted once.
        let finished = {
            let mut a = self.engine.applied.lock().unwrap();
            if a.phase == Phase::Finished && a.finished_unseen {
                a.finished_unseen = false;
                Some((a.outcomes.clone(), a.answers.clone()))
            } else {
                None
            }
        };
        if let Some((outcomes, _)) = finished {
            let mut finish = Finish::default();
            for plan in &self.engine.plans {
                match outcomes.get(&plan.track) {
                    Some(Outcome::Done) => {
                        finish.changed += 1;
                        finish.tracks.push(plan.track);
                    }
                    Some(Outcome::Failed(why)) => {
                        finish.failed += 1;
                        finish.errors.push(format!("{}: {why}", plan.title));
                    }
                    None => {}
                }
            }
            self.finish = Some(finish);
        }
    }

    /// How applying went, once (None: not finished, or already taken).
    pub fn take_finish(&mut self) -> Option<Finish> {
        self.finish.take()
    }

    /// What the session talks to the server with.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The same changes again, on the tracks as they are now (after applying).
    pub fn keep_editing(&mut self) {
        self.engine.forget();
    }

    /// Done with it: nothing staged, nothing kept.
    pub fn clear(&mut self) {
        self.staged.clear();
        self.names = Names::default();
        self.engine.forget();
        self.finish = None;
        self.apply_when_ready = false;
    }

    fn loading(&self) -> bool {
        self.engine.plans.iter().any(|p| matches!(p.state, PlanState::Loading))
    }

    fn changing(&self) -> usize {
        self.engine.plans.iter().filter(|p| matches!(&p.state, PlanState::Ready(r) if r.changes())).count()
    }

    fn refused(&self) -> usize {
        self.engine.preview.lock().unwrap().answers.values().filter(|a| a.error.is_some()).count()
    }

    /// Applies what the preview shows (once it is there, if it isn't yet).
    pub fn apply(&mut self) -> Result<(), String> {
        if self.engine.applied.lock().unwrap().phase != Phase::Editing {
            return Err("It's being applied already.".into());
        }
        if self.staged.is_empty() {
            return Err("There's nothing to change.".into());
        }
        let change = self.change();
        self.drive();
        if self.loading() || !self.engine.preview_current() {
            self.apply_when_ready = true;
            return Ok(());
        }
        if self.refused() > 0 {
            return Err("The server says it can't be written (see the red lines).".into());
        }
        if self.changing() == 0 {
            return Err("It changes nothing.".into());
        }
        self.begin_apply(&change);
        Ok(())
    }

    fn begin_apply(&mut self, change: &Change) {
        // The revisions now: what undoing goes back to.
        self.before = self
            .targets
            .iter()
            .filter_map(|t| {
                let detail = self.engine.detail(*t)?;
                let revisions = detail.revisions.as_ref()?;
                Some((*t, revisions.iter().map(|r| (r.kind.clone(), r.variant_id, r.id)).collect()))
            })
            .collect();
        self.engine.apply(&self.env, change, None);
    }

    /// Puts back what the edit changed: the metadata and the facts of the tracks written,
    /// each restored to the revision it had before (as new revisions). Needs
    /// `restore_revision`. Returns how many tracks it restored.
    pub fn undoer(&self) -> Undoer {
        Undoer { env: self.env.clone(), before: self.before.clone() }
    }

    /// One track's rows (the fields as the changes leave them), with the flags on them.
    pub fn rows(&self) -> Vec<Row> {
        let [track] = self.targets[..] else { return Vec::new() };
        let Some(r) = self.engine.ready(track) else { return Vec::new() };
        let now: Vec<String> = self.catalog.issues.get(&track).map(|f| f.iter().map(|(_, k)| k.clone()).collect()).unwrap_or_default();
        let after = self.after_flags(track);
        let mut aliases: HashMap<Uuid, String> = HashMap::new();
        if let Some(d) = self.engine.detail(track) {
            aliases.extend(d.credits.iter().filter_map(|c| Some((c.alias_id?, c.alias.clone()?))));
            aliases.extend(d.alias_names.iter().map(|(k, v)| (*k, v.clone())));
        }
        for (id, name) in &self.names.aliases {
            aliases.insert(*id, name.clone());
        }
        let names = self.names.stand_ins();
        let covers = self.engine.detail(track).map(|d| d.covers.clone()).unwrap_or_default();
        let know = panel::Know { catalog: &self.catalog, names: &names, aliases: &aliases, ambiguous: &[], covers: &covers };
        panel::rows(r, &know, &now, after.as_deref())
    }

    /// A track's flags after the edit, as the preview (or the edit just applied) says.
    fn after_flags(&self, track: Uuid) -> Option<Vec<String>> {
        let applied = self.engine.applied.lock().unwrap();
        if let Some(a) = applied.answers.get(&track) {
            return Some(a.flags.clone());
        }
        drop(applied);
        let preview = self.engine.preview.lock().unwrap();
        (preview.key == self.engine.plan_key && !preview.running && !self.staged.is_empty()).then(|| preview.answers.get(&track).filter(|a| a.error.is_none()).map(|a| a.flags.clone())).flatten()
    }

    /// The track's title and how it is named, for a heading.
    pub fn report(&self) -> Report {
        let (phase, done, total, outcomes, applied_answers) = {
            let a = self.engine.applied.lock().unwrap();
            (a.phase, a.done, a.total, a.outcomes.clone(), a.answers.clone())
        };
        let (answers, preview_error, running) = {
            let p = self.engine.preview.lock().unwrap();
            (p.answers.clone(), p.error.clone(), p.running)
        };
        let changing = self.changing();
        let refused = answers.values().filter(|a| a.error.is_some()).count();
        let current = self.engine.preview_current();
        let loading = self.loading();
        let can_apply = phase == Phase::Editing && current && changing > refused && !loading;
        let blocked = if phase == Phase::Editing && !self.staged.is_empty() && changing == 0 && !loading && current { Some("It changes nothing.".to_string()) } else { None };
        let change = self.change();
        let single = self.targets.len() == 1;
        let mut tracks = Vec::new();
        for plan in &self.engine.plans {
            let mut report = TrackReport { track: Some(plan.track), title: plan.title.clone(), ..Default::default() };
            match &plan.state {
                PlanState::Loading => continue,
                PlanState::Failed(err) => {
                    report.mark = "✗".into();
                    report.error = Some(err.clone());
                }
                PlanState::Ready(r) if !r.changes() => continue,
                PlanState::Ready(r) => {
                    let answer = applied_answers.get(&plan.track).or_else(|| answers.get(&plan.track));
                    report.lines = r.lines.clone();
                    if let Some(a) = answer {
                        for along in &a.along {
                            report.lines.extend(engine::along_lines(along, &self.catalog));
                        }
                    }
                    report.error = match outcomes.get(&plan.track) {
                        Some(Outcome::Failed(why)) => Some(why.clone()),
                        _ => answer.and_then(|a| a.error.as_ref()).filter(|e| e.code != "stale").map(|e| e.message.clone()),
                    };
                    report.mark = match outcomes.get(&plan.track) {
                        Some(Outcome::Done) => "✓",
                        Some(Outcome::Failed(_)) => "✗",
                        None if report.error.is_some() => "✗",
                        None => "",
                    }
                    .into();
                    report.warnings = r.warnings.clone();
                    if let Some(a) = answer.filter(|a| a.error.is_none()) {
                        let before: Vec<String> = self.catalog.issues.get(&plan.track).map(|f| f.iter().map(|(_, k)| k.clone()).collect()).unwrap_or_default();
                        let name = |k: &String| self.vocab.review_kind_name(k).to_lowercase();
                        report.fixed = before.iter().filter(|k| !a.flags.contains(k)).map(name).collect();
                        report.still = a.flags.iter().filter(|k| before.contains(k)).map(name).collect();
                        report.new = a.flags.iter().filter(|k| !before.contains(k)).map(name).collect();
                    }
                }
            }
            tracks.push(report);
        }
        let _ = single;
        Report { phase, done, total, loading, checking: loading || running || (!current && changing > 0), changing, refused, can_apply, blocked, preview_error, makes: self.makes(&change), tracks }
    }

    /// The new things the changes make, named.
    fn makes(&self, change: &Change) -> Vec<String> {
        let mut new = Vec::new();
        for (id, name) in &self.names.new_artists {
            if change.ops.iter().any(|op| matches!(op, Op::Credit { artist, .. } if artist == id)) {
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
            new.push("a single of its own, for each".into());
        }
        new
    }

    /// Whether something asked for in the background hasn't come (worth listing choices
    /// again when the waker rings).
    pub fn waiting(&self) -> bool {
        self.library.waiting()
    }
}

/// Puts an edit back (see `Session::undoer`): owns what it needs, so it can run after the
/// session is gone.
pub struct Undoer {
    env: Env,
    before: HashMap<Uuid, Vec<(String, Option<Uuid>, Uuid)>>,
}

impl Undoer {
    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }

    /// Restores, for each track and kind of revision it changed, the revision it had
    /// before. Returns how many revisions were restored.
    pub async fn restore(&self, tracks: &[Uuid]) -> Result<usize, String> {
        let mut restored = 0;
        for track in tracks {
            let Some(old) = self.before.get(track) else { continue };
            let known: HashSet<Uuid> = old.iter().map(|(_, _, id)| *id).collect();
            let detail = self.env.api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{track}"), None).await.map_err(|e| e.message)?;
            let Some(now) = detail.revisions else { continue };
            // Each (kind, version) with a revision made since: go back to the newest it had.
            let mut seen: HashSet<(String, Option<Uuid>)> = HashSet::new();
            for r in now.iter().filter(|r| !known.contains(&r.id)) {
                let key = (r.kind.clone(), r.variant_id);
                if !seen.insert(key.clone()) {
                    continue;
                }
                let Some((_, _, back)) = old.iter().find(|(k, v, _)| *k == key.0 && *v == key.1) else { continue };
                self.env
                    .api
                    .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/revisions/{back}/restore"), Some(serde_json::json!({ "reason": "undone" })))
                    .await
                    .map_err(|e| e.message)?;
                restored += 1;
            }
        }
        Ok(restored)
    }
}

/// What the choices for a line need, owned: made by `Session::chooser`, used without the
/// session (the matches are found by searching the whole library).
pub struct Chooser {
    catalog: Arc<Catalog>,
    vocab: Arc<Vocab>,
    targets: usize,
    staged_album: Option<Uuid>,
    names: Names,
    may_relate: bool,
    may_source: bool,
    library: Library,
}

/// The choices for a line being typed.
#[derive(Debug, Clone, Default)]
pub struct Choices {
    /// The last part's values (artists, roles, statuses…).
    pub candidates: Vec<Candidate>,
    /// For plain words: the things to change by that name, each with the line it starts.
    pub actions: Vec<(String, String)>,
    /// The last part, where its value starts and ends in the line (a choice replaces it).
    pub value: (usize, usize),
    /// What was asked for in the background and hasn't come: ask again later.
    pub waiting: bool,
}

impl Chooser {
    pub fn choices(&self, text: &str) -> Choices {
        let parsed = parse::parse(text);
        let last = parsed.current().cloned();
        let group = parsed.last_goes_with();
        let query = |l: &parse::Part| if l.key == K::Cover { resolve::cover_value(l).unwrap_or_default() } else { l.value.clone() };
        let find = |part: SearchPart, value: &str| -> Vec<u32> {
            if value.trim().is_empty() { Vec::new() } else { Search::default().run_part(&self.catalog, &value.trim().to_lowercase(), part, 8).into_iter().map(|h| h.0).collect() }
        };
        let hits = match last.as_ref().and_then(|l| resolve::search_for(l).map(|sp| (sp, query(l)))) {
            Some((sp, value)) => find(sp, &value),
            None => Vec::new(),
        };
        let track_hits = match last.as_ref().and_then(resolve::cover_value) {
            Some(value) => find(SearchPart::Tracks, &value),
            None => Vec::new(),
        };
        let words = parsed.parts.is_empty() && !parsed.words.is_empty();
        let ctx = Ctx {
            catalog: &self.catalog,
            vocab: &self.vocab,
            targets: self.targets,
            context_album: None,
            staged_album: self.staged_album,
            names: &self.names,
            may_relate: self.may_relate,
            may_source: self.may_source,
            titles: None,
            artist_names: None,
        };
        let mut library = self.library.clone();
        let candidates = match &last {
            Some(l) if !words => resolve::candidates(l, group, &ctx, &hits, &track_hits, &mut library),
            _ => Vec::new(),
        };
        let actions = if words { resolve::actions(&parsed.words.join(" "), &ctx) } else { Vec::new() };
        let value = last.map(|l| (l.value_start, l.end)).unwrap_or((text.len(), text.len()));
        Choices { candidates, actions, value, waiting: library.waiting() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::fixture;

    fn session() -> Session {
        let catalog = Arc::new(fixture::sample());
        let vocab = Arc::new(Vocab { statuses: vec!["released".into(), "demo".into()], credit_roles: vec!["primary".into(), "featured".into()], ..Default::default() });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let env = Env { api: crate::api::Api::new(std::env::temp_dir().join("acetate-session-test")), runtime: runtime.handle().clone(), waker: Arc::new(crate::NoWake), generation: 0 };
        std::mem::forget(runtime);
        Session::new(env, catalog, vocab, vec!["00000000-0000-7000-8000-000000000001".parse().unwrap()], true, true)
    }

    #[test]
    fn a_line_is_staged_whole_or_not_at_all() {
        let mut s = session();
        assert!(s.stage("title:Hello").is_ok());
        assert_eq!(s.chips().len(), 1);
        // The second part can't be: neither is staged.
        assert!(s.stage("status:demo | status:nonsense").is_err());
        assert_eq!(s.chips().len(), 1);
        assert_eq!(s.unstage(0).as_deref(), Some("title:Hello"));
        assert!(s.chips().is_empty());
    }

    #[test]
    fn covers_from_the_clipboard_are_refused_for_now() {
        let mut s = session();
        assert!(s.stage("cover:paste").is_err());
    }

    #[test]
    fn choices_for_a_status_are_the_servers_statuses() {
        let s = session();
        let c = s.chooser().choices("status:de");
        assert!(c.candidates.iter().any(|c| c.text == "demo"), "{c:?}");
    }
}
