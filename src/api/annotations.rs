//! A user's own plays, stars and ratings (see `db::annotations`).

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::annotations::{Annotation, Item};

async fn star(state: AppState, user: CurrentUser, item: Item, starred: bool) -> Result<Json<Annotation>, AppError> {
    let mut tx = state.db(&user).await?;
    let annotation = db::annotations::set_starred(&mut tx, item, starred).await?;
    tx.commit().await?;
    Ok(Json(annotation))
}

/// `POST /api/tracks/{id}/star` (and albums, artists): star it. `DELETE`: unstar.
pub async fn star_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Track(id), true).await
}
pub async fn unstar_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Track(id), false).await
}
pub async fn star_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Album(id), true).await
}
pub async fn unstar_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Album(id), false).await
}
pub async fn star_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Artist(id), true).await
}
pub async fn unstar_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    star(s, u, Item::Artist(id), false).await
}

#[derive(Debug, Deserialize)]
pub struct Rating {
    /// 0 (not rated) to 10: half stars.
    rating: i16,
}

async fn rate(state: AppState, user: CurrentUser, item: Item, rating: i16) -> Result<Json<Annotation>, AppError> {
    let mut tx = state.db(&user).await?;
    let annotation = db::annotations::set_rating(&mut tx, item, rating).await?;
    tx.commit().await?;
    Ok(Json(annotation))
}

/// `PUT /api/tracks/{id}/rating` (and albums, artists), JSON `{"rating": 0..10}`.
pub async fn rate_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Annotation>, AppError> {
    rate(s, u, Item::Track(id), r.rating).await
}
pub async fn rate_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Annotation>, AppError> {
    rate(s, u, Item::Album(id), r.rating).await
}
pub async fn rate_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Annotation>, AppError> {
    rate(s, u, Item::Artist(id), r.rating).await
}

/// `POST /api/tracks/{id}/played`: the user listened to it (clients call this after half
/// the track or 4 minutes, whichever comes first). Counts for its albums and primary
/// artists too.
pub async fn played(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Annotation>, AppError> {
    let mut tx = state.db(&user).await?;
    let annotation = db::annotations::played(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(annotation))
}
