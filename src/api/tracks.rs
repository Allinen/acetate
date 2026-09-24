use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Id of the last track on the previous page.
    before: Option<Uuid>,
    limit: Option<i64>,
    /// Substring search in title and artist (case-insensitive).
    q: Option<String>,
}

// With RLS (Phase 3) this only returns tracks the user can see.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<db::tracks::TrackSummary>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let query = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut tx = state.db(&user).await?;
    let tracks = match query {
        Some(q) => db::tracks::search_tracks(&mut tx, q, params.before, limit).await?,
        None => db::tracks::list_tracks(&mut tx, params.before, limit).await?,
    };
    Ok(Json(tracks))
}

/// Revisions and previous files are only included with the `view_history` permission.
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<db::tracks::TrackDetail>, AppError> {
    let with_history = user.has(Permission::ViewHistory);
    let mut tx = state.db(&user).await?;
    match db::tracks::get_track(&mut tx, id, with_history).await? {
        Some(track) => Ok(Json(track)),
        None => Err(AppError::NotFound),
    }
}

