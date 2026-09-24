//! Adding external source snapshots to tracks, albums and artists, and viewing
//! artists/albums with their sources.

use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::db;
use crate::db::sources::AddedSnapshot;
use crate::model::sources::{self, NewSource, SourceOwner};

/// `POST /api/tracks/{id}/sources`, JSON body, e.g.
/// `{"platform": "soundcloud", "captured_at": "2024-01-01T00:00:00Z",
///   "data": {"song_id": 123, "song_permalink": ["https://soundcloud.com/a/b"]}}`
// TODO(Phase 1): require the `edit_sources` permission (all three handlers).
pub async fn add_track_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, SourceOwner::Track(id), source).await
}

/// `POST /api/albums/{id}/sources`, same body as for tracks.
pub async fn add_album_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, SourceOwner::Album(id), source).await
}

/// `POST /api/artists/{id}/sources`, same body as for tracks.
pub async fn add_artist_source(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, SourceOwner::Artist(id), source).await
}

async fn add(
    state: &AppState,
    owner: SourceOwner,
    source: NewSource,
) -> Result<Json<AddedSnapshot>, AppError> {
    let source = sources::validate(owner.kind(), source)?;
    if !db::sources::owner_exists(&state.pool, owner).await? {
        return Err(AppError::NotFound);
    }
    let mut tx = state.pool.begin().await?;
    let added = db::sources::add_snapshot(&mut tx, owner, &source).await?;
    tx.commit().await?;
    Ok(Json(added))
}

pub async fn get_artist(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<db::catalog::ArtistDetail>, AppError> {
    db::catalog::get_artist(&state.pool, id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

pub async fn get_album(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<db::catalog::AlbumDetail>, AppError> {
    db::catalog::get_album(&state.pool, id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}
