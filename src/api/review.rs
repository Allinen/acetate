//! Review flags: what ingest noticed (possible duplicates, ambiguous artist names).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::review::Flag;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Show resolved flags instead of open ones.
    #[serde(default)]
    resolved: bool,
    before: Option<Uuid>,
    limit: Option<i64>,
    /// Only flags of this kind (e.g. `fix_later`).
    kind: Option<String>,
    /// All but flags of this kind (e.g. `fix_later`, so they don't bury the rest).
    exclude: Option<String>,
    /// Only the kinds the server raises itself (`true`: "no cover"…), or none of them
    /// (`false`). See `GET /api/vocab`'s `review_kinds`.
    automatic: Option<bool>,
    /// Only this track's flags.
    track: Option<Uuid>,
}

/// `GET /api/review?resolved=false&before=...&kind=...&exclude=...&automatic=...&track=...`:
/// flags, newest first.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<Flag>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = state.db(&user).await?;
    let filter = db::review::Filter {
        resolved: params.resolved,
        kind: params.kind.as_deref(),
        exclude: params.exclude.as_deref(),
        automatic: params.automatic,
        track: params.track,
    };
    let flags = db::review::list(&mut tx, &filter, params.before, limit).await?;
    Ok(Json(flags))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Resolve {
    /// What was done, e.g. "not a duplicate" or "fixed credits".
    resolution: String,
}

/// `POST /api/review/{id}/resolve`, JSON `{"resolution": "..."}` (optional).
pub async fn resolve(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    body: Option<Json<Resolve>>,
) -> Result<StatusCode, AppError> {
    let resolution = body.map(|Json(b)| b.resolution).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    if !db::review::resolve(&mut tx, id, &resolution).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
