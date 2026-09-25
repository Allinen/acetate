//! Files that didn't import (see `db::problems`): listed for review, with their audio.

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use super::media::serve_blob;
use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::problems::Problem;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Show resolved entries instead of open ones.
    #[serde(default)]
    resolved: bool,
    before: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /api/ingest-problems?resolved=false&before=...`: files that didn't import, newest
/// first. Your own uploads; admins see all (also the CLI's).
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<Problem>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = state.db(&user).await?;
    let mut problems = db::problems::list(&mut tx, params.resolved, params.before, limit).await?;
    // An error on our side can mention internals; that's for admins.
    if !user.has(Permission::Admin) {
        for problem in &mut problems {
            if problem.kind == "failed" {
                problem.reason = "an error on the server (an admin can see the details)".to_string();
            }
        }
    }
    Ok(Json(problems))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Resolve {
    /// What was done, e.g. "fixed the tags, imported again" or "not music".
    resolution: String,
}

/// `POST /api/ingest-problems/{id}/resolve`, JSON `{"resolution": "..."}` (optional).
/// Entries also resolve by themselves when the same file imports fine later.
pub async fn resolve(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    body: Option<Json<Resolve>>,
) -> Result<StatusCode, AppError> {
    let resolution = body.map(|Json(b)| b.resolution).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    if !db::problems::resolve(&mut tx, id, &resolution).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/ingest-problems/{id}/audio`: the file as it was received (Range requests
/// work, for seeking).
pub async fn audio(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    let found = db::problems::audio(&mut tx, id).await?;
    drop(tx);
    let Some((sha256, filename)) = found else {
        return Err(AppError::NotFound);
    };
    // The file wasn't probed (or probing it failed), so the type comes from its name.
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    let mime: mime::Mime = match extension.as_deref() {
        Some("flac") => "audio/flac",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "mp4" | "aac" | "alac") => "audio/mp4",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("aif" | "aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
    .parse()
    .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    serve_blob(state.store.path_for(&sha256), &mime, request).await
}
