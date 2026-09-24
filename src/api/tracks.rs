use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::db;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Id of the last track on the previous page.
    before: Option<Uuid>,
    limit: Option<i64>,
}

// TODO(Phase 1): require the requesting user; with RLS (Phase 3) this only returns visible tracks.
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<db::tracks::TrackSummary>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let tracks = db::tracks::list_tracks(&state.pool, params.before, limit).await?;
    Ok(Json(tracks))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<db::tracks::TrackDetail>, AppError> {
    // TODO(Phase 1): only include revisions if the user has the `view_history` permission.
    match db::tracks::get_track(&state.pool, id).await? {
        Some(track) => Ok(Json(track)),
        None => Err(AppError::NotFound),
    }
}

