//! The changelist: what was changed lately in the library's metadata, by anyone.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;

#[derive(Debug, Deserialize)]
pub struct Params {
    /// The id of the last change on the previous page.
    before: Option<Uuid>,
    limit: Option<i64>,
    /// Include imports (a file's first metadata), which are most of the revisions.
    #[serde(default)]
    imports: bool,
}

/// `GET /api/changes?before=&limit=&imports=`: the newest revisions of tracks, albums and
/// artists, with the revision each came after. Only what the user can see.
pub async fn list(State(state): State<AppState>, user: CurrentUser, Query(params): Query<Params>) -> Result<Json<db::changes::Changes>, AppError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let mut tx = state.db(&user).await?;
    Ok(Json(db::changes::recent(&mut tx, params.before, limit, params.imports).await?))
}
