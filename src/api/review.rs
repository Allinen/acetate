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

/// How many flags or tracks one bulk request may name (the client sends more in turns).
const BULK_LIMIT: usize = 10_000;

fn check_size(ids: &[Uuid]) -> Result<(), AppError> {
    if ids.len() > BULK_LIMIT {
        return Err(AppError::BadRequest(format!("at most {BULK_LIMIT} at once")));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct ResolveMany {
    flag_ids: Vec<Uuid>,
    #[serde(default)]
    resolution: String,
}

#[derive(Debug, serde::Serialize)]
pub struct Resolved {
    resolved: u64,
}

/// `POST /api/review/resolve`, JSON `{"flag_ids": [...], "resolution": "..."}`: resolves
/// many flags at once (the open ones among them the user can see). Answers how many were.
pub async fn resolve_many(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<ResolveMany>,
) -> Result<Json<Resolved>, AppError> {
    check_size(&body.flag_ids)?;
    let mut tx = state.db(&user).await?;
    let resolved = db::review::resolve_many(&mut tx, &body.flag_ids, &body.resolution).await?;
    tx.commit().await?;
    Ok(Json(Resolved { resolved }))
}

#[derive(Debug, Deserialize)]
pub struct Ambiguous {
    flag_ids: Vec<Uuid>,
    artist_id: Uuid,
    #[serde(default)]
    reason: String,
}

/// `POST /api/review/ambiguous`, JSON `{"flag_ids": [...], "artist_id": ..., "reason": ...}`:
/// ambiguous artist names settled on one artist, credited on each track (a revision each).
/// See `db::review_bulk::settle_ambiguous`.
pub async fn settle_ambiguous(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<Ambiguous>,
) -> Result<Json<db::review_bulk::Settled>, AppError> {
    check_size(&body.flag_ids)?;
    let mut tx = state.db(&user).await?;
    let settled = db::review_bulk::settle_ambiguous(&mut tx, &body.flag_ids, body.artist_id, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(settled))
}

#[derive(Debug, Deserialize)]
pub struct Singles {
    track_ids: Vec<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/tracks/singles`, JSON `{"track_ids": [...], "reason": ...}`: each track on no
/// album gets a single of its own. See `db::review_bulk::make_singles`.
pub async fn make_singles(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<Singles>,
) -> Result<Json<db::review_bulk::Singles>, AppError> {
    check_size(&body.track_ids)?;
    let mut tx = state.db(&user).await?;
    let made = db::review_bulk::make_singles(&mut tx, &body.track_ids, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(made))
}

#[derive(Debug, Deserialize)]
pub struct MergeDuplicates {
    flag_ids: Vec<Uuid>,
    max_length_difference_ms: i64,
    #[serde(default)]
    ignore_case: bool,
    #[serde(default)]
    reason: String,
    /// Only say what would be merged (the same work, rolled back: an exact preview).
    #[serde(default)]
    dry_run: bool,
}

/// `POST /api/review/merge-duplicates`, JSON `{"flag_ids": [...], "max_length_difference_ms":
/// 1000, "ignore_case": false, "reason": ..., "dry_run": true}`: the possible duplicates
/// those flags name that have the same title and about the same length are merged. See
/// `db::review_bulk::merge_duplicates`.
pub async fn merge_duplicates(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<MergeDuplicates>,
) -> Result<Json<db::review_bulk::MergedDuplicates>, AppError> {
    check_size(&body.flag_ids)?;
    if body.max_length_difference_ms < 0 {
        return Err(AppError::BadRequest("the length difference can't be negative".into()));
    }
    let same = db::review_bulk::SameTrack { max_length_difference_ms: body.max_length_difference_ms, ignore_case: body.ignore_case };
    let reason = if body.reason.trim().is_empty() { "same title and length (bulk merge)" } else { body.reason.trim() };
    let mut tx = state.db(&user).await?;
    let done = db::review_bulk::merge_duplicates(&mut tx, Some(&body.flag_ids), same, reason).await?;
    if body.dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(done))
}
