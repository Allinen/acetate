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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    pub track_id: Uuid,
    pub added_at: String,
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
}

pub struct Playlists {
    api: Api,
    runtime: tokio::runtime::Handle,
    ctx: eframe::egui::Context,
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
}

impl Playlists {
    pub fn new(api: Api, runtime: tokio::runtime::Handle, ctx: eframe::egui::Context) -> Playlists {
        Playlists {
            api,
            runtime,
            ctx,
            dir: None,
            list: None,
            details: HashMap::new(),
            logs: HashMap::new(),
            asked: HashMap::new(),
            answers: Arc::default(),
            notice: None,
            created: None,
        }
    }

    /// Starts over for a login: `dir` is where this server's and user's playlists are kept.
    pub fn reset(&mut self, dir: Option<PathBuf>) {
        self.dir = dir;
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
                    if let (Some(path), Ok(bytes)) = (self.path(id), serde_json::to_vec(&detail)) {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(path, bytes);
                    }
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
            }
        }
    }

    fn path(&self, id: Uuid) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{id}.json")))
    }

    /// Runs `work` in the background; its answers are taken in by `poll`.
    fn spawn<F>(&self, work: F)
    where
        F: std::future::Future<Output = Vec<Answer>> + Send + 'static,
    {
        let (answers, ctx) = (self.answers.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let got = work.await;
            answers.lock().unwrap().extend(got);
            ctx.request_repaint();
        });
    }

    /// A new number for a request for this playlist.
    fn ask(&mut self, id: Uuid) -> u64 {
        let number = self.asked.entry(id).or_default();
        *number += 1;
        *number
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
                Err(err) if err.to_string().starts_with("404") => Answer::Gone(id),
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

    /// Adds tracks at the end.
    pub fn add(&mut self, id: Uuid, name: String, tracks: Vec<Uuid>) {
        let api = self.api.clone();
        let count = tracks.len();
        self.spawn(async move {
            let body = json!({ "tracks": tracks });
            let result = api.send_json::<serde_json::Value>(Method::POST, &format!("/api/playlists/{id}/entries"), Some(body)).await;
            let what = if count == 1 { "1 track".to_string() } else { format!("{count} tracks") };
            vec![
                match result {
                    Ok(_) => Answer::Notice(format!("Added {what} to “{name}”")),
                    Err(err) => Answer::Notice(format!("Couldn't add to “{name}”: {err:#}")),
                },
                Answer::Refetch(id),
            ]
        });
        self.logs.remove(&id);
    }

    /// Removes entries (shown at once).
    pub fn remove(&mut self, id: Uuid, entries: Vec<Uuid>) {
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
        let mut order = None;
        self.change_locally(id, |d| {
            let Some(at) = d.entries.iter().position(|e| e.id == entry) else { return };
            let to = at as isize + by;
            if to < 0 || to as usize >= d.entries.len() {
                return;
            }
            d.entries.swap(at, to as usize);
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

    /// Renames it or makes it public/private.
    pub fn update(&mut self, id: Uuid, name: Option<String>, public: Option<bool>) {
        self.change_locally(id, |d| {
            if let Some(name) = &name {
                d.name = name.clone();
            }
            if let Some(public) = public {
                d.public = public;
            }
        });
        let (api, number) = (self.api.clone(), self.ask(id));
        self.spawn(async move {
            let body = json!({ "name": name, "public": public });
            let result = api.send_json::<Detail>(Method::PATCH, &format!("/api/playlists/{id}"), Some(body)).await;
            let mut answers = match result {
                Ok(detail) => vec![Answer::Detail(id, number, detail)],
                Err(err) => vec![Answer::Notice(format!("Couldn't change it: {err:#}")), Answer::Refetch(id)],
            };
            answers.extend(list(&api).await);
            answers
        });
    }

    /// Moves it to the trash (it can be taken back out, from the web UI for now).
    pub fn trash(&mut self, id: Uuid, name: String) {
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
        if let Some(cached) = self.details.get_mut(&id) {
            if let Some(value) = &cached.value {
                let mut detail = (**value).clone();
                change(&mut detail);
                cached.value = Some(Arc::new(detail));
            }
        }
        self.logs.remove(&id);
    }
}

/// The playlist list, fresh from the server.
async fn list(api: &Api) -> Vec<Answer> {
    match api.send_json::<Vec<Playlist>>(Method::GET, "/api/playlists", None).await {
        Ok(list) => vec![Answer::List(list)],
        Err(_) => vec![],
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
        let mut store = Playlists::new(Api::new(dir.clone()), runtime.handle().clone(), eframe::egui::Context::default());
        store.reset(Some(dir.join("playlists")));
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

        // Add the second, twice (duplicates are allowed), then move the last one up.
        store.add(id, "client test".into(), vec![tracks[1], tracks[1]]);
        wait(&mut store, &|s| entries(s, id).len() == 3);
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
        store.update(id, Some("client test (renamed)".into()), Some(true));
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
