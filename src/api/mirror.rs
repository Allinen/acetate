//! Mirrors: a copy of the blob store on another machine (the desktop client's), for
//! offline playback and as a second copy. Admins only: it's every file, hidden tracks'
//! too.
//!
//! `GET /api/mirror` lists every stored blob with its volume, size and state (see
//! `db::blobs::MirrorEntry`); `?since=<version>` only those that changed since (a delta).
//! The mirror decides what to fetch (which states and volumes it keeps) and what to do
//! about blobs that went (nothing is removed from it without asking its user). With
//! `&wait=<seconds>`, a delta with nothing in it waits until something changes (or the
//! time runs out) before answering: a mirror kept up to date as files are added.
//!
//! `GET /api/mirror/blobs/{sha256}`: a blob's bytes as stored, with `Range` (so a
//! download that stopped can go on). The mirror checks the hash of what it gets.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::watch;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::blobs::MirrorEntry;

/// The longest a mirror may ask to wait. Less than a reverse proxy usually gives a request
/// (nginx: 60 s), so the answer gets through.
const MAX_WAIT: Duration = Duration::from_secs(55);

/// Tells waiting mirrors that blobs changed: bumped on each `blob_changes` notification
/// (see `listen`).
#[derive(Debug, Default)]
pub struct BlobChanges(watch::Sender<u64>);

impl BlobChanges {
    fn bump(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }
}

/// Listens for `blob_changes` notifications (migration 0059) for as long as the server
/// runs. A lost connection is reconnected, and counts as a change (one may have been
/// missed): mirrors waiting just ask again.
pub async fn listen(pool: PgPool, changes: Arc<BlobChanges>) {
    loop {
        let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!("can't listen for blob changes: {err}");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if let Err(err) = listener.listen("blob_changes").await {
            tracing::warn!("can't listen for blob changes: {err}");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        loop {
            match listener.try_recv().await {
                Ok(Some(_)) => changes.bump(),
                // The connection was lost; the next `try_recv` connects again.
                Ok(None) => changes.bump(),
                Err(err) => {
                    tracing::warn!("listening for blob changes: {err}");
                    changes.bump();
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    since: Option<String>,
    /// Seconds to wait for a change when there's none yet (at most 55).
    wait: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct Volume {
    name: String,
    /// Where new files go unless told otherwise (covers always do).
    default: bool,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    /// Ask `?since=` this next time.
    version: String,
    /// Every blob (`true`), or only those that changed since the version asked about.
    /// Every blob also when the version is older than what the server keeps (90 days):
    /// then anything the mirror has that isn't listed is gone.
    full: bool,
    /// The server's volumes, in order (the first is the default).
    volumes: Vec<Volume>,
    blobs: Vec<MirrorEntry>,
}

/// `GET /api/mirror[?since=<version>[&wait=<seconds>]]`.
pub async fn list(State(state): State<AppState>, user: CurrentUser, Query(params): Query<ListParams>) -> Result<Json<Listing>, AppError> {
    let wait = Duration::from_secs(params.wait.unwrap_or(0)).min(MAX_WAIT);
    let deadline = tokio::time::Instant::now() + wait;
    // Before looking: a change committed while we look still wakes us after.
    let mut changed = state.blob_changes.0.subscribe();
    loop {
        let mut tx = state.db(&user).await?;
        // Before reading anything: whatever changes after this is in the next delta.
        let version = db::library::version(&mut tx).await?;
        let delta = match &params.since {
            Some(since) => db::blobs::mirror_changed_since(&mut tx, since).await?,
            None => None,
        };
        let (full, blobs) = match delta {
            Some(blobs) => (false, blobs),
            None => (true, db::blobs::mirror_all(&mut tx).await?),
        };
        // Done with the database: don't hold a connection while waiting.
        drop(tx);
        let answer = full
            || !blobs.is_empty()
            // Nothing yet: wait for a change, then look again. Out of time: say nothing changed.
            || !matches!(tokio::time::timeout_at(deadline, changed.changed()).await, Ok(Ok(())));
        if answer {
            let volumes = state
                .store
                .volumes()
                .iter()
                .map(|v| Volume { name: v.name.clone(), default: v.name == state.store.default_volume() })
                .collect();
            return Ok(Json(Listing { version, full, volumes, blobs }));
        }
    }
}

/// `GET /api/mirror/blobs/{sha256}`: the blob as stored.
pub async fn blob(State(state): State<AppState>, Path(sha256): Path<String>, request: Request) -> Result<Response, AppError> {
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err(AppError::BadRequest("not a sha256 (64 lowercase hex digits)".into()));
    }
    let Some((_, path)) = state.store.find(&sha256).await else {
        return Err(AppError::NotFound);
    };
    let mut response = super::media::serve_blob(path, &mime::APPLICATION_OCTET_STREAM, request).await?;
    // The same hash is always the same bytes.
    response.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    Ok(response)
}
