//! Keeping the catalog on disk and bringing it up to date: at start the copy on disk is
//! shown right away, then the server is asked what changed since (`?since=` the copy's
//! version: usually a small delta, applied to the copy) and its answer wins.
//!
//! The copy on disk is a snapshot (see `snapshot`) opened by mapping it, so a start costs
//! no parsing. On disk, per server and user, in the app's data folder:
//! `catalog-<key>.<generation>.snap`. Each time the catalog changes a new snapshot is
//! written (the next generation) and the older ones are removed; the catalog in memory is
//! then the new one, mapped, so the heap holds none of it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use super::{Catalog, model};
use crate::api::{Api, CatalogResponse, Session};

/// The server and user, readable but safe in a file name (things kept on disk are per
/// server and user). The server is its identity, so a new address doesn't change it; a
/// session from before servers had one is keyed by the address.
pub fn key(session: &Session) -> String {
    match session.instance {
        Some(instance) => format!("{instance}-{}", session.username),
        None => format!("{}-{}", session.server, session.username),
    }
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
}

/// What names the server alone (the mirror's index is of a server, whoever logs in): its
/// identity, or its address in a session from before servers had one.
pub fn key_of_server(session: &Session) -> String {
    match session.instance {
        Some(instance) => instance.to_string(),
        None => session.server.clone(),
    }
}

/// Moves what's kept in `dir` for the key `from` (the catalog, downloads, queue…: a name
/// like `catalog-<key>.<generation>.snap`, `offline-<key>`, `list-<key>.json`) to the key
/// `to`. What is already there under `to` is left alone (and so is what's under `from`).
pub fn move_kept(dir: &Path, from: &str, to: &str) {
    if from == to {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let Some((word, rest)) = name.split_once('-') else { continue };
        let Some(after) = rest.strip_prefix(from) else { continue };
        if !word.chars().all(|c| c.is_ascii_lowercase()) || !(after.is_empty() || after.starts_with('.')) {
            continue;
        }
        let target = dir.join(format!("{word}-{to}{after}"));
        if !target.exists() {
            let _ = std::fs::rename(entry.path(), target);
        }
    }
}

/// What a load or refresh found.
pub enum Update {
    /// A catalog to show: `from_server` false for the copy on disk.
    Loaded { catalog: Arc<Catalog>, from_server: bool },
    /// The server's copy is the one we have.
    Unchanged,
    Failed(String),
}

pub struct Store {
    dir: PathBuf,
    /// The start of this server and user's file names.
    prefix: String,
    /// Held while the files are read or written.
    files: std::sync::Mutex<()>,
}

impl Store {
    pub fn new(data_dir: &Path, session: &Session) -> Store {
        Store { dir: data_dir.to_path_buf(), prefix: format!("catalog-{}", key(session)), files: std::sync::Mutex::new(()) }
    }

    /// The snapshots on disk, newest generation first.
    fn snapshots(&self) -> Vec<(u64, PathBuf)> {
        let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let generation = name.strip_prefix(&self.prefix)?.strip_prefix('.')?.strip_suffix(".snap")?;
                Some((u64::from_str_radix(generation, 16).ok()?, entry.path()))
            })
            .collect();
        found.sort_by_key(|(generation, _)| std::cmp::Reverse(*generation));
        found
    }

    /// Removes the older snapshots (what a platform won't remove while it is mapped stays
    /// until the next start), and the copies older versions of the app kept.
    fn tidy(&self, keep: u64) {
        for (generation, path) in self.snapshots() {
            if generation != keep {
                let _ = std::fs::remove_file(path);
            }
        }
        for old in ["json", "etag", "deltas", "json.part"] {
            let _ = std::fs::remove_file(self.dir.join(format!("{}.{old}", self.prefix)));
        }
    }

    /// The copy on disk, if there is one that opens.
    pub fn load(&self) -> Option<Arc<Catalog>> {
        let _files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        for (generation, path) in self.snapshots() {
            match Catalog::open_snapshot(&path) {
                Ok(catalog) => {
                    self.tidy(generation);
                    return Some(Arc::new(catalog));
                }
                // Another format after an update, or damaged: the server's copy replaces it.
                Err(err) => eprintln!("the kept library doesn't open ({err:#}): fetching it again"),
            }
        }
        self.tidy(0);
        None
    }

    /// Keeps `catalog` as the next generation and returns it as mapped from there (the
    /// catalog itself if that fails: it works the same, from memory).
    fn keep(&self, catalog: Catalog) -> Arc<Catalog> {
        let _files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let generation = self.snapshots().first().map_or(1, |(g, _)| g + 1);
        let path = self.dir.join(format!("{}.{generation:016x}.snap", self.prefix));
        let temp = path.with_extension("snap.part");
        let kept = std::fs::create_dir_all(&self.dir)
            .map_err(anyhow::Error::from)
            .and_then(|_| catalog.write_snapshot(&temp))
            .and_then(|_| std::fs::rename(&temp, &path).map_err(Into::into))
            .and_then(|_| Catalog::open_snapshot(&path));
        match kept {
            Ok(mapped) => {
                self.tidy(generation);
                Arc::new(mapped)
            }
            Err(err) => {
                eprintln!("couldn't keep the library on disk: {err:#}");
                let _ = std::fs::remove_file(&temp);
                Arc::new(catalog)
            }
        }
    }

    /// Asks the server what changed since `current` (all of it without one); a new copy
    /// is returned, and kept on disk.
    pub async fn refresh(self: &Arc<Self>, api: &Api, current: Option<Arc<Catalog>>) -> anyhow::Result<Option<Arc<Catalog>>> {
        let since = current.as_ref().and_then(|c| c.version.clone());
        let etag = current.as_ref().and_then(|c| c.etag.clone());
        match api.catalog(etag.as_deref(), since.as_deref()).await? {
            CatalogResponse::Unchanged => Ok(None),
            CatalogResponse::Changed { body, etag } => {
                // Parsing, indexing and writing are work for threads of their own, not the
                // async ones.
                let store = self.clone();
                let catalog = tokio::task::spawn_blocking(move || {
                    let delta = model::is_delta(&body)?;
                    let mut catalog = match (delta, &current) {
                        (true, Some(current)) => current.apply(model::parse(&body)?),
                        _ => super::index::Merger::from_base(model::parse_base(&body)?)?.build(None)?,
                    };
                    // The ETag is the whole catalog's: a delta keeps the one we had.
                    catalog.etag = if delta { current.and_then(|c| c.etag.clone()) } else { etag };
                    anyhow::Ok(store.keep(catalog))
                })
                .await?
                .context("keeping the library on disk")?;
                Ok(Some(catalog))
            }
        }
    }
}

/// Loads the disk copy (if any), then refreshes from the server, reporting each step.
pub async fn load_and_refresh(store: Arc<Store>, api: Api, report: impl Fn(Update)) {
    let disk = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || store.load()).await.ok().flatten()
    };
    if let Some(catalog) = disk.clone() {
        report(Update::Loaded { catalog, from_server: false });
    }
    refresh(&store, &api, disk, &report).await;
    crate::memory::trim();
}

pub async fn refresh(store: &Arc<Store>, api: &Api, current: Option<Arc<Catalog>>, report: &impl Fn(Update)) {
    match store.refresh(api, current).await {
        Ok(Some(catalog)) => report(Update::Loaded { catalog, from_server: true }),
        Ok(None) => report(Update::Unchanged),
        Err(err) => report(Update::Failed(format!("{err:#}"))),
    }
}

/// Builds a catalog from a server's whole answer saved as a file, keeps it as a snapshot
/// and opens that, saying how long each step takes (ignored by default):
/// `ACETATE_TEST_CATALOG=<catalog.json> cargo test --release a_whole_catalog_is_kept -- --ignored --nocapture`.
/// Nothing but a temporary folder is written.
#[cfg(test)]
mod kept_copy {
    use super::*;

    #[test]
    #[ignore]
    fn a_whole_catalog_is_kept() {
        let Ok(path) = std::env::var("ACETATE_TEST_CATALOG") else { return };
        let body = std::fs::read(path).unwrap();
        let t = std::time::Instant::now();
        let built = super::super::index::Merger::from_base(model::parse_base(&body).unwrap()).unwrap().build(None).unwrap();
        println!("built from the JSON in {:?}: {} tracks", t.elapsed(), built.track_count());
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path(), &Session { server: "http://x".into(), token: String::new(), username: "u".into(), instance: None });
        let t = std::time::Instant::now();
        let kept = store.keep(built);
        println!("kept in {:?}: {} MB on disk, mapped: {}", t.elapsed(), std::fs::metadata(&store.snapshots()[0].1).unwrap().len() / 1_000_000, kept.is_mapped());
        drop(kept);
        for n in 1..=3 {
            let t = std::time::Instant::now();
            let loaded = store.load().expect("the snapshot opens");
            println!("load {n}: opened in {:?}: {} tracks, version {:?}", t.elapsed(), loaded.track_count(), loaded.version);
            let t = std::time::Instant::now();
            let list = loaded.album_list("", super::super::index::AlbumSort::Title);
            let tracks = loaded.track_list("artist:a", super::super::index::TrackSort::Title, false);
            println!("   first album list and a track filter in {:?}: {} albums, {} tracks", t.elapsed(), list.len(), tracks.len());
        }
    }

    #[test]
    fn the_key_follows_the_server_not_its_address() {
        let id = uuid::Uuid::new_v4();
        let at = |server: &str, instance| Session { server: server.into(), token: String::new(), username: "ann".into(), instance };
        assert_eq!(key(&at("https://a.example", Some(id))), key(&at("https://b.example", Some(id))));
        assert_ne!(key(&at("https://a.example", Some(id))), key(&at("https://a.example", Some(uuid::Uuid::new_v4()))));
        assert_eq!(key(&at("https://a.example", None)), "https___a_example-ann");
    }

    #[test]
    fn what_is_kept_moves_to_the_new_key_once() {
        let dir = tempfile::tempdir().unwrap();
        let touch = |name: &str| std::fs::write(dir.path().join(name), name).unwrap();
        for name in ["catalog-old-ann.0000000000000001.snap", "list-old-ann.json", "offline-old-ann", "catalog-old-anna.1.snap", "catalog-other.1.snap"] {
            touch(name);
        }
        move_kept(dir.path(), "old-ann", "new-ann");
        let mut names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["catalog-new-ann.0000000000000001.snap", "catalog-old-anna.1.snap", "catalog-other.1.snap", "list-new-ann.json", "offline-new-ann"]);
        // Again: nothing left to move, nothing damaged.
        move_kept(dir.path(), "old-ann", "new-ann");
        assert!(dir.path().join("list-new-ann.json").is_file());
    }
}
