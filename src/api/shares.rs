//! Share links.
//!
//! Managing shares (`/api/shares`) needs the `share` permission. Opening one
//! (`/api/share/{token}/...`) needs nothing but the token: these routes are public, and
//! only ever show what the share opens up. The web UI's page for a link is `/s/{token}`.

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::media::{AudioBlob, serve_audio, serve_blob};
use super::{AppError, AppState};
use crate::auth::{self, CurrentUser, Permission};
use crate::db;
use crate::db::shares::{ActiveShare, ShareInfo, SharedTrack};
use crate::model::sources::SourceOwner;

// ---- Managing shares ----

#[derive(Debug, Deserialize)]
pub struct NewShare {
    /// What to share: exactly one of these.
    track_id: Option<Uuid>,
    album_id: Option<Uuid>,
    /// When the link stops working. Never, if left out.
    expires_at: Option<DateTime<Utc>>,
    /// Play the original files, not transcoded copies. Needs `download_original`.
    #[serde(default)]
    original: bool,
    /// The quality of the copies, in kbps: one of the server's (`GET /api/qualities`).
    /// The default if left out.
    kbps: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CreatedShare {
    id: Uuid,
    /// Only returned now: the database just keeps its hash, so it can't be shown again.
    token: String,
    /// The link's path, e.g. `/s/3fa9...`.
    path: String,
}

/// `POST /api/shares`, JSON `{"track_id": "..."}` or `{"album_id": "..."}`, optionally
/// with `"expires_at": "2026-12-31T00:00:00Z"`, and `"original": true` to play the original
/// files instead of transcoded copies (only with `download_original`).
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<NewShare>,
) -> Result<Json<CreatedShare>, AppError> {
    let owner = match (body.track_id, body.album_id) {
        (Some(id), None) => SourceOwner::Track(id),
        (None, Some(id)) => SourceOwner::Album(id),
        _ => {
            return Err(AppError::BadRequest(
                "give exactly one of `track_id` and `album_id`".to_string(),
            ));
        }
    };
    if body.expires_at.is_some_and(|at| at <= Utc::now()) {
        return Err(AppError::BadRequest("`expires_at` is in the past".to_string()));
    }

    if body.original && !user.has(Permission::DownloadOriginal) {
        return Err(AppError::Forbidden(
            "sharing the original file needs the download_original permission: a share link without it plays a transcoded copy".to_string(),
        ));
    }

    if let Some(kbps) = body.kbps {
        state.transcodes.pick(Some(kbps)).map_err(AppError::BadRequest)?;
    }

    let mut tx = state.db(&user).await?;
    // You can only share what you can see (the lookup goes through row-level security).
    if !db::sources::owner_exists(&mut tx, owner).await? {
        return Err(AppError::NotFound);
    }
    let token = auth::new_token();
    let id = db::shares::create(
        &mut tx,
        &auth::token_hash(&token),
        body.track_id,
        body.album_id,
        body.expires_at,
        body.original,
        body.kbps.map(|k| k as i32),
    )
    .await?;
    tx.commit().await?;

    Ok(Json(CreatedShare {
        id,
        path: format!("/s/{token}"),
        token,
    }))
}

/// `GET /api/shares`: your shares (admins: everyone's), newest first.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<Vec<ShareInfo>>, AppError> {
    let mut tx = state.db(&user).await?;
    let only_by = if user.has(Permission::Admin) { None } else { user.id() };
    Ok(Json(db::shares::list(&mut tx, only_by).await?))
}

/// `DELETE /api/shares/{id}`: revokes a share of yours (admins: anyone's). The link stops
/// working immediately.
pub async fn revoke(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.db(&user).await?;
    let only_by = if user.has(Permission::Admin) { None } else { user.id() };
    if !db::shares::revoke(&mut tx, id, only_by).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- Opening a share (public) ----

#[derive(Debug, Serialize)]
pub struct SharedView {
    /// "track" or "album".
    kind: &'static str,
    /// The shared track's or album's id. Logged-in users who can see it are sent to its
    /// normal page instead of this view.
    id: Uuid,
    title: String,
    artist: String,
    cover_id: Option<Uuid>,
    /// For a track share, just that track.
    tracks: Vec<SharedTrack>,
}

/// `GET /api/share/{token}`: what the link shows. 404 if the link doesn't work (unknown,
/// revoked or expired), without saying which.
pub async fn view(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<SharedView>, AppError> {
    let share = find(&state, &token).await?;
    let mut tx = db::begin_share(&state.pool, share.id).await?;
    let tracks = db::shares::tracks(&mut tx, &share).await?;
    let cover_id = tracks.iter().find_map(|t| t.cover_id);

    let view = match (share.track_id, share.album_id) {
        (Some(track_id), _) => {
            let Some(track) = tracks.first() else {
                return Err(AppError::NotFound);
            };
            SharedView {
                kind: "track",
                id: track_id,
                title: track.title.clone(),
                artist: track.artist_display.clone(),
                cover_id,
                tracks,
            }
        }
        (None, Some(album_id)) => {
            let album = db::shares::album(&mut tx, album_id)
                .await?
                .ok_or(AppError::NotFound)?;
            SharedView {
                kind: "album",
                id: album_id,
                title: album.title,
                artist: album.artist,
                // The album's own cover, else the first track's.
                cover_id: album.cover_id.or(cover_id),
                tracks,
            }
        }
        // The schema guarantees one of the two.
        (None, None) => return Err(AppError::NotFound),
    };
    Ok(Json(view))
}

/// `GET /api/share/{token}/tracks/{track_id}/stream`: a shared track's current file: a
/// transcoded copy, or the original if the link gives it (`ActiveShare::original`).
pub async fn stream(
    State(state): State<AppState>,
    Path((token, track_id)): Path<(String, Uuid)>,
    request: Request,
) -> Result<Response, AppError> {
    let share = find(&state, &token).await?;
    let mut tx = db::begin_share(&state.pool, share.id).await?;
    let file = db::shares::file(&mut tx, &share, track_id).await?;
    drop(tx);
    let file = file.ok_or(AppError::NotFound)?;
    let blob = AudioBlob { sha256: file.sha256, container: file.container, codec: file.codec };
    // A quality the server doesn't make anymore (its admin changed the list): the default.
    let kbps = state.transcodes.pick(share.kbps.map(|k| k as u32)).or_else(|_| state.transcodes.pick(None)).map_err(AppError::BadRequest)?;
    serve_audio(&state, &blob, share.original, kbps, request).await
}

/// `GET /api/share/{token}/covers/{cover_id}`: a cover of a shared track or album.
pub async fn cover(
    State(state): State<AppState>,
    Path((token, cover_id)): Path<(String, Uuid)>,
    request: Request,
) -> Result<Response, AppError> {
    let share = find(&state, &token).await?;
    let mut tx = db::begin_share(&state.pool, share.id).await?;
    let cover = db::shares::cover(&mut tx, &share, cover_id).await?;
    drop(tx);
    let cover = cover.ok_or(AppError::NotFound)?;
    let mime = cover
        .mime
        .and_then(|m| m.parse().ok())
        .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    serve_blob(state.store.path_for(&cover.sha256).await, &mime, request).await
}

async fn find(state: &AppState, token: &str) -> Result<ActiveShare, AppError> {
    db::shares::find_active(&state.pool, &auth::token_hash(token))
        .await?
        .ok_or(AppError::NotFound)
}
