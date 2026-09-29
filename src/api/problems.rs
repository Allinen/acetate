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
use crate::ingest::describe::{NewDescription, Report, ValidDescription};
use crate::ingest::{self, AddAs, Given, IngestOptions, ReplaceOptions};

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
    serve_blob(state.store.path_for(&sha256).await, &mime, request).await
}

/// An open entry, ready to import again: the file in the blob store, and what it was
/// imported with (status, restriction, platform ids...).
async fn load_open(
    state: &AppState,
    user: &CurrentUser,
    id: Uuid,
) -> Result<(ingest::StagedFile, db::problems::OpenProblem, ValidDescription), AppError> {
    let mut tx = state.db(user).await?;
    let problem = db::problems::get_open(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    drop(tx);
    let Some(json) = problem.description.clone() else {
        return Err(AppError::BadRequest("this entry doesn't say where the file came from".to_string()));
    };
    let description: NewDescription = serde_json::from_value(json)
        .map_err(|err| AppError::BadRequest(format!("the stored description doesn't fit: {err}")))?;
    let description = description.validate(|_| None).map_err(AppError::BadRequest)?;
    // Already stored: it stays on the volume it's on.
    let file = ingest::StagedFile::stored(&state.store, &problem.sha256, problem.size_bytes as u64).await;
    Ok((file, problem, description))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImportWith {
    /// Used instead of the tags' title (for a file whose tags have none).
    title: Option<String>,
    /// Used instead of the tags' artists. Each is split on `;` like the ARTIST tag.
    artists: Vec<String>,
    /// The file is an original file (an og file) with this name, when its tags don't say.
    original_filename: Option<String>,
}

/// `POST /api/ingest-problems/{id}/import`, JSON `{"title": "...", "artists": ["..."],
/// "original_filename": "..."}` (the last: it's an original file, when its tags don't say):
/// imports the kept file again, with what it was imported with the first time, using these
/// values for what its tags lack. The file and its raw tags aren't changed; the track's
/// first revision says which values were given by hand. Returns the ingest report; if it's
/// refused again, the entry is updated with the new reason.
pub async fn import(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(with): Json<ImportWith>,
) -> Result<Json<Report>, AppError> {
    let (file, problem, mut description) = load_open(&state, &user, id).await?;
    if let Some(name) = with.original_filename.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        description.hints.original_filename = Some(name.to_string());
    }
    if description.restrict_to.is_some() && !user.has(Permission::ManageAccess) {
        return Err(AppError::Forbidden(
            "this file was to be restricted; importing it needs the manage_access permission".to_string(),
        ));
    }
    let options = IngestOptions {
        user: user.id(),
        uploaded_filename: problem.uploaded_filename,
        origin: description.origin,
        status: description.status.unwrap_or_else(|| "released".to_string()),
        sources: description.track_sources,
        restrict_to: description.restrict_to,
        hints: description.hints,
        local_path: problem.path,
        given: Given {
            title: with.title,
            artists: with.artists,
        },
        volume: None,
        tags_from: None, // a tagged copy was only there for the original request
    };
    let report = ingest::ingest_staged(&state.pool, &state.store, &file, &options, false).await?;
    Ok(Json(report))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddToTrack {
    track_id: Uuid,
    /// `current_file` (the track's file becomes a previous one) or `previous_file`.
    add_as: AddAs,
    #[serde(default)]
    reason: String,
    /// The file is an original file (an og file) with this name, when its tags don't say.
    #[serde(default)]
    original_filename: Option<String>,
}

/// `POST /api/ingest-problems/{id}/add-to-track`, JSON `{"track_id": "...", "add_as":
/// "current_file" | "previous_file", "reason": "..."}`: for a file that belongs to a track
/// that's already there, e.g. a better copy without tags. Needs `replace_file`.
pub async fn add_to_track(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AddToTrack>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (file, problem, description) = load_open(&state, &user, id).await?;
    let options = ReplaceOptions {
        user: user.id(),
        uploaded_filename: problem.uploaded_filename,
        origin: description.origin,
        reason: body.reason,
        volume: None,
        original_filename: body.original_filename.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
    };
    let outcome = ingest::add_file_to_track(&state.pool, &file, body.track_id, body.add_as, &options).await?;
    Ok(Json(serde_json::json!({
        "track_id": body.track_id,
        "file_id": outcome.new_file_id,
        "current_file_id": match body.add_as {
            AddAs::CurrentFile => outcome.new_file_id,
            AddAs::PreviousFile => outcome.old_file_id,
        },
    })))
}
