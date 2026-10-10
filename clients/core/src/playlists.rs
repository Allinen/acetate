//! Playlists in the client: the list (from the catalog, or fresher after a change made
//! here) and each playlist's entries, kept on disk and refreshed in the background (the
//! server's copy wins, as for the catalog). Changes go to the server; removing and
//! reordering show at once, and a playlist is fetched again after each change (or
//! error), so what's shown is always what the server has.
//!
//! "Liked songs" isn't here: it isn't a playlist on the server, just the starred
//! tracks (see `ui::views::playlist`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::Api;
use crate::catalog::model::Playlist;
use crate::catalog::{Cached, Catalog};

/// A playlist with its entries (`GET /api/playlists/{id}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detail {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub public: bool,
    pub owner: String,
    pub mine: bool,
    pub entries: Vec<Entry>,
    /// When it last changed, as the server said when it was fetched.
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub track_id: Uuid,
    pub added_at: String,
    /// Added here, waiting to be sent (in the outbox); `id` is made up until then.
    #[serde(default)]
    pub waiting: bool,
}

/// One change in a playlist's log (`GET /api/playlists/{id}/log`).
#[derive(Debug, Clone, Deserialize)]
pub struct LogEntry {
    pub id: Uuid,
    pub at: String,
    pub by: Option<String>,
    pub action: String,
    pub detail: serde_json::Value,
    pub undone_at: Option<String>,
}

impl LogEntry {
    /// What the change did, in words.
    pub fn describe(&self) -> String {
        let count = |key: &str| self.detail.get(key).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let entries = |n: usize| if n == 1 { "1 track".to_string() } else { format!("{n} tracks") };
        match self.action.as_str() {
            "add" => format!("Added {}", entries(count("entries"))),
            "remove" => format!("Removed {}", entries(count("entries"))),
            "reorder" => "Changed the order".into(),
            "undo" => format!("Undid a change: {}", self.detail.get("summary").and_then(|s| s.as_str()).unwrap_or("")),
            "import" => format!(
                "Imported from {}",
                self.detail.get("from").and_then(|s| s.as_str()).unwrap_or("elsewhere")
            ),
            other => other.to_string(),
        }
    }

    /// Whether it can be undone (not already, and not an undo itself).
    pub fn undoable(&self) -> bool {
        self.undone_at.is_none() && matches!(self.action.as_str(), "add" | "remove" | "reorder")
    }
}

/// What came back from the server.
enum Answer {
    List(Vec<Playlist>),
    /// A playlist as the server has it, for the request numbered so (see `asked`).
    Detail(Uuid, u64, Detail),
    Gone(Uuid),
    Log(Uuid, Vec<LogEntry>),
    Created { id: Uuid, name: String },
    /// Something to tell the user ("Added 3 tracks to X", or an error).
    Notice(String),
    /// A change failed: fetch this playlist again.
    Refetch(Uuid),
    /// The server is away: the copy kept here is what there is (nothing to tell).
    Offline(Uuid),
}

pub struct Playlists {
    api: Api,
    runtime: tokio::runtime::Handle,
    waker: crate::Waker,
    /// Where each playlist is kept (None until logged in).
    dir: Option<PathBuf>,
    /// The list, fresher than the catalog's after a change here (and when that was).
    list: Option<(Vec<Playlist>, Instant)>,
    details: HashMap<Uuid, Cached<Detail>>,
    logs: HashMap<Uuid, Vec<LogEntry>>,
    /// Numbers each request for a playlist: only the latest one's answer is kept (an
    /// older one arriving later would show the playlist as it was before a change).
    asked: HashMap<Uuid, u64>,
    answers: Arc<Mutex<Vec<Answer>>>,
    /// The last thing to tell the user, and when.
    pub notice: Option<(String, Instant)>,
    /// A playlist just made here: the app opens it.
    pub created: Option<Uuid>,
    /// Where additions go (see `outbox`).
    outbox: Option<Arc<crate::outbox::Outbox>>,
}

impl Playlists {
    pub fn new(api: Api, runtime: tokio::runtime::Handle, waker: crate::Waker) -> Playlists {
        Playlists {
            api,
            runtime,
            waker,
            dir: None,
            list: None,
            details: HashMap::new(),
            logs: HashMap::new(),
            asked: HashMap::new(),
            answers: Arc::default(),
            notice: None,
            created: None,
            outbox: None,
        }
    }

    /// Starts over for a login: `dir` is where this server's and user's playlists are kept;
    /// `outbox`, where additions go.
    pub fn reset(&mut self, dir: Option<PathBuf>, outbox: Option<Arc<crate::outbox::Outbox>>) {
        self.dir = dir;
        self.outbox = outbox;
        self.list = None;
        self.details.clear();
        self.logs.clear();
    }

    /// A catalog asked for at `asked` arrived from the server: its list is at least as
    /// new as a change made before then.
    pub fn catalog_arrived(&mut self, asked: Instant) {
        if self.list.as_ref().is_some_and(|(_, at)| *at <= asked) {
            self.list = None;
        }
    }

    /// The playlists: the user's own first, then public ones.
    pub fn list<'a>(&'a self, catalog: Option<&'a Catalog>) -> &'a [Playlist] {
        match (&self.list, catalog) {
            (Some((list, _)), _) => list,
            (None, Some(c)) => &c.playlists,
            (None, None) => &[],
        }
    }

    /// The playlist with its entries, as kept (from disk at first); fetched in the
    /// background the first time it's asked for in this session.
    pub fn detail(&mut self, id: Uuid) -> &Cached<Detail> {
        if !self.details.contains_key(&id) {
            let kept = self.path(id).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Detail>(&b).ok());
            self.details.insert(id, Cached { value: kept.map(Arc::new), ..Cached::default() });
            self.fetch(id);
        }
        &self.details[&id]
    }

    pub fn log(&self, id: Uuid) -> Option<&[LogEntry]> {
        self.logs.get(&id).map(Vec::as_slice)
    }

    /// Takes in the server's answers. Call once per frame.
    pub fn poll(&mut self) {
        // Taken out first: the lock is released before answering them (which may start
        // new work).
        let answers = std::mem::take(&mut *self.answers.lock().unwrap());
        for answer in answers {
            match answer {
                Answer::List(list) => self.list = Some((list, Instant::now())),
                Answer::Detail(id, number, _) if self.asked.get(&id) != Some(&number) => {}
                Answer::Detail(id, _, detail) => {
                    self.keep(id, &detail);
                    let cached = self.details.entry(id).or_default();
                    cached.value = Some(Arc::new(detail));
                    cached.refreshing = false;
                    cached.error = None;
                    cached.confirmed_at = Some(std::time::SystemTime::now());
                }
                Answer::Gone(id) => {
                    // Kept as gone (not removed), so it isn't fetched again and again.
                    self.details.insert(id, Cached { error: Some("This playlist isn't there anymore.".into()), ..Cached::default() });
                }
                Answer::Log(id, log) => {
                    self.logs.insert(id, log);
                }
                Answer::Created { id, name } => {
                    self.created = Some(id);
                    self.notice = Some((format!("Made the playlist “{name}”"), Instant::now()));
                }
                Answer::Notice(text) => self.notice = Some((text, Instant::now())),
                Answer::Refetch(id) => self.fetch(id),
                Answer::Offline(id) => {
                    if let Some(cached) = self.details.get_mut(&id) {
                        cached.refreshing = false;
                    }
                }
            }
        }
    }

    fn path(&self, id: Uuid) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{id}.json")))
    }

    /// Writes a playlist to disk (best effort: it's fetched again next time).
    fn keep(&self, id: Uuid, detail: &Detail) {
        if let (Some(path), Ok(bytes)) = (self.path(id), serde_json::to_vec(detail)) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, bytes);
        }
    }

    /// Runs `work` in the background; its answers are taken in by `poll`.
    fn spawn<F>(&self, work: F)
    where
        F: std::future::Future<Output = Vec<Answer>> + Send + 'static,
    {
        let (answers, waker) = (self.answers.clone(), self.waker.clone());
        self.runtime.spawn(async move {
            let got = work.await;
            answers.lock().unwrap().extend(got);
            waker.changed();
        });
    }

    /// For a change only the server can make (`what`): whether it can be made now. Offline
    /// it can't, and the user is told so. (Adding tracks can: it waits in the outbox.)
    fn needs_server(&self, what: &str) -> bool {
        if !self.api.connection().is_offline() {
            return true;
        }
        // (Taken in by `poll`, and the app woken afterwards: whoever called this may hold
        // the lock the app's waker takes.)
        self.answers.lock().unwrap().push(Answer::Notice(format!("Offline: {what} needs the server.")));
        let waker = self.waker.clone();
        self.runtime.spawn(async move { waker.changed() });
        false
    }

    /// A new number for a request for this playlist.
    fn ask(&mut self, id: Uuid) -> u64 {
        let number = self.asked.entry(id).or_default();
        *number += 1;
        *number
    }

    /// A catalog arrived: every playlist whose copy here is older than it says (or missing)
    /// is fetched, one after another in the background, so each is on disk to use offline.
    pub fn keep_all_fresh(&mut self, catalog: &Catalog) {
        let stale: Vec<Uuid> = catalog
            .playlists
            .iter()
            .filter(|p| {
                let kept = self.details.get(&p.id).and_then(|c| c.value.clone()).or_else(|| {
                    let bytes = std::fs::read(self.path(p.id)?).ok()?;
                    serde_json::from_slice::<Detail>(&bytes).ok().map(Arc::new)
                });
                is_stale(kept.as_deref(), p)
            })
            .map(|p| p.id)
            .collect();
        if stale.is_empty() {
            return;
        }
        let asked: Vec<(Uuid, u64)> = stale.iter().map(|&id| (id, self.ask(id))).collect();
        let api = self.api.clone();
        self.spawn(async move {
            let mut answers = Vec::new();
            for (id, number) in asked {
                match api.send_json::<Detail>(Method::GET, &format!("/api/playlists/{id}"), None).await {
                    Ok(detail) => answers.push(Answer::Detail(id, number, detail)),
                    // (Gone meanwhile, or the server went away: the next catalog says.)
                    Err(err) if err.status.is_none() => break,
                    Err(_) => {}
                }
            }
            answers
        });
    }

    /// Fetches a playlist (and the list, whose counts change with it).
    pub fn fetch(&mut self, id: Uuid) {
        if let Some(cached) = self.details.get_mut(&id) {
            cached.refreshing = true;
        }
        let number = self.ask(id);
        let api = self.api.clone();
        self.spawn(async move {
            let detail = api.send_json::<Detail>(Method::GET, &format!("/api/playlists/{id}"), None).await;
            let mut answers = vec![match detail {
                Ok(detail) => Answer::Detail(id, number, detail),
                Err(err) if err.status == Some(404) => Answer::Gone(id),
                Err(err) if err.code == "offline" => return vec![Answer::Offline(id)],
                Err(err) => Answer::Notice(format!("Couldn't load the playlist: {err:#}")),
            }];
            answers.extend(list(&api).await);
            answers
        });
    }

    pub fn fetch_log(&self, id: Uuid) {
        let api = self.api.clone();
        self.spawn(async move {
            match api.send_json::<Vec<LogEntry>>(Method::GET, &format!("/api/playlists/{id}/log"), None).await {
                Ok(log) => vec![Answer::Log(id, log)],
                Err(err) => vec![Answer::Notice(format!("Couldn't load the history: {err:#}"))],
            }
        });
    }

    /// Makes a playlist, with these tracks in it.
    pub fn create(&self, name: String, tracks: Vec<Uuid>) {
        if !self.needs_server("making a playlist") {
            return;
        }
        let api = self.api.clone();
        self.spawn(async move {
            #[derive(Deserialize)]
            struct Created {
                id: Uuid,
            }
            let body = json!({ "name": name, "tracks": tracks });
            let mut answers = match api.send_json::<Created>(Method::POST, "/api/playlists", Some(body)).await {
                Ok(created) => vec![Answer::Created { id: created.id, name }],
                Err(err) => vec![Answer::Notice(format!("Couldn't make the playlist: {err:#}"))],
            };
            answers.extend(list(&api).await);
            answers
        });
    }

    /// Adds tracks at the end: shown at once (waiting), sent through the outbox (now, or
    /// when the server is back). It answers with a notice and the playlist fetched again.
    pub fn add(&mut self, id: Uuid, name: String, tracks: Vec<Uuid>) {
        let Some(outbox) = self.outbox.clone() else { return };
        let added = tracks.iter().map(|&track_id| Entry { id: Uuid::new_v4(), track_id, added_at: String::new(), waiting: true });
        self.change_locally(id, |d| d.entries.extend(added));
        if let Some(detail) = self.details.get(&id).and_then(|c| c.value.clone()) {
            self.keep(id, &detail);
        }
        outbox.push(crate::outbox::Change::Add { playlist: id, name, tracks });
    }

    /// Removes entries (shown at once).
    pub fn remove(&mut self, id: Uuid, entries: Vec<Uuid>) {
        if !self.needs_server("taking tracks out of a playlist") {
            return;
        }
        self.change_locally(id, |d| d.entries.retain(|e| !entries.contains(&e.id)));
        let api = self.api.clone();
        self.spawn(async move {
            let body = json!({ "entries": entries });
            let result = api.send(Method::DELETE, &format!("/api/playlists/{id}/entries"), Some(body)).await;
            let mut answers = vec![Answer::Refetch(id)];
            if let Err(err) = result {
                answers.push(Answer::Notice(format!("Couldn't remove it: {err:#}")));
            }
            answers
        });
    }

    /// Moves an entry up or down by one (shown at once).
    pub fn move_entry(&mut self, id: Uuid, entry: Uuid, by: isize) {
        let mut at = None;
        self.change_locally(id, |d| at = d.entries.iter().position(|e| e.id == entry));
        if let Some(at) = at {
            self.move_entry_to(id, entry, at as isize + by);
        }
    }

    /// Puts the entries in this order (shown at once, one request): `order` has every entry.
    pub fn set_order(&mut self, id: Uuid, order: Vec<Uuid>) {
        if !self.needs_server("reordering a playlist") {
            return;
        }
        self.change_locally(id, |d| {
            let mut by_id: std::collections::HashMap<Uuid, _> = d.entries.drain(..).map(|e| (e.id, e)).collect();
            let mut entries: Vec<_> = order.iter().filter_map(|i| by_id.remove(i)).collect();
            // (An entry the order left out keeps its place at the end.)
            entries.extend(by_id.into_values());
            d.entries = entries;
        });
        let api = self.api.clone();
        self.spawn(async move {
            match api.send(Method::PUT, &format!("/api/playlists/{id}/order"), Some(json!({ "entries": order }))).await {
                Err(err) => vec![Answer::Notice(format!("Couldn't reorder it: {err:#}")), Answer::Refetch(id)],
                Ok(()) => vec![],
            }
        });
    }

    /// Moves an entry to this place (shown at once, one request).
    pub fn move_entry_to(&mut self, id: Uuid, entry: Uuid, to: isize) {
        if !self.needs_server("reordering a playlist") {
            return;
        }
        let mut order = None;
        self.change_locally(id, |d| {
            let Some(at) = d.entries.iter().position(|e| e.id == entry) else { return };
            if to < 0 || to as usize >= d.entries.len() || to as usize == at {
                return;
            }
            let moved = d.entries.remove(at);
            d.entries.insert(to as usize, moved);
            order = Some(d.entries.iter().map(|e| e.id).collect::<Vec<_>>());
        });
        let Some(order) = order else { return };
        let api = self.api.clone();
        self.spawn(async move {
            let body = json!({ "entries": order });
            match api.send(Method::PUT, &format!("/api/playlists/{id}/order"), Some(body)).await {
                Err(err) => vec![Answer::Notice(format!("Couldn't move it: {err:#}")), Answer::Refetch(id)],
                // No refetch on success: moving several times quickly would flicker.
                Ok(()) => vec![],
            }
        });
    }

    /// Renames it, describes it, or makes it public/private.
    pub fn update(&mut self, id: Uuid, name: Option<String>, public: Option<bool>, description: Option<String>) {
        if !self.needs_server("changing a playlist") {
            return;
        }
        self.change_locally(id, |d| {
            if let Some(name) = &name {
                d.name = name.clone();
            }
            if let Some(public) = public {
                d.public = public;
            }
            if let Some(description) = &description {
                d.description = description.clone();
            }
        });
        let (api, number) = (self.api.clone(), self.ask(id));
        self.spawn(async move {
            let body = json!({ "name": name, "public": public, "description": description });
            let result = api.send_json::<Detail>(Method::PATCH, &format!("/api/playlists/{id}"), Some(body)).await;
            let mut answers = match result {
                Ok(detail) => vec![Answer::Detail(id, number, detail)],
                Err(err) => vec![Answer::Notice(format!("Couldn't change it: {err:#}")), Answer::Refetch(id)],
            };
            answers.extend(list(&api).await);
            answers
        });
    }

    /// Moves it to the trash (it's taken back out on the Trash page).
    pub fn trash(&mut self, id: Uuid, name: String) {
        if !self.needs_server("moving a playlist to the trash") {
            return;
        }
        let api = self.api.clone();
        self.spawn(async move {
            let mut answers = match api.send(Method::DELETE, &format!("/api/playlists/{id}"), None).await {
                Err(err) => vec![Answer::Notice(format!("Couldn't move it to the trash: {err:#}"))],
                Ok(()) => vec![Answer::Notice(format!("Moved “{name}” to the trash")), Answer::Gone(id)],
            };
            answers.extend(list(&api).await);
            answers
        });
    }

    pub fn undo(&mut self, id: Uuid, change: Uuid) {
        if !self.needs_server("undoing") {
            return;
        }
        let api = self.api.clone();
        self.spawn(async move {
            #[derive(Deserialize)]
            struct Undone {
                summary: String,
            }
            let result = api.send_json::<Undone>(Method::POST, &format!("/api/playlists/{id}/log/{change}/undo"), None).await;
            let mut answers = vec![
                match result {
                    Ok(undone) => Answer::Notice(format!("Undone: {}", undone.summary)),
                    Err(err) => Answer::Notice(format!("Couldn't undo it: {err:#}")),
                },
                Answer::Refetch(id),
            ];
            if let Ok(log) = api.send_json::<Vec<LogEntry>>(Method::GET, &format!("/api/playlists/{id}/log"), None).await {
                answers.push(Answer::Log(id, log));
            }
            answers
        });
    }

    fn change_locally(&mut self, id: Uuid, change: impl FnOnce(&mut Detail)) {
        // A fetch already on its way has the playlist as it was: its answer is dropped.
        self.ask(id);
        if let Some(cached) = self.details.get_mut(&id)
            && let Some(value) = &cached.value
        {
            let mut detail = (**value).clone();
            change(&mut detail);
            cached.value = Some(Arc::new(detail));
        }
        self.logs.remove(&id);
    }
}

/// Whether the copy of a playlist kept here (if any) is older than the catalog says it is.
fn is_stale(kept: Option<&Detail>, listed: &Playlist) -> bool {
    match (kept, &listed.updated_at) {
        (None, _) => true,
        (Some(kept), Some(_)) => kept.updated_at != listed.updated_at,
        // A catalog from before it said when: by the count.
        (Some(kept), None) => kept.entries.len() as i64 != listed.entries,
    }
}

/// The playlist list, fresh from the server.
async fn list(api: &Api) -> Vec<Answer> {
    match api.send_json::<Vec<Playlist>>(Method::GET, "/api/playlists", None).await {
        Ok(list) => vec![Answer::List(list)],
        Err(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_server_only_changes_are_refused_with_the_reason() {
        let dir = tempfile::tempdir().unwrap();
        let away = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let session = crate::api::Session { server: format!("http://{away}"), token: "t".into(), username: "ann".into(), instance: None };
        std::fs::write(dir.path().join("session.json"), serde_json::to_vec(&session).unwrap()).unwrap();
        let api = Api::new(dir.path().to_path_buf());
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let _ = runtime.block_on(api.send(Method::GET, "/api/vocab", None));
        assert!(api.connection().is_offline());
        let mut playlists = Playlists::new(api, runtime.handle().clone(), Arc::new(crate::NoWake));
        playlists.remove(Uuid::nil(), vec![Uuid::nil()]);
        playlists.poll();
        assert_eq!(playlists.notice.map(|n| n.0).as_deref(), Some("Offline: taking tracks out of a playlist needs the server."));
    }

    #[test]
    fn a_kept_playlist_is_fetched_again_when_the_catalog_says_it_changed() {
        let listed = |at: Option<&str>, entries| Playlist {
            id: Uuid::nil(),
            name: "Mix".into(),
            description: String::new(),
            public: false,
            owner: "ann".into(),
            mine: true,
            entries,
            duration_ms: 0,
            updated_at: at.map(str::to_string),
        };
        let entry = Entry { id: Uuid::nil(), track_id: Uuid::nil(), added_at: String::new(), waiting: false };
        let kept = |at: Option<&str>| Detail {
            id: Uuid::nil(),
            name: "Mix".into(),
            description: String::new(),
            public: false,
            owner: "ann".into(),
            mine: true,
            entries: vec![entry.clone()],
            updated_at: at.map(str::to_string),
        };
        let (t1, t2) = (Some("2026-10-01T10:00:00Z"), Some("2026-10-02T10:00:00Z"));
        assert!(is_stale(None, &listed(t1, 1)), "not kept yet");
        assert!(!is_stale(Some(&kept(t1)), &listed(t1, 1)));
        assert!(is_stale(Some(&kept(t1)), &listed(t2, 1)), "reordered or renamed: the same count, a new time");
        assert!(is_stale(Some(&kept(None)), &listed(t1, 1)), "kept before copies said when");
        assert!(!is_stale(Some(&kept(t1)), &listed(None, 1)));
        assert!(is_stale(Some(&kept(t1)), &listed(None, 2)));
    }
}

/// Against a real server: `ACETATE_TEST_SERVER=http://… ACETATE_TEST_TOKEN=…
/// ACETATE_TEST_TRACKS=<id>,<id> cargo test playlists_against -- --ignored`. Makes a
/// playlist named "client test" (and leaves it in the trash).
#[cfg(test)]
mod against_a_server {
    use super::*;
    use std::time::Duration;

    #[test]
    #[ignore]
    fn playlists_against_a_server() {
        let (Ok(server), Ok(token), Ok(tracks)) =
            (std::env::var("ACETATE_TEST_SERVER"), std::env::var("ACETATE_TEST_TOKEN"), std::env::var("ACETATE_TEST_TRACKS"))
        else {
            return;
        };
        let tracks: Vec<Uuid> = tracks.split(',').map(|t| t.parse().unwrap()).collect();
        let dir = std::env::temp_dir().join(format!("acetate-playlists-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let api = Api::new(dir.clone());
        let mut store = Playlists::new(api.clone(), runtime.handle().clone(), std::sync::Arc::new(crate::NoWake));
        let outbox = crate::outbox::Outbox::open(dir.join("outbox-test.json"), api, runtime.handle().clone(), std::sync::Arc::new(crate::NoWake));
        store.reset(Some(dir.join("playlists")), Some(outbox.clone()));
        // Polls until `done` says so (or gives up).
        let wait = |store: &mut Playlists, done: &dyn Fn(&mut Playlists) -> bool| {
            for _ in 0..100 {
                store.poll();
                if done(store) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let shown: Vec<_> = store.details.values().filter_map(|c| c.value.as_ref()).map(|d| (d.name.clone(), d.entries.len())).collect();
            panic!("timed out; last notice: {:?}; playlists: {shown:?}", store.notice);
        };
        let entries = |store: &mut Playlists, id: Uuid| -> Vec<Uuid> {
            store.details.get(&id).and_then(|c| c.value.as_ref()).map(|d| d.entries.iter().map(|e| e.track_id).collect()).unwrap_or_default()
        };

        store.create("client test".into(), vec![tracks[0]]);
        wait(&mut store, &|s| s.created.is_some());
        let id = store.created.take().unwrap();
        store.detail(id);
        wait(&mut store, &|s| s.details.get(&id).is_some_and(|c| c.value.is_some()));
        assert_eq!(entries(&mut store, id), [tracks[0]]);

        // Add the second, twice (duplicates are allowed): shown at once, waiting, then as
        // the server has it once sent. Then move the last one up.
        store.add(id, "client test".into(), vec![tracks[1], tracks[1]]);
        assert_eq!(entries(&mut store, id).len(), 3);
        wait(&mut store, &|s| {
            for sent in outbox.take() {
                if let crate::outbox::Sent::Playlist(p) = sent {
                    s.fetch(p);
                }
            }
            s.details[&id].value.as_ref().is_some_and(|d| d.entries.len() == 3 && d.entries.iter().all(|e| !e.waiting))
        });
        let last = store.details[&id].value.as_ref().unwrap().entries[2].id;
        store.move_entry(id, last, -1);
        assert_eq!(entries(&mut store, id), [tracks[0], tracks[1], tracks[1]], "shown at once");
        std::thread::sleep(Duration::from_millis(300));
        store.fetch(id);
        wait(&mut store, &|s| !s.details[&id].refreshing);
        assert_eq!(store.details[&id].value.as_ref().unwrap().entries[1].id, last, "the server has the new order");

        // Remove the first; rename; make it public.
        let first = store.details[&id].value.as_ref().unwrap().entries[0].id;
        store.remove(id, vec![first]);
        assert_eq!(entries(&mut store, id).len(), 2, "shown at once");
        store.update(id, Some("client test (renamed)".into()), Some(true), None);
        wait(&mut store, &|s| s.details[&id].value.as_ref().is_some_and(|d| d.name == "client test (renamed)" && d.public && d.entries.len() == 2));
        wait(&mut store, &|s| s.list(None).iter().any(|p| p.id == id && p.name == "client test (renamed)"));

        // The history, and undoing the removal.
        store.fetch_log(id);
        wait(&mut store, &|s| s.log(id).is_some_and(|l| l.iter().any(|e| e.action == "remove")));
        let removal = store.log(id).unwrap().iter().find(|e| e.action == "remove").unwrap().clone();
        assert_eq!(removal.describe(), "Removed 1 track");
        store.undo(id, removal.id);
        wait(&mut store, &|s| entries(s, id).len() == 3);

        store.trash(id, "client test (renamed)".into());
        wait(&mut store, &|s| s.details[&id].error.is_some());
        wait(&mut store, &|s| !s.list(None).iter().any(|p| p.id == id));
    }
}
