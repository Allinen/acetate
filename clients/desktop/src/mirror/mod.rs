//! A mirror of the server's blobs on this machine (admins): its audio, covers, extras and
//! files that didn't import, for offline playback and as a second copy. Opt in, per
//! server volume (`index::Config`).
//!
//! It keeps what's *wanted* (`index`): what the mirror keeps of what the server lists,
//! plus anything pinned. A sync brings the server's list up to date (all of it the first
//! time, then what changed), then fetches what's missing: first from folders on this
//! machine that have the files the server imported (by path: a blob imported from
//! `/srv/music/a.flac` is at `<folder>/a.flac` if it's there with the right size; taken in
//! as a reflink), then by downloading the rest. Downloads are hashed as they come in, and
//! only go in if the hash is their name; files from folders too if `check_hashes` is on,
//! else they're trusted by path and size until `rehash` checks every blob here.
//!
//! Nothing is removed because the server let something go (trashed, purged, replaced):
//! those are listed as removable, for the user to remove or keep.
//!
//! One mirror at a time per root (`mirror.lock`): the app's or the headless one's.

pub mod cli;
pub mod in_app;
pub mod index;
pub mod service;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;

use crate::api::Api;
use index::{Count, Index, Removable, RemovableGroup, VolumeTotal};
use store::LocalStore;

/// Downloads at once.
const PARALLEL: usize = 3;
/// How long a watching mirror's request waits for a change (the server allows 55).
const WAIT_SECONDS: u64 = 50;
/// Problems kept to show.
const PROBLEMS_KEPT: usize = 50;

/// What the mirror is doing, for showing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Doing {
    #[default]
    Idle,
    /// Checking the blobs here against what the index says (and hashing ones it doesn't
    /// know yet).
    Checking,
    /// Asking the server what it has.
    Listing,
    /// Looking for wanted blobs in folders on this machine.
    LookingIn,
    Downloading,
    Rehashing,
    Removing,
    /// Up to date, waiting for the server to say something changed.
    Waiting,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub doing: Doing,
    /// Progress through what it's doing: blobs (or files) done of how many, and bytes.
    pub done: u64,
    pub of: u64,
    pub bytes: u64,
    pub of_bytes: u64,
    /// The file or blob at hand.
    pub current: Option<String>,
    /// This run's totals.
    pub downloaded: u64,
    pub taken_in: u64,
    /// The latest problems, newest last.
    pub problems: Vec<String>,
}

/// What a sync would do and where it stands, worked out from the index.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub wanted: Count,
    pub had: Count,
    pub missing: Count,
    /// Missing blobs whose size the server doesn't know (some thumbnails).
    pub missing_unsized: u64,
    pub removable: Count,
    /// Here, taken in by path and size without a hash check (yet).
    pub unchecked: Count,
    /// Free space where the blobs go (None: couldn't tell).
    pub free_bytes: Option<u64>,
}

/// Everything the app shows about the mirror, read at once (in the background).
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub config: index::Config,
    pub server_volumes: Vec<index::ServerVolume>,
    pub volumes: Vec<VolumeTotal>,
    pub plan: Plan,
    pub removable: Vec<RemovableGroup>,
    /// When the server's list was last had (seconds since 1970).
    pub synced_at: Option<i64>,
}

/// What `Mirror::detect` found about a folder here.
#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    /// The server's folder it has the files of (None: none of its files are the server's).
    pub server_folder: Option<String>,
    /// Its files found at the same path below that folder, with the same size.
    pub matched: u64,
    /// Its files in all.
    pub files: u64,
}

/// What a rehash found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rehashed {
    pub checked: u64,
    /// Blobs not what their names say: set aside (`<root>/corrupt/`), fetched again.
    pub set_aside: Vec<String>,
    /// Blobs the index had that weren't here anymore: forgotten, fetched again.
    pub missing: u64,
}

pub struct Mirror {
    api: Api,
    store: LocalStore,
    index: Arc<Mutex<Index>>,
    pub status: Arc<Mutex<Status>>,
    /// Bumped by `stop`: every run started before stops.
    generation: Arc<AtomicU64>,
    stop: Arc<Notify>,
    /// The blobs folder was checked against the index (once per run).
    checked: AtomicBool,
    /// Blobs the server listed but couldn't give (404): not asked for again until its
    /// list says something changed about them.
    unavailable: Mutex<HashSet<String>>,
    _lock: std::fs::File,
}

/// Whether the run it was taken for was stopped: a run takes one when it starts, and
/// `Mirror::stop` stops every run started before (one started after goes on).
#[derive(Clone)]
struct StopToken {
    generation: Arc<AtomicU64>,
    at: u64,
}

impl StopToken {
    fn stopped(&self) -> bool {
        self.generation.load(Relaxed) != self.at
    }

    fn check(&self) -> anyhow::Result<()> {
        if self.stopped() { Err(Stopped.into()) } else { Ok(()) }
    }
}

/// Why something stopped: asked to (not a problem to show).
#[derive(Debug)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Stopped {}

/// The server lists a blob it doesn't have (its file is missing there).
#[derive(Debug)]
struct NotOnServer;

impl std::fmt::Display for NotOnServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server lists it but doesn't have it (not asked for again until that changes)")
    }
}

impl std::error::Error for NotOnServer {}

impl Mirror {
    /// Opens the mirror in `root`. Fails if another is open there (the app's or the
    /// headless one).
    pub fn open(root: &Path, api: Api) -> anyhow::Result<Mirror> {
        std::fs::create_dir_all(root)?;
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(root.join("mirror.lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("another Acetate (the app or its mirror service) is mirroring into {} already", root.display())
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(err).context("locking the mirror"),
        }
        let store = LocalStore::open(root)?;
        store.clear_leftovers();
        let index = Index::open(&root.join("mirror.sqlite"))?;
        Ok(Mirror {
            api,
            store,
            index: Arc::new(Mutex::new(index)),
            status: Arc::default(),
            generation: Arc::default(),
            stop: Arc::default(),
            checked: AtomicBool::new(false),
            unavailable: Mutex::default(),
            _lock: lock,
        })
    }

    /// The server logged in to (the mirror is of its blobs).
    pub fn server(&self) -> anyhow::Result<String> {
        Ok(self.api.session().context("not logged in")?.server)
    }

    pub fn config(&self) -> anyhow::Result<index::Config> {
        self.index.lock().unwrap().config(&self.server()?)
    }

    pub fn set_config(&self, config: &index::Config) -> anyhow::Result<()> {
        self.index.lock().unwrap().set_config(&self.server()?, config)
    }

    pub fn server_volumes(&self) -> anyhow::Result<Vec<index::ServerVolume>> {
        self.index.lock().unwrap().server_volumes(&self.server()?)
    }

    /// Stops what's running as soon as it can (a download in the middle is gone on from
    /// next time). What's started after runs as usual.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Relaxed);
        self.stop.notify_waiters();
        self.doing(Doing::Idle, 0, 0);
    }

    fn token(&self) -> StopToken {
        StopToken { generation: self.generation.clone(), at: self.generation.load(Relaxed) }
    }

    /// Waits for `future`, unless `stop` is (or gets) stopped first.
    async fn unless_stopped<T>(&self, stop: &StopToken, future: impl std::future::Future<Output = T>) -> anyhow::Result<T> {
        let notified = self.stop.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        stop.check()?;
        tokio::select! {
            out = future => Ok(out),
            _ = notified => Err(Stopped.into()),
        }
    }

    fn set_status(&self, change: impl FnOnce(&mut Status)) {
        change(&mut self.status.lock().unwrap());
    }

    fn doing(&self, doing: Doing, of: u64, of_bytes: u64) {
        self.set_status(|s| {
            *s = Status { doing, of, of_bytes, downloaded: s.downloaded, taken_in: s.taken_in, problems: std::mem::take(&mut s.problems), ..Status::default() }
        });
    }

    fn problem(&self, problem: String) {
        self.set_status(|s| {
            s.problems.push(problem);
            let extra = s.problems.len().saturating_sub(PROBLEMS_KEPT);
            s.problems.drain(..extra);
        });
    }

    /// Where it stands (from the index; `refresh` first for the server's latest).
    pub fn plan(&self) -> anyhow::Result<Plan> {
        let server = self.server()?;
        let index = self.index.lock().unwrap();
        let (missing, missing_unsized) = index.missing_totals()?;
        let removable = index.removable_groups(&server)?;
        Ok(Plan {
            wanted: index.wanted()?,
            had: index.had()?,
            missing,
            missing_unsized,
            removable: Count {
                blobs: removable.iter().map(|g| g.count.blobs).sum(),
                bytes: removable.iter().map(|g| g.count.bytes).sum(),
            },
            unchecked: index.unchecked()?,
            free_bytes: fs4::available_space(self.store.blobs_dir()).ok(),
        })
    }

    /// Which of the server's folders a folder here has the files of: the one under which
    /// most of its files are at the same path (below the folder) with the same size, as
    /// the server's last list has them. Blocking: walks the folder (stops early when
    /// `stop` says).
    pub fn detect(&self, folder: &Path, stop: &dyn Fn() -> bool) -> anyhow::Result<Detected> {
        let server = self.server()?;
        let listed = self.index.lock().unwrap().listed_paths(&server)?;
        // By file name: (the server's path, `/` between its parts; size).
        let mut by_name: HashMap<String, Vec<(String, u64)>> = HashMap::new();
        for (path, size) in listed {
            let path = path.replace('\\', "/");
            let name = path.rsplit('/').next().unwrap_or_default().to_string();
            by_name.entry(name).or_default().push((path, size));
        }
        let mut counts: HashMap<String, u64> = HashMap::new();
        let mut files = 0;
        store::walk_files(folder, &mut |path, size| {
            files += 1;
            let Ok(rel) = path.strip_prefix(folder) else { return };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let name = rel.rsplit('/').next().unwrap_or_default();
            for (server_path, server_size) in by_name.get(name).into_iter().flatten() {
                if *server_size == size
                    && let Some(above) = server_path.strip_suffix(rel.as_str())
                    && let Some(server_folder) = above.strip_suffix('/')
                {
                    *counts.entry(server_folder.to_string()).or_default() += 1;
                }
            }
        }, stop);
        let best = counts.into_iter().max_by_key(|(_, n)| *n);
        Ok(Detected { matched: best.as_ref().map_or(0, |(_, n)| *n), server_folder: best.map(|(f, _)| f), files })
    }

    /// Everything the app shows (blocking: a few queries).
    pub fn snapshot(&self) -> anyhow::Result<Snapshot> {
        let server = self.server()?;
        let plan = self.plan()?;
        let index = self.index.lock().unwrap();
        Ok(Snapshot {
            config: index.config(&server)?,
            server_volumes: index.server_volumes(&server)?,
            volumes: index.volume_totals(&server)?,
            plan,
            removable: index.removable_groups(&server)?,
            synced_at: index.synced_at(&server)?,
        })
    }

    /// Removes the removable blobs of one group (see `index::RemovableGroup`). How many.
    pub fn remove_group(&self, state: Option<&str>, volume: Option<&str>) -> anyhow::Result<u64> {
        let sha256s: Vec<String> = self
            .removable()?
            .into_iter()
            .filter(|r| r.state.as_deref() == state && r.volume.as_deref() == volume)
            .map(|r| r.sha256)
            .collect();
        self.remove(&sha256s)
    }

    pub fn removable(&self) -> anyhow::Result<Vec<Removable>> {
        self.index.lock().unwrap().removable(&self.server()?)
    }

    /// Checks the blobs folder against the index, once per run: a blob the index has but
    /// isn't here (or has another size) is forgotten (and fetched again); a file here the
    /// index doesn't know is hashed, and kept if it's what its name says (else set aside).
    /// The first time over a folder of blobs from elsewhere this hashes all of them.
    async fn check_here(&self, stop: &StopToken) -> anyhow::Result<()> {
        if self.checked.load(Relaxed) {
            return Ok(());
        }
        let check = self.config().is_ok_and(|c| c.check_hashes);
        let this = self.blocking_parts(stop);
        let done = tokio::task::spawn_blocking(move || this.check_here(check)).await?;
        self.checked.store(done?, Relaxed);
        Ok(())
    }

    /// Brings the server's list up to date: what changed since last time, or all of it.
    /// `wait`: seconds to wait for a change when there's none yet. Whether anything
    /// changed.
    pub async fn refresh(&self, wait: Option<u64>) -> anyhow::Result<bool> {
        let result = self.refresh_with(&self.token(), wait).await;
        self.doing(Doing::Idle, 0, 0);
        result
    }

    async fn refresh_with(&self, stop: &StopToken, wait: Option<u64>) -> anyhow::Result<bool> {
        let server = self.server()?;
        let since = self.index.lock().unwrap().version(&server)?;
        self.doing(if wait.is_some() { Doing::Waiting } else { Doing::Listing }, 0, 0);
        let listing = self.unless_stopped(stop, self.api.mirror_list(since.as_deref(), wait)).await?.map_err(|err| match err.status {
            Some(403) => anyhow::anyhow!("mirroring needs an admin account"),
            _ => anyhow::Error::new(err),
        })?;
        stop.check()?;
        let changed = listing.full || !listing.blobs.is_empty();
        {
            let mut unavailable = self.unavailable.lock().unwrap();
            if listing.full {
                unavailable.clear();
            }
            for blob in &listing.blobs {
                unavailable.remove(&blob.sha256);
            }
        }
        let index = self.index.clone();
        tokio::task::spawn_blocking(move || {
            index.lock().unwrap().apply_listing(&server, &listing.version, listing.full, &listing.volumes, &listing.blobs)
        })
        .await??;
        Ok(changed)
    }

    /// Fetches what's missing: from the folders to look in, then the server.
    async fn fetch_missing(&self, stop: &StopToken) -> anyhow::Result<()> {
        let config = self.config()?;
        let missing = self.index.lock().unwrap().missing()?;
        if missing.is_empty() {
            return Ok(());
        }
        if !config.look_in.is_empty() {
            let this = self.blocking_parts(stop);
            tokio::task::spawn_blocking(move || this.look_in(&config)).await??;
        }
        let mut missing = self.index.lock().unwrap().missing()?;
        let unavailable = self.unavailable.lock().unwrap().clone();
        missing.retain(|(sha256, _)| !unavailable.contains(sha256));
        self.download(stop, missing).await
    }

    /// One sync: check what's here (once a run), the server's latest list, then fetch
    /// what's missing.
    pub async fn sync(&self) -> anyhow::Result<()> {
        let stop = self.token();
        self.check_here(&stop).await?;
        self.refresh_with(&stop, None).await?;
        self.fetch_missing(&stop).await?;
        self.doing(Doing::Idle, 0, 0);
        Ok(())
    }

    /// Syncs, then keeps up: waits for the server to say something changed, fetches it,
    /// and so on until stopped. A problem (the server unreachable) is noted and tried
    /// again a little later.
    pub async fn watch(&self) {
        let stop = self.token();
        let mut first = true;
        let mut delay = 5;
        loop {
            let result = async {
                self.check_here(&stop).await?;
                let changed = self.refresh_with(&stop, if first { None } else { Some(WAIT_SECONDS) }).await?;
                // Also when nothing changed: something that failed before is tried again.
                let missing = !self.index.lock().unwrap().missing()?.is_empty();
                if changed || missing || first {
                    self.fetch_missing(&stop).await?;
                }
                anyhow::Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    first = false;
                    delay = 5;
                }
                Err(err) if err.is::<Stopped>() => return,
                Err(err) => {
                    self.problem(format!("{err:#}"));
                    self.doing(Doing::Idle, 0, 0);
                    if self.unless_stopped(&stop, tokio::time::sleep(std::time::Duration::from_secs(delay))).await.is_err() {
                        return;
                    }
                    delay = (delay * 2).min(300);
                }
            }
            if stop.stopped() {
                return;
            }
        }
    }

    /// Hashes every blob here again. One not what its name says is set aside and
    /// forgotten (so it's fetched again).
    pub async fn rehash(&self) -> anyhow::Result<Rehashed> {
        let this = self.blocking_parts(&self.token());
        let result = tokio::task::spawn_blocking(move || this.rehash()).await?;
        self.doing(Doing::Idle, 0, 0);
        result
    }

    /// Removes these blobs (the user chose to), except any wanted again meanwhile. How
    /// many were removed.
    pub fn remove(&self, sha256s: &[String]) -> anyhow::Result<u64> {
        self.doing(Doing::Removing, sha256s.len() as u64, 0);
        let mut removed = 0;
        for sha256 in sha256s {
            let index = self.index.lock().unwrap();
            if index.is_wanted(sha256)? || !index.has(sha256)? {
                continue;
            }
            self.store.remove(sha256)?;
            index.forget(sha256)?;
            removed += 1;
            self.set_status(|s| s.done += 1);
        }
        self.doing(Doing::Idle, 0, 0);
        Ok(removed)
    }

    /// Downloads these blobs, a few at a time.
    async fn download(&self, stop: &StopToken, missing: Vec<(String, Option<u64>)>) -> anyhow::Result<()> {
        let total_bytes = missing.iter().filter_map(|(_, s)| *s).sum();
        self.doing(Doing::Downloading, missing.len() as u64, total_bytes);
        let mut queue = missing.into_iter();
        let mut running = tokio::task::JoinSet::new();
        loop {
            while running.len() < PARALLEL
                && let Some((sha256, size)) = queue.next()
            {
                let job = Download {
                    api: self.api.clone(),
                    store: self.store.clone(),
                    index: self.index.clone(),
                    status: self.status.clone(),
                    stop: stop.clone(),
                };
                running.spawn(async move {
                    let result = job.run(&sha256, size).await;
                    (sha256, result)
                });
            }
            let Some(done) = running.join_next().await else { break };
            let (sha256, result) = done?;
            match result {
                Ok(()) => self.set_status(|s| {
                    s.done += 1;
                    s.downloaded += 1;
                }),
                Err(err) if err.is::<Stopped>() => {
                    running.abort_all();
                    return Err(err);
                }
                Err(err) if err.is::<NotOnServer>() => {
                    self.unavailable.lock().unwrap().insert(sha256.clone());
                    self.set_status(|s| s.done += 1);
                    self.problem(format!("{sha256}: {err}"));
                }
                Err(err) => {
                    self.set_status(|s| s.done += 1);
                    self.problem(format!("{sha256}: {err:#}"));
                }
            }
        }
        stop.check()
    }

    /// What the blocking work needs, to move to another thread.
    fn blocking_parts(&self, stop: &StopToken) -> Blocking {
        Blocking { store: self.store.clone(), index: self.index.clone(), status: self.status.clone(), stop: stop.clone() }
    }
}

/// One download: into `tmp/<sha256>.part` (from where an earlier one stopped), hashed as
/// it comes, moved in if the hash is its name.
struct Download {
    api: Api,
    store: LocalStore,
    index: Arc<Mutex<Index>>,
    status: Arc<Mutex<Status>>,
    stop: StopToken,
}

impl Download {
    async fn run(&self, sha256: &str, size: Option<u64>) -> anyhow::Result<()> {
        let part = self.store.part_path(sha256);
        self.status.lock().unwrap().current = Some(sha256.to_string());
        // What an earlier download got: hashed first, then gone on from.
        let earlier = part.clone();
        let (mut hasher, mut got) = tokio::task::spawn_blocking(move || -> std::io::Result<(Sha256, u64)> {
            let mut hasher = Sha256::new();
            let got = match std::fs::File::open(&earlier) {
                Ok(mut file) => store::hash_into(&mut hasher, &mut file, &|| false)?.unwrap_or(0),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
                Err(err) => return Err(err),
            };
            Ok((hasher, got))
        })
        .await??;
        if size.is_some_and(|size| got > size) {
            (hasher, got) = (Sha256::new(), 0);
            let _ = tokio::fs::remove_file(&part).await;
        }
        if Some(got) != size {
            match self.api.stream_from(&format!("/api/mirror/blobs/{sha256}"), got).await {
                Ok(mut response) => {
                    let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&part).await?;
                    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT && got > 0 {
                        // The whole file, not the rest: start over.
                        file.set_len(0).await?;
                        (hasher, got) = (Sha256::new(), 0);
                    }
                    while let Some(chunk) = response.chunk().await.context("downloading")? {
                        if self.stop.stopped() {
                            file.flush().await?;
                            return Err(Stopped.into());
                        }
                        hasher.update(&chunk);
                        file.write_all(&chunk).await?;
                        got += chunk.len() as u64;
                        self.status.lock().unwrap().bytes += chunk.len() as u64;
                    }
                    file.flush().await?;
                }
                // Asked from its end: it was all here.
                Err(err) if err.status == Some(416) && got > 0 => {}
                Err(err) if err.status == Some(404) => return Err(NotOnServer.into()),
                Err(err) => return Err(err.into()),
            }
        }
        let hashed = hex::encode(hasher.finalize());
        if hashed != sha256 {
            let _ = tokio::fs::remove_file(&part).await;
            anyhow::bail!("what came hashes to {hashed}, not its name: thrown away (it's fetched again next time)");
        }
        let store = self.store.clone();
        let sha = sha256.to_string();
        tokio::task::spawn_blocking(move || store.commit(&part, &sha)).await??;
        self.index.lock().unwrap().add(sha256, got, "downloaded", true)
    }
}

/// The mirror's blocking work (hashing, walking folders), on a thread of its own.
struct Blocking {
    store: LocalStore,
    index: Arc<Mutex<Index>>,
    status: Arc<Mutex<Status>>,
    stop: StopToken,
}

impl Blocking {
    fn doing(&self, doing: Doing, of: u64, of_bytes: u64) {
        let mut s = self.status.lock().unwrap();
        *s = Status { doing, of, of_bytes, downloaded: s.downloaded, taken_in: s.taken_in, problems: std::mem::take(&mut s.problems), ..Status::default() };
    }

    fn progress(&self, current: &Path, bytes: u64) {
        if self.stop.stopped() {
            return;
        }
        let mut s = self.status.lock().unwrap();
        s.done += 1;
        s.bytes += bytes;
        s.current = Some(current.display().to_string());
    }

    fn problem(&self, problem: String) {
        let mut s = self.status.lock().unwrap();
        s.problems.push(problem);
        let extra = s.problems.len().saturating_sub(PROBLEMS_KEPT);
        s.problems.drain(..extra);
    }

    /// See `Mirror::check_here`. Whether it got through (not stopped).
    fn check_here(&self, check: bool) -> anyhow::Result<bool> {
        let had: HashMap<String, u64> = self.index.lock().unwrap().all_had()?.into_iter().collect();
        let files = self.store.walk();
        let unknown: Vec<_> = files.iter().filter(|(name, _, _)| !had.contains_key(name)).collect();
        self.doing(Doing::Checking, unknown.len() as u64, unknown.iter().map(|(_, _, s)| s).sum());
        // In the index but not here (removed by hand), or another size: fetched again.
        let here: HashMap<&str, u64> = files.iter().map(|(name, _, size)| (name.as_str(), *size)).collect();
        for (sha256, size) in &had {
            match here.get(sha256.as_str()) {
                Some(found) if found == size => {}
                Some(_) => {
                    if let Some(path) = self.store.path_of(sha256) {
                        let aside = self.store.set_aside(&path)?;
                        self.problem(format!("{sha256} wasn't the size it was: set aside as {}", aside.display()));
                    }
                    self.index.lock().unwrap().forget(sha256)?;
                }
                None => self.index.lock().unwrap().forget(sha256)?,
            }
        }
        // Here but not in the index (a copy of a server's volume, say): kept if it's in its
        // place and the size a server lists it with (hashed first if `check`); one no server
        // lists, or not where its name says, is hashed.
        let stop = || self.stop.stopped();
        for (name, path, size) in unknown {
            if self.stop.stopped() {
                return Ok(false);
            }
            let in_place = self.store.path_of(name).is_some_and(|p| &p == path);
            let listed = self.index.lock().unwrap().listed_size(name)?;
            if in_place && !check && listed == Some(*size) {
                self.index.lock().unwrap().add(name, *size, "found", false)?;
            } else {
                let Some((sha256, _)) = store::hash_file_until(path, &stop)? else { return Ok(false) };
                if in_place && &sha256 == name {
                    self.index.lock().unwrap().add(name, *size, "found", true)?;
                } else {
                    let aside = self.store.set_aside(path)?;
                    self.problem(format!("{} isn't a blob of this mirror (it hashes to {sha256}): set aside as {}", path.display(), aside.display()));
                }
            }
            self.progress(path, *size);
        }
        Ok(true)
    }

    /// Takes in missing blobs from the folders to look in: each at the path its server
    /// path maps to (`index::LookIn`), if it's there with the right size. Hashed first if
    /// `check_hashes`, else trusted (and marked unchecked).
    fn look_in(&self, config: &index::Config) -> anyhow::Result<()> {
        let missing = self.index.lock().unwrap().missing_with_paths()?;
        let mut found = Vec::new();
        for (sha256, size, server_path) in &missing {
            if let Some(local) = config.look_in.iter().find_map(|l| l.local_path(server_path))
                && std::fs::metadata(&local).is_ok_and(|m| m.is_file() && m.len() == *size)
            {
                found.push((sha256, *size, local));
            }
        }
        for folder in &config.look_in {
            if !folder.folder.is_dir() {
                self.problem(format!("{} isn't a folder (or isn't there): not looked in", folder.folder.display()));
            }
        }
        if found.is_empty() {
            return Ok(());
        }
        self.doing(Doing::LookingIn, found.len() as u64, found.iter().map(|(_, s, _)| s).sum());
        let stop = || self.stop.stopped();
        for (sha256, size, path) in found {
            self.stop.check()?;
            match self.store.take_in(&path, sha256, config.check_hashes, &stop) {
                Ok(how) => {
                    let how = match how {
                        store::TakenIn::Reflinked => "reflinked",
                        store::TakenIn::Copied => "copied",
                    };
                    self.index.lock().unwrap().add(sha256, size, how, config.check_hashes)?;
                    self.status.lock().unwrap().taken_in += 1;
                }
                Err(err) if err.is::<Stopped>() => return Err(err),
                Err(err) => self.problem(format!("{}: {err:#}", path.display())),
            }
            self.progress(&path, size);
        }
        Ok(())
    }

    fn rehash(&self) -> anyhow::Result<Rehashed> {
        let had = self.index.lock().unwrap().all_had()?;
        self.doing(Doing::Rehashing, had.len() as u64, had.iter().map(|(_, s)| s).sum());
        let mut report = Rehashed::default();
        let stop = || self.stop.stopped();
        for (sha256, size) in had {
            self.stop.check()?;
            let Some(path) = self.store.path_of(&sha256) else { continue };
            match store::hash_file_until(&path, &stop) {
                Ok(None) => return Err(Stopped.into()),
                Ok(Some((hashed, _))) if hashed == sha256 => self.index.lock().unwrap().verified(&sha256)?,
                Ok(_) => {
                    let aside = self.store.set_aside(&path)?;
                    self.problem(format!("{sha256} isn't what its name says: set aside as {}", aside.display()));
                    self.index.lock().unwrap().forget(&sha256)?;
                    report.set_aside.push(sha256.clone());
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    self.index.lock().unwrap().forget(&sha256)?;
                    report.missing += 1;
                }
                Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
            }
            report.checked += 1;
            self.progress(&path, size);
        }
        Ok(report)
    }
}
