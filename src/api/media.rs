//! Serving stored blobs: audio files (with range requests, for seeking) and cover images.
//!
//! Audio goes out as the original file only to those allowed it (`download_original`), and
//! as a transcoded copy (Opus, see `storage::transcode`, with none of the original's tags)
//! to everyone else (`serve_audio`). Downloads are the
//! original with the track's current tags written in (`download`). Cover images may also be
//! asked for smaller (`?size=small|standard`: copies, see `storage::images`). Stored files
//! are never changed: tags and copies are made on the side.

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

#[derive(Debug, serde::Deserialize)]
pub struct StreamQuery {
    /// `true`: a transcoded copy even if you're allowed the original (for a client that
    /// can't play the file, or wants less to download).
    transcode: Option<bool>,
}

/// `GET /api/files/{id}/stream`: the file as stored if you have `download_original`
/// (unless `?transcode=true`), otherwise a transcoded copy. Supports `Range` requests, so
/// browsers can seek. Previous (replaced) files of a track need `view_history`.
pub async fn stream_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(file_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
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

    let blob = AudioBlob { sha256: file.sha256, container: file.container, codec: file.codec };
    let original_allowed = user.has(Permission::DownloadOriginal) && !query.transcode.unwrap_or(false);
    serve_audio(&state, &blob, original_allowed, request).await
}

#[derive(Debug, serde::Deserialize)]
pub struct DownloadQuery {
    /// `false`: the stored file exactly as it is, without the library's tags written in.
    tags: Option<bool>,
}

/// `GET /api/tracks/{id}/download`: the track's current file as a download, with the
/// track's current title, artists, album, number, date, genres, lyrics and main cover
/// written into its tags (`?tags=false`: the stored bytes untouched). Needs
/// `download_original`. The stored file isn't changed: the tags go into a copy.
pub async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    Query(query): Query<DownloadQuery>,
    request: Request,
) -> Result<Response, AppError> {
    use crate::db::tracks;
    use crate::storage::tagged::{self, TrackTags};

    let mut tx = state.db(&user).await?;
    let track = tracks::get_track(&mut tx, track_id, false).await?.ok_or(AppError::NotFound)?;
    let album = track.albums.first();
    let album_artist = match album {
        Some(a) => tracks::album_artist(&mut tx, a.album_id).await?,
        None => None,
    };
    let album_covers = match album {
        Some(a) if track.covers.is_empty() => tracks::covers_of(&mut tx, a.album_id).await?,
        _ => Vec::new(),
    };
    drop(tx);

    let original = state.store.path_for(&track.file.sha256).await;
    if !tokio::fs::try_exists(&original).await.unwrap_or(false) {
        return Err(AppError::Gone("this file's audio is gone (purged)".to_string()));
    }
    let name = download_name(
        &track.artist_display,
        &track.title,
        extension_for(track.file.container.as_deref(), track.file.codec.as_deref())
            .or_else(|| {
                let own = track.file.original_filename.as_deref().unwrap_or(&track.file.uploaded_filename);
                std::path::Path::new(own).extension().map(|e| e.to_string_lossy().to_string())
            })
            .as_deref(),
    );
    let mime = audio_mime(track.file.container.as_deref(), track.file.codec.as_deref());
    if query.tags == Some(false) {
        return Ok(attach(serve_blob(original, &mime, request).await?, &name));
    }

    // The copy the tags go into: deleted when it's dropped. (On Linux an open file lives
    // on until it's sent, so dropping it after the response is made is fine.)
    let extension = std::path::Path::new(&name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let copy = state.store.temp_file(&extension).map_err(|e| AppError::Internal(e.into()))?;
    let cover = match track.covers.first().or(album_covers.first()) {
        Some(c) => Some((tokio::fs::read(state.store.path_for(&c.blob_sha256).await).await.map_err(|e| AppError::Internal(e.into()))?, c.mime.clone())),
        None => None,
    };
    let tags = TrackTags {
        title: track.title.clone(),
        artist: track.artist_display.clone(),
        album: album.map(|a| a.title.clone()),
        album_artist,
        track_number: album.and_then(|a| a.track_number).map(|n| n as u32),
        disc_number: album.and_then(|a| a.disc_number).map(|n| n as u32),
        date: track.release_date.clone().or(track.release_year.map(|y| y.to_string())),
        genres: track.genres.clone(),
        lyrics: track.lyrics.iter().filter(|l| !l.is_synced).max_by_key(|l| l.is_official).map(|l| l.body.clone()),
        cover,
    };
    let path = copy.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        // Contents only: `fs::copy` would copy the blob's read-only permissions too.
        std::io::copy(&mut std::fs::File::open(&original)?, &mut std::fs::OpenOptions::new().write(true).truncate(true).open(&path)?)?;
        tagged::write(&path, &tags)
    })
    .await
    .map_err(|e| AppError::Internal(e.into()))?
    .map_err(|err| AppError::Conflict(format!("couldn't write tags into this file ({err:#}): download it untouched with ?tags=false")))?;
    let response = serve_blob(copy.path().to_path_buf(), &mime, request).await?;
    drop(copy);
    Ok(attach(response, &name))
}

/// "Artist - Title.ext" (the extension from the file's own name), safe as a file name.
fn download_name(artist: &str, title: &str, ext: Option<&str>) -> String {
    let clean = |s: &str| -> String {
        s.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect()
    };
    let stem = clean(&format!("{artist} - {title}"));
    match ext {
        Some(ext) if ext.chars().all(|c| c.is_ascii_alphanumeric()) => format!("{stem}.{ext}"),
        _ => stem,
    }
}

/// The file extension for what's inside (a file's own name may say otherwise).
fn extension_for(container: Option<&str>, codec: Option<&str>) -> Option<String> {
    let ext = match (container, codec) {
        (Some("flac"), _) => "flac",
        (Some("mp3"), _) => "mp3",
        (Some("ogg"), Some("opus")) => "opus",
        (Some("ogg"), _) => "ogg",
        (Some("isomp4"), _) => "m4a",
        (Some("wav" | "riff"), _) => "wav",
        (Some("aiff"), _) => "aiff",
        (Some("mkv"), _) => "mka",
        (Some("webm"), _) => "webm",
        _ => return None,
    };
    Some(ext.to_string())
}

/// Marks a response as a download under `name` (RFC 6266: UTF-8 file name, percent-encoded).
fn attach(mut response: Response, name: &str) -> Response {
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}")) {
        response.headers_mut().insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    response
}

/// A stored audio file, as far as serving it goes.
pub(super) struct AudioBlob {
    pub sha256: String,
    pub container: Option<String>,
    pub codec: Option<String>,
}

/// Sends audio: the original if `original_allowed`, otherwise a transcoded copy, made now if it's the first
/// time. Without ffmpeg on the server, the copy can't be made: that's said, and the
/// original is *not* sent instead (it would hand it to those who aren't allowed it).
pub(super) async fn serve_audio(
    state: &AppState,
    blob: &AudioBlob,
    original_allowed: bool,
    request: Request,
) -> Result<Response, AppError> {
    use crate::storage::transcode::TranscodeError;

    let original = state.store.path_for(&blob.sha256).await;
    if original_allowed {
        let mime = audio_mime(blob.container.as_deref(), blob.codec.as_deref());
        return serve_blob(original, &mime, request).await;
    }
    match state.transcodes.get(&original, &blob.sha256).await {
        Ok(copy) => serve_blob(copy, &state.transcodes.mime(), request).await,
        Err(TranscodeError::NoFfmpeg) => Err(AppError::Unavailable(
            "this server can't make the lower-quality copy you'd get (ffmpeg isn't installed on it): ask its admin to install it, or for the download_original permission".to_string(),
        )),
        Err(TranscodeError::Failed(err)) => Err(AppError::Internal(err.context(format!("transcoding file {}", blob.sha256)))),
    }
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
