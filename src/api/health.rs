//! The library's health check (see `health`): admins start a run in the background and
//! read its progress and last report.

use axum::Json;
use axum::extract::State;
use serde::Deserialize;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::health::{self, Status};

/// `GET /api/health`: the checks there are, the run going now (with its progress and the
/// results so far) and the last finished run's report.
pub async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.health.status())
}

#[derive(Debug, Deserialize)]
pub struct RunRequest {
    /// The checks to run, by id. Left out: every check that doesn't read all the files.
    checks: Option<Vec<String>>,
}

/// `POST /api/health/run`, JSON `{"checks": ["blobs-present"]}`: starts a run in the
/// background (409 if one is going). Poll `GET /api/health`.
pub async fn run(State(state): State<AppState>, user: CurrentUser, Json(body): Json<RunRequest>) -> Result<Json<Status>, AppError> {
    let ids = body.checks.unwrap_or_else(|| health::CHECKS.iter().filter(|c| !c.heavy).map(|c| c.id.to_string()).collect());
    let by = user.login.as_ref().map(|l| l.username.clone()).unwrap_or_default();
    match state.health.start(state.pool.clone(), state.store.clone(), &ids, by) {
        Ok(()) => Ok(Json(state.health.status())),
        Err(message) if message.starts_with("a health check is running") => Err(AppError::Conflict(message)),
        Err(message) => Err(AppError::BadRequest(message)),
    }
}
