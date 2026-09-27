//! Keeping the catalog on disk and bringing it up to date: at start the copy on disk is
//! shown right away, then the server is asked (with the copy's ETag) and its answer wins.
//!
//! One copy per server and user, in the app's data folder: `catalog-<key>.json` (the
//! body exactly as the server sent it) and `catalog-<key>.etag`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;

use super::{model, Catalog};
use crate::api::{Api, CatalogResponse, Session};

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
}

impl Store {
    pub fn new(data_dir: &Path, session: &Session) -> Store {
        let key = key(session);
        Store {
            body: data_dir.join(format!("catalog-{key}.json")),
            etag: data_dir.join(format!("catalog-{key}.etag")),
        }
    }

    /// The copy on disk, if there is one that reads.
    pub fn load(&self) -> Option<Arc<Catalog>> {
        let body = std::fs::read(&self.body).ok()?;
        match model::parse(&body) {
            Ok(raw) => Some(Arc::new(Catalog::build(raw))),
            // An old format after an update: the server's copy replaces it.
            Err(_) => None,
        }
    }

    /// Asks the server; a new copy is parsed, kept on disk and returned.
    pub async fn refresh(&self, api: &Api, have_copy: bool) -> anyhow::Result<Option<Arc<Catalog>>> {
        let etag = if have_copy { std::fs::read_to_string(&self.etag).ok() } else { None };
        match api.catalog(etag.as_deref()).await? {
            CatalogResponse::Unchanged => Ok(None),
            CatalogResponse::Changed { body, etag } => {
                // Parsing and indexing is CPU work: off the async threads.
                let (catalog, body) = tokio::task::spawn_blocking(move || {
                    let raw = model::parse(&body)?;
                    anyhow::Ok((Arc::new(Catalog::build(raw)), body))
                })
                .await??;
                self.save(&body, etag.as_deref()).context("keeping the library on disk")?;
                Ok(Some(catalog))
            }
        }
    }

    fn save(&self, body: &[u8], etag: Option<&str>) -> std::io::Result<()> {
        if let Some(dir) = self.body.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written next to it first, then renamed over: a crash never leaves half a file.
        let temp = self.body.with_extension("json.part");
        std::fs::write(&temp, body)?;
        std::fs::rename(&temp, &self.body)?;
        match etag {
            Some(etag) => std::fs::write(&self.etag, etag),
            None => match std::fs::remove_file(&self.etag) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            },
        }
    }
}

/// Loads the disk copy (if any), then refreshes from the server, reporting each step.
pub async fn load_and_refresh(store: Arc<Store>, api: Api, report: impl Fn(Update)) {
    let disk = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || store.load()).await.ok().flatten()
    };
    let have_copy = disk.is_some();
    if let Some(catalog) = disk {
        report(Update::Loaded { catalog, from_server: false });
    }
    refresh(&store, &api, have_copy, &report).await;
}

pub async fn refresh(store: &Store, api: &Api, have_copy: bool, report: &impl Fn(Update)) {
    match store.refresh(api, have_copy).await {
        Ok(Some(catalog)) => report(Update::Loaded { catalog, from_server: true }),
        Ok(None) => report(Update::Unchanged),
        Err(err) => report(Update::Failed(format!("{err:#}"))),
    }
}
