//! Keeping the catalog on disk and bringing it up to date: at start the copy on disk is
//! shown right away, then the server is asked what changed since (`?since=` the copy's
//! version: usually a small delta, applied to the copy) and its answer wins.
//!
//! On disk, per server and user, in the app's data folder: `catalog-<key>.json` (the last
//! whole catalog, exactly as the server sent it), `catalog-<key>.etag`, and
//! `catalog-<key>.deltas` (every delta since, one per line, as sent). Loading applies
//! them in order. When the deltas get big, the next refresh asks for the whole catalog
//! again, which starts them over.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use super::{model, Catalog};
use crate::api::{Api, CatalogResponse, Session};

/// Deltas kept on disk beyond this: the next refresh fetches the whole catalog instead.
const DELTAS_LIMIT: u64 = 32 * 1024 * 1024;

/// Deltas on disk beyond this are folded into the whole catalog after a start.
const COMPACT_ABOVE: u64 = 4 * 1024 * 1024;

/// The server and user, readable but safe in a file name (things kept on disk are per
/// server and user).
pub fn key(session: &Session) -> String {
    format!("{}-{}", session.server, session.username)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect()
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
    body: PathBuf,
    etag: PathBuf,
    deltas: PathBuf,
    /// Held while the files are read or written: a load never sees a copy half way
    /// through being compacted, and a delta is never lost to it.
    files: std::sync::Mutex<()>,
}

impl Store {
    pub fn new(data_dir: &Path, session: &Session) -> Store {
        let key = key(session);
        Store {
            body: data_dir.join(format!("catalog-{key}.json")),
            etag: data_dir.join(format!("catalog-{key}.etag")),
            deltas: data_dir.join(format!("catalog-{key}.deltas")),
            files: std::sync::Mutex::new(()),
        }
    }

    /// The copy on disk (the whole catalog, then its deltas), if there is one that reads.
    pub fn load(&self) -> Option<Arc<Catalog>> {
        let _files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let body = std::fs::read(&self.body).ok()?;
        // An old format after an update: the server's copy replaces it.
        let raw = model::parse(&body).ok()?;
        // Every delta merged into the rows first, the index built once: building it takes
        // a moment at 185k tracks, and there can be thousands of deltas.
        let mut merger = super::index::Merger::new(raw);
        if let Ok(deltas) = std::fs::read(&self.deltas) {
            for line in deltas.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                match model::parse(line) {
                    Ok(delta) => merger.add(delta),
                    // A half-written last line (a crash while saving): the rest is fine.
                    Err(_) => break,
                }
            }
        }
        Some(Arc::new(Catalog::build(merger.finish())))
    }

    /// Asks the server what changed since `current` (all of it without one); a new copy
    /// is returned, and kept on disk.
    pub async fn refresh(&self, api: &Api, current: Option<Arc<Catalog>>) -> anyhow::Result<Option<Arc<Catalog>>> {
        let deltas_too_big = std::fs::metadata(&self.deltas).is_ok_and(|m| m.len() > DELTAS_LIMIT);
        let since = current.as_ref().and_then(|c| c.version.clone()).filter(|_| !deltas_too_big);
        let etag = if current.is_some() { std::fs::read_to_string(&self.etag).ok() } else { None };
        match api.catalog(etag.as_deref(), since.as_deref()).await? {
            CatalogResponse::Unchanged => Ok(None),
            CatalogResponse::Changed { body, etag } => {
                // Parsing and indexing is CPU work: off the async threads.
                let (catalog, body, delta) = tokio::task::spawn_blocking(move || {
                    let raw = model::parse(&body)?;
                    let delta = raw.delta;
                    let catalog = match (delta, current) {
                        (true, Some(current)) => current.apply(raw),
                        _ => Catalog::build(raw),
                    };
                    anyhow::Ok((Arc::new(catalog), body, delta))
                })
                .await??;
                let _files = self.files.lock().unwrap_or_else(|e| e.into_inner());
                if delta {
                    self.save_delta(&body).context("keeping the library on disk")?;
                } else {
                    self.save(&body, etag.as_deref()).context("keeping the library on disk")?;
                }
                Ok(Some(catalog))
            }
        }
    }

    /// Folds the deltas into the whole catalog on disk when they've grown (the next start
    /// reads one file, not hundreds of deltas on top of it). The version stays the same, so
    /// nothing in memory changes. Blocking: for a thread of its own.
    fn compact(&self) -> anyhow::Result<()> {
        let _files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        if !std::fs::metadata(&self.deltas).is_ok_and(|m| m.len() > COMPACT_ABOVE) {
            return Ok(());
        }
        let folded = {
            let body = std::fs::read(&self.body)?;
            let deltas = std::fs::read(&self.deltas)?;
            super::compact::fold(&body, &deltas)?
        };
        // The same order as a whole catalog arriving: the body replaced first, then the
        // deltas emptied (a crash between the two leaves deltas that apply again, which
        // does no harm: the rows they hold are replaced by id).
        let temp = self.body.with_extension("json.part");
        std::fs::write(&temp, folded)?;
        std::fs::rename(&temp, &self.body)?;
        std::fs::write(&self.deltas, b"")?;
        Ok(())
    }

    /// A whole catalog: replaces the copy, and its deltas start over.
    fn save(&self, body: &[u8], etag: Option<&str>) -> std::io::Result<()> {
        if let Some(dir) = self.body.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written next to it first, then renamed over: a crash never leaves half a file.
        let temp = self.body.with_extension("json.part");
        std::fs::write(&temp, body)?;
        std::fs::rename(&temp, &self.body)?;
        std::fs::write(&self.deltas, b"")?;
        match etag {
            Some(etag) => std::fs::write(&self.etag, etag),
            None => std::fs::write(&self.etag, b""),
        }
    }

    /// A delta: added to the copy's deltas (one line; a delta body has no newlines).
    fn save_delta(&self, body: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.deltas)?;
        file.write_all(body)?;
        file.write_all(b"\n")
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
    // Once the screen has what it needs: the copy on disk is tidied for the next start
    // (nothing to report if it fails: the deltas stay, and apply as before).
    let compacting = store.clone();
    if let Ok(Ok(())) = tokio::task::spawn_blocking(move || compacting.compact()).await {
        crate::memory::trim();
    }
}

pub async fn refresh(store: &Store, api: &Api, current: Option<Arc<Catalog>>, report: &impl Fn(Update)) {
    match store.refresh(api, current).await {
        Ok(Some(catalog)) => report(Update::Loaded { catalog, from_server: true }),
        Ok(None) => report(Update::Unchanged),
        Err(err) => report(Update::Failed(format!("{err:#}"))),
    }
}

/// Reads a kept copy the way `Store::load` does and says where it stops (ignored by
/// default): `ACETATE_TEST_STORE=<…/catalog-KEY>` (without `.json`). Read-only.
#[cfg(test)]
mod kept_copy {
    use super::*;

    #[test]
    #[ignore]
    fn a_kept_copy_loads() {
        let Ok(prefix) = std::env::var("ACETATE_TEST_STORE") else { return };
        let body = std::fs::read(format!("{prefix}.json")).unwrap();
        let t = std::time::Instant::now();
        let raw = model::parse(&body).unwrap_or_else(|e| panic!("the copy doesn't read: {e:#}"));
        println!("copy: {} tracks, version {:?}, parsed in {:?}", raw.tracks.rows.len(), raw.version, t.elapsed());
        let mut merger = crate::catalog::index::Merger::new(raw);
        let deltas = std::fs::read(format!("{prefix}.deltas")).unwrap_or_default();
        let lines: Vec<&[u8]> = deltas.split(|b| *b == b'\n').filter(|l| !l.is_empty()).collect();
        for (n, line) in lines.iter().enumerate() {
            match model::parse(line) {
                Ok(delta) => merger.add(delta),
                Err(err) => panic!("delta {n} doesn't read: {err:#}"),
            }
        }
        let t = std::time::Instant::now();
        let raw = merger.finish();
        println!("deltas merged in {:?}", t.elapsed());
        let catalog = Catalog::build(raw);
        println!("built in {:?}", t.elapsed());
        println!("with {} deltas: {} tracks, version {:?}, in {:?} in all", lines.len(), catalog.tracks.len(), catalog.version, t.elapsed());
        // Folded into one body (in memory only: nothing on disk is touched), the same
        // catalog has to come out, and load faster.
        let body = std::fs::read(format!("{prefix}.json")).unwrap();
        let deltas = std::fs::read(format!("{prefix}.deltas")).unwrap_or_default();
        let t = std::time::Instant::now();
        let folded = super::super::compact::fold(&body, &deltas).unwrap_or_else(|e| panic!("folding: {e:#}"));
        println!("folded in {:?}: {} MB", t.elapsed(), folded.len() / 1_000_000);
        let t = std::time::Instant::now();
        let again = Catalog::build(model::parse(&folded).unwrap());
        println!("the folded copy loads in {:?}: {} tracks, version {:?}", t.elapsed(), again.tracks.len(), again.version);
        assert_eq!(again.tracks.len(), catalog.tracks.len());
        assert_eq!(again.version, catalog.version);
        let (mut a, mut b): (Vec<_>, Vec<_>) = (again.tracks.iter().map(|t| (t.id, t.title.clone())).collect(), catalog.tracks.iter().map(|t| (t.id, t.title.clone())).collect());
        a.sort();
        b.sort();
        assert!(a == b, "the folded copy's tracks differ");
    }
}
