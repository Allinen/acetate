//! Playlists (see `db::playlists`). Anyone logged in (`browse`) has their own; public
//! ones can be seen by everyone logged in, changed only by their owner.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::playlists::{LogEntry, PlaylistDetail, PlaylistSummary};

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// `?trash=true`: your own playlists in the trash instead.
    #[serde(default)]
    trash: bool,
}

/// `GET /api/playlists[?trash=true]`: yours and public ones (yours first).
pub async fn list(State(state): State<AppState>, user: CurrentUser, Query(params): Query<ListParams>) -> Result<Json<Vec<PlaylistSummary>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::playlists::list(&mut tx, params.trash).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPlaylist {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    public: bool,
    /// Tracks to start with, in order.
    #[serde(default)]
    tracks: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct Created {
    id: Uuid,
}

/// `POST /api/playlists {name, description?, public?, tracks?}`.
pub async fn create(State(state): State<AppState>, user: CurrentUser, Json(body): Json<NewPlaylist>) -> Result<Json<Created>, AppError> {
    let mut tx = state.db(&user).await?;
    let id = db::playlists::create(&mut tx, &body.name, &body.description, body.public).await?;
    if !body.tracks.is_empty() {
        db::playlists::add(&mut tx, id, &body.tracks, None).await?;
    }
    tx.commit().await?;
    Ok(Json(Created { id }))
}

/// `GET /api/playlists/{id}`: the playlist and the entries you can see, in order.
pub async fn get(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<PlaylistDetail>, AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::get(&mut tx, id).await?.map(Json).ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
    name: Option<String>,
    description: Option<String>,
    public: Option<bool>,
}

/// `PATCH /api/playlists/{id} {name?, description?, public?}` (the owner).
pub async fn update(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Changes>) -> Result<Json<PlaylistDetail>, AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::update(&mut tx, id, body.name.as_deref(), body.description.as_deref(), body.public).await?;
    let detail = db::playlists::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(detail))
}

/// `DELETE /api/playlists/{id}`: into the trash (the owner). `POST .../restore`: back.
pub async fn trash(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<(), AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::set_trashed(&mut tx, id, true).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn restore(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<(), AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::set_trashed(&mut tx, id, false).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddEntries {
    tracks: Vec<Uuid>,
    /// Before the `at`-th entry you see (0 = at the start); left out: at the end.
    at: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Added {
    entries: Vec<Uuid>,
}

/// `POST /api/playlists/{id}/entries {tracks: [...], at?}` (the owner). The same track
/// may be added more than once.
pub async fn add(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<AddEntries>) -> Result<Json<Added>, AppError> {
    let mut tx = state.db(&user).await?;
    let entries = db::playlists::add(&mut tx, id, &body.tracks, body.at).await?;
    tx.commit().await?;
    Ok(Json(Added { entries }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryIds {
    entries: Vec<Uuid>,
}

/// `DELETE /api/playlists/{id}/entries {entries: [...]}` (the owner): logged, undoable.
pub async fn remove(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<EntryIds>) -> Result<(), AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::remove(&mut tx, id, &body.entries).await?;
    tx.commit().await?;
    Ok(())
}

/// `PUT /api/playlists/{id}/order {entries: [...]}` (the owner): every entry you see, in
/// the new order. Logged, undoable.
pub async fn reorder(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<EntryIds>) -> Result<(), AppError> {
    let mut tx = state.db(&user).await?;
    db::playlists::reorder(&mut tx, id, &body.entries).await?;
    tx.commit().await?;
    Ok(())
}

/// `GET /api/playlists/{id}/log` (the owner): changes, newest first.
pub async fn log(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Vec<LogEntry>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::playlists::history(&mut tx, id).await?))
}

#[derive(Debug, Serialize)]
pub struct Undone {
    summary: String,
}

/// `POST /api/playlists/{id}/log/{change}/undo` (the owner).
pub async fn undo(State(state): State<AppState>, user: CurrentUser, Path((id, change)): Path<(Uuid, Uuid)>) -> Result<Json<Undone>, AppError> {
    let mut tx = state.db(&user).await?;
    let summary = db::playlists::undo(&mut tx, id, change).await?;
    tx.commit().await?;
    Ok(Json(Undone { summary }))
}
