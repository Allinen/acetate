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
}

impl Store {
    pub fn new(data_dir: &Path, session: &Session) -> Store {
        let key = key(session);
        Store {
            body: data_dir.join(format!("catalog-{key}.json")),
            etag: data_dir.join(format!("catalog-{key}.etag")),
            deltas: data_dir.join(format!("catalog-{key}.deltas")),
        }
    }

    /// The copy on disk (the whole catalog, then its deltas), if there is one that reads.
    pub fn load(&self) -> Option<Arc<Catalog>> {
        let body = std::fs::read(&self.body).ok()?;
        // An old format after an update: the server's copy replaces it.
        let mut catalog = Catalog::build(model::parse(&body).ok()?);
        if let Ok(deltas) = std::fs::read(&self.deltas) {
            for line in deltas.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                match model::parse(line) {
                    Ok(delta) => catalog = catalog.apply(delta),
                    // A half-written last line (a crash while saving): the rest is fine.
                    Err(_) => break,
                }
            }
        }
        Some(Arc::new(catalog))
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
                if delta {
                    self.save_delta(&body).context("keeping the library on disk")?;
                } else {
                    self.save(&body, etag.as_deref()).context("keeping the library on disk")?;
                }
                Ok(Some(catalog))
            }
        }
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
}

pub async fn refresh(store: &Store, api: &Api, current: Option<Arc<Catalog>>, report: &impl Fn(Update)) {
    match store.refresh(api, current).await {
        Ok(Some(catalog)) => report(Update::Loaded { catalog, from_server: true }),
        Ok(None) => report(Update::Unchanged),
        Err(err) => report(Update::Failed(format!("{err:#}"))),
    }
}
