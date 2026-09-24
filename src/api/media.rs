//! Serving stored blobs: audio files (with range requests, for seeking) and cover images.
//!
//! Files are served exactly as stored: no transcoding, no tag injection.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use super::{AppError, AppState};

/// `GET /api/files/{id}/stream`: the file as stored. Supports `Range` requests, so
/// browsers can seek. Works for replaced (older) files too.
// TODO(Phase 1): require the `stream` permission.
pub async fn stream_file(
    State(state): State<AppState>,
    Path(file_id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let Some(file) = sqlx::query!(
        "SELECT sha256, container, codec FROM files WHERE id = $1",
        file_id
    )
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(AppError::NotFound);
    };

    let mime = audio_mime(file.container.as_deref(), file.codec.as_deref());
    serve_blob(state.store.path_for(&file.sha256), &mime, request).await
}

/// `GET /api/covers/{id}`: a cover image.
pub async fn cover(
    State(state): State<AppState>,
    Path(cover_id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let Some(cover) = sqlx::query!(
        "SELECT blob_sha256, mime FROM covers WHERE id = $1",
        cover_id
    )
    .fetch_optional(&state.pool)
    .await?
    else {
        return Err(AppError::NotFound);
    };

    let mime = cover
        .mime
        .and_then(|m| m.parse().ok())
        .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    serve_blob(state.store.path_for(&cover.blob_sha256), &mime, request).await
}

/// Serves a file from disk using tower-http's `ServeFile`, which handles `Range`,
/// `If-Modified-Since` etc. for us.
async fn serve_blob(
    path: std::path::PathBuf,
    mime: &mime::Mime,
    request: Request,
) -> Result<Response, AppError> {
    // `oneshot` runs a tower service (here ServeFile) for a single request.
    let response = ServeFile::new_with_mime(path, mime).oneshot(request).await?;
    Ok(response.map(Body::new).into_response())
}

/// The content type browsers need to play the file, from what symphonia detected.
fn audio_mime(container: Option<&str>, codec: Option<&str>) -> mime::Mime {
    let name = match (container, codec) {
        (Some("flac"), _) => "audio/flac",
        (Some("mp3"), _) | (_, Some("mp3")) => "audio/mpeg",
        (Some("ogg"), _) => "audio/ogg",
        (Some("isomp4"), _) => "audio/mp4",
        (Some("wav" | "riff"), _) => "audio/wav",
        (Some("aiff"), _) => "audio/aiff",
        (Some("mkv" | "webm"), _) => "audio/webm",
        (_, Some("flac")) => "audio/flac",
        (_, Some("aac")) => "audio/aac",
        _ => "application/octet-stream",
    };
    name.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM)
}
