//! Music kept on this device to play without the server (the phone's downloads).
//!
//! What to keep is a list of pins: a track, an album, a playlist, or Liked songs. The
//! caller works out which files those are now (they change: a playlist gets a track, a star
//! is added) and says so with `set_wanted`; this fetches what's missing, one file at a time,
//! into the content-addressed `blobs` store (hashed as it comes, resumed after a stop), and
//! takes away what's no longer wanted (the user took a pin off). A file is looked up by its
//! hash on the server (`Want::source`), whatever it is here: the original, or, when the
//! server only gave a copy (a lower quality chosen, or no right to originals), the copy,
//! named by its own hash.
//!
//! Downloads only run while `allowed` (the app says so: Wi-Fi, or mobile data if the user
//! allows it).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::Api;
use crate::blobs::{self, Store};

/// A file to have: how the server names it (the hash of the file as it keeps it) and where
/// to ask for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Want {
    pub source: String,
    pub file_id: Uuid,
    /// Its cover's key (16 hex digits), to keep the cover too.
    pub cover: Option<String>,
}

/// Why something is kept: a track, an album, a playlist or "liked" (id empty).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub kind: String,
    pub id: String,
    /// What to call it where downloads are listed.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Kept {
    /// The blob's hash (the file's own, so the original's when it's the original).
    blob: String,
    size: u64,
    /// The quality asked for (None: the server's own choice).
    kbps: Option<u32>,
    /// It is the file as the server keeps it (its hash is `source`).
    original: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    files: HashMap<String, Kept>,
    pins: Vec<Pin>,
}

#[derive(Default)]
struct Inner {
    index: Index,
    queue: VecDeque<Want>,
    wanted: HashSet<String>,
    /// Covers still to fetch (keys).
    cover_queue: VecDeque<String>,
    /// What's being fetched now.
    current: Option<String>,
    failed: Vec<(String, String)>,
    /// Fetched in this run (for "3 of 40").
    done: usize,
}

/// What the app shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub files: usize,
    pub bytes: u64,
    pub queued: usize,
    pub downloading: bool,
    /// Of this run's.
    pub asked: usize,
    pub done: usize,
    pub failed: usize,
    /// Stopped by the user: nothing comes until `resume`.
    pub paused: bool,
    /// Bytes of the file coming now.
    pub current_bytes: u64,
    /// Every byte fetched since the app started.
    pub received: u64,
}

/// The tracks one pin covers (places in the catalog): an album's, a playlist's, the liked
/// ones… None while that can't be worked out (a playlist still being fetched).
pub fn pin_tracks(
    pin: &Pin,
    catalog: &crate::catalog::Catalog,
    playlist_tracks: &mut dyn FnMut(Uuid) -> Option<Vec<Uuid>>,
    liked: &dyn Fn() -> Vec<u32>,
) -> Option<Vec<u32>> {
    let id = pin.id.parse::<Uuid>().ok();
    match pin.kind.as_str() {
        "track" => Some(id.and_then(|id| catalog.track_index(id)).into_iter().collect()),
        "album" => Some(id.map(|id| catalog.album_tracks(id).to_vec()).unwrap_or_default()),
        "playlist" => id.and_then(|id| playlist_tracks(id)).map(|found| found.into_iter().filter_map(|t| catalog.track_index(t)).collect()),
        "liked" => Some(liked()),
        _ => Some(Vec::new()),
    }
}

/// The file of a track as something to want (None: the catalog doesn't know its hash).
pub fn want_of(catalog: &crate::catalog::Catalog, track: u32) -> Option<Want> {
    let t = catalog.track_at(track);
    Some(Want { source: t.sha256()?, file_id: t.file_id(), cover: t.cover().map(|c| format!("{:016x}", c.key.0)) })
}

/// The files the pins want now. `playlist_tracks` says a playlist's tracks (None: not known
/// yet); `liked`, the starred ones. The second answer is false while some pin couldn't be
/// worked out (then nothing should be taken away).
pub fn wanted(
    pins: &[Pin],
    catalog: &crate::catalog::Catalog,
    playlist_tracks: &mut dyn FnMut(Uuid) -> Option<Vec<Uuid>>,
    liked: &dyn Fn() -> Vec<u32>,
) -> (Vec<Want>, bool) {
    let mut complete = true;
    let mut seen = HashSet::new();
    let mut wants = Vec::new();
    for pin in pins {
        match pin_tracks(pin, catalog, playlist_tracks, liked) {
            Some(tracks) => wants.extend(tracks.into_iter().filter_map(|t| want_of(catalog, t)).filter(|w| seen.insert(w.source.clone()))),
            None => complete = false,
        }
    }
    (wants, complete)
}

pub struct Offline {
    store: Store,
    file: PathBuf,
    /// Where the covers of what's kept are (`<key>-small`, `<key>-standard`).
    covers: PathBuf,
    api: Api,
    runtime: tokio::runtime::Handle,
    waker: crate::Waker,
    inner: Mutex<Inner>,
    /// 0: the server's own choice.
    kbps: AtomicU32,
    allowed: AtomicBool,
    /// The user stopped them (kept to go on from).
    paused: AtomicBool,
    working: AtomicBool,
    stop: AtomicBool,
    current_bytes: AtomicU64,
    /// Every byte fetched since the app started (for a speed).
    received: AtomicU64,
}

impl Offline {
    /// `dir`: where the files and the list of what's kept live.
    pub fn open(dir: &Path, api: Api, runtime: tokio::runtime::Handle, waker: crate::Waker) -> anyhow::Result<Arc<Offline>> {
        let store = Store::open(dir)?;
        store.clear_leftovers();
        let file = dir.join("offline.json");
        let covers = dir.join("covers");
        let index = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Ok(Arc::new(Offline {
            store,
            file,
            covers,
            api,
            runtime,
            waker,
            inner: Mutex::new(Inner { index, ..Inner::default() }),
            kbps: AtomicU32::new(0),
            allowed: AtomicBool::new(true),
            paused: AtomicBool::new(false),
            working: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            current_bytes: AtomicU64::new(0),
            received: AtomicU64::new(0),
        }))
    }

    /// The quality to fetch at from now on (None: the server's own choice: the original if
    /// the user may have it).
    pub fn set_quality(&self, kbps: Option<u32>) {
        self.kbps.store(kbps.unwrap_or(0), Ordering::SeqCst);
    }

    /// Whether downloads may run now (the app's say: the connection). Going on again picks
    /// up the queue.
    pub fn set_allowed(self: &Arc<Self>, allowed: bool) {
        self.allowed.store(allowed, Ordering::SeqCst);
        if allowed {
            self.stop.store(false, Ordering::SeqCst);
            self.kick();
        } else {
            // What's coming stops (kept to go on from).
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Stops fetching (what's here stays, what's coming waits).
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.waker.changed();
    }

    /// Goes on with what's coming.
    pub fn resume(self: &Arc<Self>) {
        self.paused.store(false, Ordering::SeqCst);
        self.stop.store(false, Ordering::SeqCst);
        self.waker.changed();
        self.kick();
    }

    pub fn pins(&self) -> Vec<Pin> {
        self.inner.lock().unwrap().index.pins.clone()
    }

    pub fn is_pinned(&self, kind: &str, id: &str) -> bool {
        self.inner.lock().unwrap().index.pins.iter().any(|p| p.kind == kind && p.id == id)
    }

    /// Adds or takes off a pin (the caller then says what's wanted).
    pub fn pin(&self, pin: Pin, on: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.index.pins.retain(|p| !(p.kind == pin.kind && p.id == pin.id));
            if on {
                inner.index.pins.push(pin);
            }
            self.save(&inner.index);
        }
        self.waker.changed();
    }

    /// What's wanted now: what's missing is fetched, and what's here but not wanted is taken
    /// away, unless `complete` is false (some pin can't be worked out yet, a playlist still
    /// being fetched: then nothing is taken away, what's known to be wanted just comes).
    pub fn set_wanted(self: &Arc<Self>, wants: Vec<Want>, complete: bool) {
        let all_wants = wants.clone();
        {
            let mut inner = self.inner.lock().unwrap();
            inner.wanted = wants.iter().map(|w| w.source.clone()).collect();
            if !complete {
                // What's kept stays wanted.
                let kept: Vec<String> = inner.index.files.keys().cloned().collect();
                inner.wanted.extend(kept);
            }
            // Taken off: gone.
            let unwanted: Vec<String> = inner.index.files.keys().filter(|s| !inner.wanted.contains(*s)).cloned().collect();
            for source in unwanted {
                if let Some(kept) = inner.index.files.remove(&source)
                    && !inner.index.files.values().any(|k| k.blob == kept.blob)
                {
                    let _ = self.store.remove(&kept.blob);
                }
            }
            let wanted = inner.wanted.clone();
            inner.queue.retain(|w| wanted.contains(&w.source));
            let queued: HashSet<String> = inner.queue.iter().map(|w| w.source.clone()).collect();
            let current = inner.current.clone();
            for want in wants {
                if !inner.index.files.contains_key(&want.source) && !queued.contains(&want.source) && current.as_ref() != Some(&want.source) {
                    inner.queue.push_back(want);
                }
            }
            if inner.queue.is_empty() && inner.current.is_none() {
                inner.done = 0;
                inner.failed.clear();
            }
            inner.failed.retain(|(s, _)| wanted.contains(s));
            // Covers of what's wanted: the ones not here are fetched; with the whole picture
            // known, the ones of what isn't wanted go.
            let keys: HashSet<String> = all_wants.iter().filter_map(|w| w.cover.clone()).collect();
            inner.cover_queue = keys.iter().filter(|k| !self.cover_here(k)).cloned().collect();
            if complete {
                self.keep_only_covers(&keys);
            }
            self.save(&inner.index);
        }
        self.waker.changed();
        self.kick();
    }

    fn cover_here(&self, key: &str) -> bool {
        ["small", "standard"].iter().all(|size| self.covers.join(format!("{key}-{size}")).is_file())
    }

    /// Where a kept cover is (`size`: "small" or "standard"), if it's here.
    pub fn cover_path(&self, key: &str, size: &str) -> Option<PathBuf> {
        Some(self.covers.join(format!("{key}-{size}"))).filter(|p| p.is_file())
    }

    /// Takes away the covers of keys not in `keep`.
    fn keep_only_covers(&self, keep: &HashSet<String>) {
        let Ok(items) = std::fs::read_dir(&self.covers) else { return };
        for item in items.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let key = name.split('-').next().unwrap_or("");
            if !keep.contains(key) {
                let _ = std::fs::remove_file(item.path());
            }
        }
    }

    async fn fetch_cover(&self, key: &str) {
        let _ = tokio::fs::create_dir_all(&self.covers).await;
        for size in ["small", "standard"] {
            let path = self.covers.join(format!("{key}-{size}"));
            if path.is_file() {
                continue;
            }
            if let Ok(bytes) = self.api.image(key, Some(size)).await {
                let part = self.covers.join(format!("{key}-{size}.part"));
                if tokio::fs::write(&part, bytes).await.is_ok() {
                    let _ = tokio::fs::rename(&part, &path).await;
                }
            }
        }
    }

    /// The size of what's kept for a file (by its hash on the server), if it's here.
    pub fn kept_size(&self, source: &str) -> Option<u64> {
        self.inner.lock().unwrap().index.files.get(source).map(|k| k.size)
    }

    /// Where a file is, if it's here: `source` is its hash on the server.
    pub fn local_path(&self, source: &str) -> Option<PathBuf> {
        let blob = self.inner.lock().unwrap().index.files.get(source)?.blob.clone();
        self.store.find(&blob)
    }

    pub fn status(&self) -> Status {
        let inner = self.inner.lock().unwrap();
        Status {
            files: inner.index.files.len(),
            bytes: inner.index.files.values().map(|k| k.size).sum(),
            queued: inner.queue.len() + usize::from(inner.current.is_some()),
            downloading: inner.current.is_some(),
            // Counted from what is, not what was added: stopping and starting again, or a
            // playlist changing, can't make the same file count twice.
            asked: inner.done + inner.queue.len() + usize::from(inner.current.is_some()) + inner.failed.len(),
            done: inner.done,
            failed: inner.failed.len(),
            paused: self.paused.load(Ordering::SeqCst),
            current_bytes: self.current_bytes.load(Ordering::SeqCst),
            received: self.received.load(Ordering::SeqCst),
        }
    }

    /// Why files couldn't be fetched, as (their hash, what happened).
    pub fn failures(&self) -> Vec<(String, String)> {
        self.inner.lock().unwrap().failed.clone()
    }

    /// Forgets every pin and takes every file away.
    pub fn remove_all(self: &Arc<Self>) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.index.pins.clear();
        }
        self.set_wanted(Vec::new(), true);
    }

    fn save(&self, index: &Index) {
        if let Ok(bytes) = serde_json::to_vec(index) {
            let _ = std::fs::write(&self.file, bytes);
        }
    }

    /// Starts fetching if there's something to fetch and nothing is.
    pub fn kick(self: &Arc<Self>) {
        if !self.allowed.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) || self.working.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stop.store(false, Ordering::SeqCst);
        let me = self.clone();
        self.runtime.spawn(async move {
            me.work().await;
            me.working.store(false, Ordering::SeqCst);
            me.waker.changed();
            // Something may have been added as it ended.
            if me.allowed.load(Ordering::SeqCst) && { let inner = me.inner.lock().unwrap(); !inner.queue.is_empty() || !inner.cover_queue.is_empty() } && !me.stop.load(Ordering::SeqCst) {
                me.kick();
            }
        });
    }

    async fn work(&self) {
        loop {
            if self.stop.load(Ordering::SeqCst) || !self.allowed.load(Ordering::SeqCst) || self.paused.load(Ordering::SeqCst) {
                return;
            }
            // Covers first (small, and they make the lists look right offline).
            let cover = self.inner.lock().unwrap().cover_queue.pop_front();
            if let Some(key) = cover {
                self.fetch_cover(&key).await;
                self.waker.changed();
                continue;
            }
            let want = {
                let mut inner = self.inner.lock().unwrap();
                let Some(want) = inner.queue.pop_front() else { return };
                inner.current = Some(want.source.clone());
                want
            };
            self.waker.changed();
            self.current_bytes.store(0, Ordering::SeqCst);
            let kbps = Some(self.kbps.load(Ordering::SeqCst)).filter(|k| *k != 0);
            let (path, name) = match kbps {
                Some(k) => (format!("/api/files/{}/stream?kbps={k}", want.file_id), format!("{}-{k}", want.source)),
                None => (format!("/api/files/{}/stream", want.file_id), want.source.clone()),
            };
            let result = blobs::fetch(
                &self.api,
                &self.store,
                &path,
                &name,
                None,
                None,
                &|n| {
                    self.current_bytes.fetch_add(n, Ordering::SeqCst);
                    self.received.fetch_add(n, Ordering::SeqCst);
                },
                &|| self.stop.load(Ordering::SeqCst),
            )
            .await;
            let mut inner = self.inner.lock().unwrap();
            inner.current = None;
            match result {
                Ok((blob, size)) => {
                    if inner.wanted.contains(&want.source) {
                        let original = blob == want.source;
                        inner.index.files.insert(want.source.clone(), Kept { blob, size, kbps, original });
                        inner.done += 1;
                        self.save(&inner.index);
                    } else {
                        // Taken off while it came.
                        let _ = self.store.remove(&blob);
                    }
                }
                // Asked to stop: it's kept to go on from.
                Err(err) if err.is::<blobs::Stopped>() => {
                    inner.queue.push_front(want);
                    return;
                }
                Err(err) => {
                    // The server isn't there: try again when it may be.
                    let unreachable = err.downcast_ref::<crate::api::ApiError>().is_some_and(|e| e.status.is_none());
                    if unreachable {
                        inner.queue.push_front(want);
                        return;
                    }
                    inner.failed.push((want.source, format!("{err:#}")));
                }
            }
            drop(inner);
            self.waker.changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::tests::{HELLO, serve};

    fn setup(body: &'static [u8]) -> (tempfile::TempDir, Arc<Offline>, tokio::runtime::Runtime) {
        let root = tempfile::tempdir().unwrap();
        let server = serve(body);
        std::fs::write(root.path().join("session.json"), serde_json::json!({"server": server, "token": "t", "username": "u"}).to_string()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let offline = Offline::open(&root.path().join("offline"), Api::new(root.path().to_path_buf()), runtime.handle().clone(), Arc::new(crate::NoWake)).unwrap();
        (root, offline, runtime)
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn what_is_wanted_comes_and_what_is_taken_off_goes() {
        let (_root, offline, _runtime) = setup(b"hello");
        let want = Want { source: HELLO.to_string(), file_id: Uuid::from_u128(1), cover: None };
        offline.pin(Pin { kind: "track".into(), id: "t1".into(), name: "T".into() }, true);
        offline.set_wanted(vec![want.clone()], true);
        until("the file", || offline.local_path(HELLO).is_some());
        assert_eq!(std::fs::read(offline.local_path(HELLO).unwrap()).unwrap(), b"hello");
        let status = offline.status();
        assert_eq!((status.files, status.bytes, status.queued, status.done), (1, 5, 0, 1));
        assert!(offline.is_pinned("track", "t1"));

        // It's still here after a restart.
        let (api, handle) = (offline.api.clone(), offline.runtime.clone());
        let again = Offline::open(offline.file.parent().unwrap(), api, handle, Arc::new(crate::NoWake)).unwrap();
        assert!(again.local_path(HELLO).is_some() && again.is_pinned("track", "t1"));

        // Taken off: gone from the index and the disk.
        let path = offline.local_path(HELLO).unwrap();
        offline.pin(Pin { kind: "track".into(), id: "t1".into(), name: String::new() }, false);
        offline.set_wanted(vec![], true);
        assert!(offline.local_path(HELLO).is_none() && !path.exists());
    }

    #[test]
    fn a_copy_the_server_made_is_kept_under_the_hash_it_was_asked_for_by() {
        let (_root, offline, _runtime) = setup(b"hello");
        // The file as the server keeps it hashes to something else than what it sends.
        let source = "a".repeat(64);
        offline.set_quality(Some(128));
        offline.set_wanted(vec![Want { source: source.clone(), file_id: Uuid::from_u128(2), cover: None }], true);
        until("the copy", || offline.local_path(&source).is_some());
        assert!(offline.local_path(&source).unwrap().ends_with(format!("2c/f2/{HELLO}")));
        assert!(!offline.inner.lock().unwrap().index.files[&source].original);
    }

    #[test]
    fn nothing_comes_while_stopped_and_it_goes_on_when_resumed() {
        let (_root, offline, _runtime) = setup(b"hello");
        offline.pause();
        offline.set_wanted(vec![Want { source: HELLO.to_string(), file_id: Uuid::from_u128(1), cover: None }], true);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(offline.local_path(HELLO).is_none());
        let status = offline.status();
        assert!(status.paused && status.queued == 1);
        offline.resume();
        until("the file once resumed", || offline.local_path(HELLO).is_some());
    }

    #[test]
    fn covers_of_what_is_wanted_are_kept_and_go_with_it() {
        let (_root, offline, _runtime) = setup(b"image");
        let want = Want { source: HELLO.to_string(), file_id: Uuid::from_u128(1), cover: Some("0011223344556677".into()) };
        offline.set_wanted(vec![want], true);
        until("the cover", || offline.cover_path("0011223344556677", "small").is_some() && offline.cover_path("0011223344556677", "standard").is_some());
        until("the file", || offline.kept_size(HELLO).is_some());
        offline.set_wanted(vec![], true);
        assert!(offline.cover_path("0011223344556677", "small").is_none(), "gone with it");
    }

    #[test]
    fn nothing_is_fetched_while_not_allowed() {
        let (_root, offline, _runtime) = setup(b"hello");
        offline.set_allowed(false);
        offline.set_wanted(vec![Want { source: HELLO.to_string(), file_id: Uuid::from_u128(1), cover: None }], true);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(offline.local_path(HELLO).is_none());
        assert_eq!(offline.status().queued, 1);
        offline.set_allowed(true);
        until("the file once allowed", || offline.local_path(HELLO).is_some());
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    #[test]
    fn an_index_written_by_hand_is_read() {
        let json = serde_json::json!({ "files": { "ab": { "blob": "ab", "size": 5, "kbps": null, "original": true } }, "pins": [] });
        let index: Index = serde_json::from_slice(json.to_string().as_bytes()).unwrap();
        assert_eq!(index.files.len(), 1);
    }

    #[test]
    fn nothing_is_taken_away_while_a_pin_cannot_be_worked_out_yet() {
        let root = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let offline = Offline::open(root.path(), Api::new(root.path().to_path_buf()), runtime.handle().clone(), Arc::new(crate::NoWake)).unwrap();
        offline.inner.lock().unwrap().index.files.insert("x".into(), Kept { blob: "x".into(), size: 1, kbps: None, original: true });
        offline.set_wanted(vec![], false);
        assert_eq!(offline.status().files, 1, "kept while the pins aren't known");
        offline.set_wanted(vec![], true);
        assert_eq!(offline.status().files, 0);
    }
}
