//! Chunked uploads: one file sent over several requests, for proxies that cap a request's
//! body (Cloudflare: 100 MB). The file is written to a temp file and hashed as chunks
//! arrive, exactly like a file sent in one request, then used by the upload routes in
//! place of a file part (`receive`).
//!
//! * `POST /api/uploads` starts one: `{"id"}`.
//! * `PUT /api/uploads/{id}?offset=N` appends the request's body. `offset` must be what the
//!   upload holds so far (else 409, with that size): a chunk that broke off can be sent
//!   again from where the server got to (`GET /api/uploads/{id}`: `{"size"}`).
//! * Then, in the form sent to `/api/ingest`, `/api/upload`, `/api/tracks/{id}/file` or
//!   `/api/extras/{id}/files`, a file part with `Content-Type: application/vnd.acetate.upload`
//!   and the upload's id as its body stands for the file (its filename is used as usual).
//!   That uses the upload up.
//! * `DELETE /api/uploads/{id}` drops one.
//!
//! Uploads are kept in memory (a restart forgets them, and their temp files go) and only
//! for whoever started them. One left alone for an hour is dropped. The upload limits
//! (`upload_limits`) apply to each chunk (a request) and count every chunk towards the
//! day; the whole file has no size limit of its own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Body;
use axum::extract::multipart::Field;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::storage::{BlobWriter, PendingBlob};

/// The content type of a form part that names a chunked upload instead of carrying bytes.
pub const UPLOAD_PART_TYPE: &str = "application/vnd.acetate.upload";

/// Dropped after this long without a chunk.
const IDLE: Duration = Duration::from_secs(3600);
/// Open at once per account.
const MAX_OPEN: usize = 16;

#[derive(Default)]
pub struct Uploads {
    open: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<Staged>>>>,
}

struct Staged {
    owner: Option<Uuid>,
    /// None once it broke (a write failed): then it's only good for dropping.
    writer: Option<BlobWriter>,
    touched: Instant,
}

impl Uploads {
    /// Drops uploads left alone too long (their temp files go with them). Ones busy with
    /// a chunk right now are left.
    fn sweep(&self) {
        self.open.lock().unwrap().retain(|_, staged| match staged.try_lock() {
            Ok(staged) => staged.touched.elapsed() < IDLE,
            Err(_) => true,
        });
    }

    /// The upload, if it's there and `user`'s.
    async fn get(&self, id: Uuid, user: &CurrentUser) -> Result<Arc<tokio::sync::Mutex<Staged>>, AppError> {
        let staged = self.open.lock().unwrap().get(&id).cloned().ok_or(AppError::NotFound)?;
        if staged.lock().await.owner != user.id() {
            return Err(AppError::NotFound);
        }
        Ok(staged)
    }

    fn remove(&self, id: Uuid) {
        self.open.lock().unwrap().remove(&id);
    }
}

#[derive(Serialize)]
pub struct Started {
    id: Uuid,
}

/// `POST /api/uploads`
pub async fn start(State(state): State<AppState>, user: CurrentUser) -> Result<(StatusCode, Json<Started>), AppError> {
    state.uploads.sweep();
    let mine = {
        let open: Vec<_> = state.uploads.open.lock().unwrap().values().cloned().collect();
        let mut mine = 0;
        for staged in open {
            if staged.lock().await.owner == user.id() {
                mine += 1;
            }
        }
        mine
    };
    if mine >= MAX_OPEN {
        return Err(AppError::Conflict(format!(
            "you have {MAX_OPEN} unfinished uploads: finish or drop one first (they're dropped after an hour on their own)"
        )));
    }
    let writer = state.store.writer().await?;
    let id = Uuid::now_v7();
    let staged = Staged { owner: user.id(), writer: Some(writer), touched: Instant::now() };
    state.uploads.open.lock().unwrap().insert(id, Arc::new(tokio::sync::Mutex::new(staged)));
    Ok((StatusCode::CREATED, Json(Started { id })))
}

#[derive(Serialize)]
pub struct Progress {
    /// Bytes received so far: where the next chunk starts.
    size: u64,
}

/// `GET /api/uploads/{id}`
pub async fn progress(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Progress>, AppError> {
    let staged = state.uploads.get(id, &user).await?;
    let staged = staged.lock().await;
    let writer = staged.writer.as_ref().ok_or_else(broken)?;
    Ok(Json(Progress { size: writer.size() }))
}

#[derive(Deserialize)]
pub struct ChunkParams {
    offset: u64,
}

/// `PUT /api/uploads/{id}?offset=N`, the chunk as the body.
pub async fn append(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(params): Query<ChunkParams>,
    body: Body,
) -> Result<Json<Progress>, AppError> {
    let staged = state.uploads.get(id, &user).await?;
    let mut staged = staged.lock().await;
    staged.touched = Instant::now();
    let writer = staged.writer.as_mut().ok_or_else(broken)?;
    if params.offset != writer.size() {
        return Err(AppError::Conflict(format!(
            "the upload has {} bytes so far: send from there (offset={})",
            writer.size(),
            writer.size()
        )));
    }
    let mut stream = body.into_data_stream();
    // Bytes that arrived are kept even if the request breaks off, so it can go on from
    // there; a failed write breaks the upload (what's on disk no longer matches the hash).
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        if let Err(err) = writer.write(&chunk).await {
            staged.writer = None;
            drop(staged);
            state.uploads.remove(id);
            return Err(AppError::Internal(err));
        }
    }
    Ok(Json(Progress { size: writer.size() }))
}

/// `DELETE /api/uploads/{id}`
pub async fn drop_upload(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    state.uploads.get(id, &user).await?;
    state.uploads.remove(id);
    Ok(StatusCode::NO_CONTENT)
}

fn broken() -> AppError {
    AppError::Conflict("this upload broke off on the server's side: start it again".to_string())
}

/// A file part of an upload form, written to a temp file and hashed: its bytes, or, if
/// it names a chunked upload (`UPLOAD_PART_TYPE`), that upload's file (used up).
pub async fn receive(state: &AppState, user: &CurrentUser, mut field: Field<'_>) -> Result<PendingBlob, AppError> {
    let bad_request = |err: axum::extract::multipart::MultipartError| AppError::BadRequest(err.to_string());
    if field.content_type() == Some(UPLOAD_PART_TYPE) {
        let text = field.text().await.map_err(bad_request)?;
        let id: Uuid = text
            .trim()
            .parse()
            .map_err(|_| AppError::BadRequest(format!("\"{}\" isn't an upload id", text.trim())))?;
        let staged = state.uploads.get(id, user).await?;
        state.uploads.remove(id);
        let writer = staged.lock().await.writer.take().ok_or_else(broken)?;
        return Ok(writer.finish().await?);
    }
    let mut writer = state.store.writer().await?;
    while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
        writer.write(&chunk).await?;
    }
    Ok(writer.finish().await?)
}
