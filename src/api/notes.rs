//! Notes on tracks, albums and artists (see `db::notes`). Listed with each one's details;
//! written here (needs `edit_metadata`).

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::notes::{Owner, Revision};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewNote {
    track_id: Option<Uuid>,
    album_id: Option<Uuid>,
    artist_id: Option<Uuid>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    reason: String,
}

/// `POST /api/notes`, JSON `{"track_id" | "album_id" | "artist_id": "…", "title": "…",
/// "body": "…", "reason": "…"}`: a new note. Answers `{id, revision_id}`.
pub async fn create(State(state): State<AppState>, user: CurrentUser, Json(body): Json<NewNote>) -> Result<Json<serde_json::Value>, AppError> {
    let owner = match (body.track_id, body.album_id, body.artist_id) {
        (Some(id), None, None) => Owner::Track(id),
        (None, Some(id), None) => Owner::Album(id),
        (None, None, Some(id)) => Owner::Artist(id),
        _ => return Err(AppError::BadRequest("give one of track_id, album_id or artist_id".into())),
    };
    let mut tx = state.db(&user).await?;
    let (id, revision_id) = db::notes::create(&mut tx, owner, &body.title, &body.body, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id, "revision_id": revision_id })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditNote {
    /// The revision it was edited from (a stale one: 409).
    base_revision_id: Option<Uuid>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/notes/{id}`, JSON `{"base_revision_id": "…", "title": "…", "body": "…",
/// "reason": "…"}`. Answers `{revision_id, changed}`.
pub async fn edit(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<EditNote>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let (revision_id, changed) = db::notes::edit(&mut tx, id, body.base_revision_id, &body.title, &body.body, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "revision_id": revision_id, "changed": changed })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hide {
    /// true: take it off; false: bring it back.
    hidden: bool,
    reason: String,
}

/// `POST /api/notes/{id}/hidden`, JSON `{"hidden": true, "reason": "…"}`: takes a note off
/// (it's kept) or brings it back. Answers `{changed}`.
pub async fn set_hidden(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Hide>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::notes::set_hidden(&mut tx, id, body.hidden, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// `GET /api/notes/{id}/revisions`: its revisions, newest first.
pub async fn revisions(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Vec<Revision>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::notes::revisions(&mut tx, id).await?))
}
