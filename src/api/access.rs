//! Restricting tracks (`manage_access`). Who sees what is enforced by row-level security;
//! these routes only change the settings (see `db::access`).

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::access::Access;
use crate::model::sources::SourceOwner;

/// `GET /api/tracks/{id}/access`: whether it's restricted, to which groups, and the
/// log of changes.
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<Access>, AppError> {
    let mut tx = state.db(&user).await?;
    let access = db::access::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    Ok(Json(access))
}

#[derive(Debug, Deserialize)]
pub struct SetAccess {
    restricted: bool,
    /// Group ids that may see it (when restricted). Empty = admins only.
    #[serde(default)]
    groups: Vec<Uuid>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Serialize)]
pub struct AccessResult {
    changed: bool,
    /// Whether you can still see it (restricting it to groups you're not in hides it).
    visible: bool,
}

/// `PUT /api/tracks/{id}/access`, JSON `{"restricted": true, "groups": ["<group id>"], "reason": ""}`
pub async fn set(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetAccess>,
) -> Result<Json<AccessResult>, AppError> {
    let mut tx = state.db(&user).await?;
    // You can only change what you can see.
    if !db::sources::owner_exists(&mut tx, SourceOwner::Track(id)).await? {
        return Err(AppError::NotFound);
    }
    let changed = db::access::set(&mut tx, id, body.restricted, &body.groups, &body.reason).await?;
    let visible = db::sources::owner_exists(&mut tx, SourceOwner::Track(id)).await?;
    tx.commit().await?;
    Ok(Json(AccessResult { changed, visible }))
}

#[derive(Debug, Serialize)]
pub struct AlbumAccessResult {
    /// Tracks whose access changed.
    changed: usize,
    /// The album's tracks you could see (and so were set); tracks you can't see are left
    /// as they are.
    tracks: usize,
}

/// `PUT /api/albums/{id}/access`, same JSON as for a track: sets it on every track of the
/// album that you can see.
pub async fn set_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetAccess>,
) -> Result<Json<AlbumAccessResult>, AppError> {
    let mut tx = state.db(&user).await?;
    if !db::sources::owner_exists(&mut tx, SourceOwner::Album(id)).await? {
        return Err(AppError::NotFound);
    }
    let tracks = db::access::visible_album_tracks(&mut tx, id).await?;
    let mut changed = 0;
    for track_id in &tracks {
        if db::access::set(&mut tx, *track_id, body.restricted, &body.groups, &body.reason).await? {
            changed += 1;
        }
    }
    tx.commit().await?;
    Ok(Json(AlbumAccessResult {
        changed,
        tracks: tracks.len(),
    }))
}
