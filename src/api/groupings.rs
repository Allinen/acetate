//! Groupings (see `db::groupings`): a collective, an era, a project… with its tracks and
//! albums. Reading needs `browse`; changing, `edit_metadata` (sources: `edit_sources`).

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;

/// `GET /api/groupings/{id}`: what it is, its tracks and albums (the ones the user can
/// see), who's credited on them, its sources and every change.
pub async fn get(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<db::groupings::Detail>, AppError> {
    let mut tx = state.db(&user).await?;
    db::groupings::detail(&mut tx, id).await?.map(Json).ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    kind: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    reason: String,
}

/// `POST /api/groupings`, JSON `{"kind": "collective", "name": "…", "description": ""}`.
pub async fn create(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Fields>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let id = db::groupings::create(&mut tx, &body.kind, &body.name, &body.description, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

/// `PUT /api/groupings/{id}`, the same JSON: its new kind, name and description.
pub async fn edit(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Fields>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    db::groupings::edit(&mut tx, id, &body.kind, &body.name, &body.description, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Why {
    reason: String,
}

/// `DELETE /api/groupings/{id}`, JSON `{"reason": ""}` (optional): deletes it if it's empty
/// (no tracks, releases, members, sources or extras), else 400 saying what's in it.
pub async fn delete(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, body: Option<Json<Why>>) -> Result<Json<serde_json::Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    db::groupings::delete(&mut tx, id, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Members {
    #[serde(default)]
    track_ids: Vec<Uuid>,
    #[serde(default)]
    album_ids: Vec<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/groupings/{id}/add`, JSON `{"track_ids": […], "album_ids": […]}`. A track
/// alone on a single brings the single, and a single its only track. Returns how many
/// were added, `{"tracks", "albums"}` (ones already in it aren't counted).
pub async fn add(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Members>) -> Result<Json<db::groupings::Changed>, AppError> {
    let mut tx = state.db(&user).await?;
    let added = db::groupings::add(&mut tx, id, &body.track_ids, &body.album_ids, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(added))
}

/// `POST /api/groupings/{id}/remove`, the same JSON: takes them out (a single and its
/// only track together).
pub async fn remove(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Members>) -> Result<Json<db::groupings::Changed>, AppError> {
    let mut tx = state.db(&user).await?;
    let removed = db::groupings::remove(&mut tx, id, &body.track_ids, &body.album_ids, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(removed))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artists {
    artist_ids: Vec<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/groupings/{id}/members/add`, JSON `{"artist_ids": […]}`: who's in it.
pub async fn add_members(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Artists>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let added = db::groupings::add_members(&mut tx, id, &body.artist_ids, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "added": added })))
}

/// `POST /api/groupings/{id}/members/remove`, the same JSON.
pub async fn remove_members(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Artists>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let removed = db::groupings::remove_members(&mut tx, id, &body.artist_ids, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}

#[derive(Debug, Default, Deserialize)]
pub struct IntoQuery {
    /// An existing collective (else a new one).
    into: Option<Uuid>,
}

/// `GET /api/artists/{id}/collective[?into=<grouping>]`: what turning the artist into a
/// collective (or merging it into that one) would do; changes nothing. See
/// `db::groupings::collective_preview`.
pub async fn collective_preview(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, axum::extract::Query(q): axum::extract::Query<IntoQuery>) -> Result<Json<db::groupings::CollectivePreview>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::groupings::collective_preview(&mut tx, id, q.into).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Reason {
    reason: String,
    /// An existing collective to merge into (else a new one).
    into: Option<Uuid>,
}

/// `POST /api/artists/{id}/collective`, JSON `{"reason": "", "into": null}` (optional): does
/// it. Returns the collective's id.
pub async fn turn_into_collective(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, body: Option<Json<Reason>>) -> Result<Json<serde_json::Value>, AppError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let grouping = db::groupings::turn_into_collective(&mut tx, id, body.into, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "grouping_id": grouping })))
}
