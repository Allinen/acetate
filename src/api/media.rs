//! Serving stored blobs: audio files (with range requests, for seeking) and cover images.
//!
//! Files are served exactly as stored: no transcoding, no tag injection. Cover images may
//! also be asked for smaller (`?size=small|standard`: copies, see `storage::images`); the
//! original is never changed.

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};

// Files and covers are found through the tracks they belong to, never on their own. So
// when row-level security hides a track, its files and covers are hidden too,
// and asking for them is a plain 404, the same as for an id that doesn't exist.

/// `GET /api/files/{id}/stream`: the file as stored. Supports `Range` requests, so
/// browsers can seek. Previous (replaced) files of a track need `view_history`.
pub async fn stream_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(file_id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    let file = sqlx::query!(
        r#"
        SELECT f.sha256, f.container, f.codec, f.purged_at,
               EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id) AS "is_current!",
               EXISTS (
                   SELECT 1 FROM track_file_history h
                   JOIN tracks t ON t.id = h.track_id
                   WHERE h.old_file_id = f.id
               ) AS "is_previous!"
        FROM files f
        WHERE f.id = $1
        "#,
        file_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    // Done with the database; don't hold a connection while the file is sent.
    drop(tx);

    let Some(file) = file else {
        return Err(AppError::NotFound);
    };
    let visible = file.is_current || (file.is_previous && user.has(Permission::ViewHistory));
    if !visible {
        return Err(AppError::NotFound);
    }

    if let Some(purged_at) = file.purged_at {
        return Err(AppError::Gone(format!(
            "this file was purged on {}: its record is kept, but the audio is gone",
            purged_at.format("%Y-%m-%d")
        )));
    }

    let mime = audio_mime(file.container.as_deref(), file.codec.as_deref());
    serve_blob(state.store.path_for(&file.sha256).await, &mime, request).await
}

/// `GET /api/covers/{id}`: a cover image of a track or album.
pub async fn cover(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(cover_id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    // An album cover is visible if any of the album's tracks is.
    let cover = sqlx::query!(
        r#"
        SELECT c.blob_sha256, c.mime
        FROM covers c
        WHERE c.id = $1
          AND (EXISTS (SELECT 1 FROM tracks t WHERE t.id = c.track_id)
               OR EXISTS (
                   SELECT 1 FROM album_tracks at
                   JOIN tracks t ON t.id = at.track_id
                   WHERE at.album_id = c.album_id
               ))
        "#,
        cover_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    drop(tx);

    let Some(cover) = cover else {
        return Err(AppError::NotFound);
    };
    let mime = cover
        .mime
        .and_then(|m| m.parse().ok())
        .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    serve_blob(state.store.path_for(&cover.blob_sha256).await, &mime, request).await
}

#[derive(Debug, serde::Deserialize)]
pub struct ImageQuery {
    /// `small` (200 px on the longest side) or `standard` (640 px). Without it: the
    /// original.
    size: Option<String>,
}

/// `GET /api/images/{hash}`: a cover image by its content: its SHA-256, or the start of
/// it (16+ hex digits, as the catalog's `cover_key`). Visible if a cover the user can see
/// uses it. The same image under many covers is one URL, and it never changes: clients
/// may keep it forever. `?size=small|standard`: a smaller copy (a JPEG), or the original
/// if it's that small already.
pub async fn image(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(hash): Path<String>,
    Query(query): Query<ImageQuery>,
    request: Request,
) -> Result<Response, AppError> {
    use crate::storage::images::{Scaled, Size};

    let size = match query.size.as_deref() {
        None => None,
        Some(text) => Some(Size::parse(text).ok_or_else(|| AppError::BadRequest("size is `small` or `standard` (or left out, for the original)".into()))?),
    };
    let mut tx = state.db(&user).await?;
    let image = crate::db::covers::image_by_hash(&mut tx, &hash).await?;
    drop(tx);
    let Some(image) = image else {
        return Err(AppError::NotFound);
    };
    let original = state.store.path_for(&image.blob_sha256).await;
    let scaled = match size {
        Some(size) => state.images.sized(&original, &image.blob_sha256, size).await?,
        None => Scaled::Original,
    };
    let mut response = match scaled {
        Scaled::Resized(path) => serve_blob(path, &mime::IMAGE_JPEG, request).await?,
        Scaled::Original => {
            let mime = image.mime.and_then(|m| m.parse().ok()).unwrap_or(mime::APPLICATION_OCTET_STREAM);
            serve_blob(original, &mime, request).await?
        }
    };
    // `private`: one user's view (shared caches mustn't keep it); `immutable`: the same
    // hash is always the same bytes.
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(response)
}

/// Serves a file from disk using tower-http's `ServeFile`, which handles `Range`,
/// `If-Modified-Since` etc. for us.
pub(super) async fn serve_blob(
    path: std::path::PathBuf,
    mime: &mime::Mime,
    request: Request,
) -> Result<Response, AppError> {
    // `oneshot` runs a tower service (here ServeFile) for a single request.
    let response = ServeFile::new_with_mime(path, mime).oneshot(request).await?;
    Ok(response.map(Body::new).into_response())
}

/// The content type browsers need to play the file, from what symphonia detected.
pub(super) fn audio_mime(container: Option<&str>, codec: Option<&str>) -> mime::Mime {
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
