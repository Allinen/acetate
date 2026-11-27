//! Lookups for tools outside Acetate, before they upload or propose something: what has
//! these bytes, what has this platform id, which tracks have this title. See `db::lookup`.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::lookup::{FileHit, PlatformId, SourceHit, TrackHit};

/// The most hashes or platform ids looked up in one request.
const MAX_ITEMS: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Files {
    sha256: Vec<String>,
}

/// `POST /api/lookup/files {sha256: [...]}`: per hash, in the order given, the tracks,
/// extras, covers and open ingest problems that have those bytes.
pub async fn files(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Files>) -> Result<Json<Vec<FileHit>>, AppError> {
    if body.sha256.len() > MAX_ITEMS {
        return Err(AppError::BadRequest(format!("at most {MAX_ITEMS} hashes at a time")));
    }
    let shas: Vec<String> = body.sha256.iter().map(|s| s.trim().to_lowercase()).collect();
    if let Some(bad) = shas.iter().find(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(AppError::BadRequest(format!("{bad:?} isn't a sha256 (64 hex digits)")));
    }
    let mut tx = state.db(&user).await?;
    Ok(Json(db::lookup::files(&mut tx, &shas).await?))
}

#[derive(Debug, Deserialize)]
pub struct Sources {
    items: Vec<PlatformId>,
}

/// `POST /api/lookup/sources {items: [{platform, external_id}]}`: per id, in the order
/// given, the tracks, albums, artists and groupings with a source of that id.
pub async fn sources(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Sources>) -> Result<Json<Vec<SourceHit>>, AppError> {
    if body.items.len() > MAX_ITEMS {
        return Err(AppError::BadRequest(format!("at most {MAX_ITEMS} ids at a time")));
    }
    let mut tx = state.db(&user).await?;
    Ok(Json(db::lookup::sources(&mut tx, &body.items).await?))
}

#[derive(Debug, Deserialize)]
pub struct TrackSearch {
    q: String,
    artist: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/tracks/search?q=&artist=&limit=`: tracks by title, and by artist as
/// displayed if given; best match first (`score`).
pub async fn search_tracks(State(state): State<AppState>, user: CurrentUser, Query(params): Query<TrackSearch>) -> Result<Json<Vec<TrackHit>>, AppError> {
    let q = params.q.trim();
    if q.is_empty() {
        return Err(AppError::BadRequest("`q` can't be empty".into()));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let mut tx = state.db(&user).await?;
    Ok(Json(db::lookup::search_tracks(&mut tx, q, params.artist.as_deref(), limit).await?))
}
