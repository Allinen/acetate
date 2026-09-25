//! Managing covers of tracks and albums (`edit_metadata`): upload, copy, reorder, trash.
//! Showing one is `media::cover`.

use axum::Json;
use axum::extract::{Multipart, Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::model::sources::SourceOwner;

/// Largest cover image accepted.
pub const MAX_COVER_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct AddedCover {
    id: Uuid,
}

/// `POST /api/tracks/{id}/covers`, multipart with one image file (JPEG, PNG, WebP or GIF).
pub async fn upload_to_track(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Json<AddedCover>, AppError> {
    upload(state, user, CoverOwner::Track(id), multipart).await
}

/// `POST /api/albums/{id}/covers`, like for a track.
pub async fn upload_to_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    multipart: Multipart,
) -> Result<Json<AddedCover>, AppError> {
    upload(state, user, CoverOwner::Album(id), multipart).await
}

async fn upload(
    state: AppState,
    user: CurrentUser,
    owner: CoverOwner,
    mut multipart: Multipart,
) -> Result<Json<AddedCover>, AppError> {
    let bad_request = |err: axum::extract::multipart::MultipartError| AppError::BadRequest(err.to_string());
    let Some(field) = multipart.next_field().await.map_err(bad_request)? else {
        return Err(AppError::BadRequest("send the image as a file field".to_string()));
    };
    let bytes = field.bytes().await.map_err(bad_request)?;

    // What it is, from the bytes themselves (not the filename or the content type the
    // client claims).
    let Some(image) = crate::model::image_info(&bytes) else {
        return Err(AppError::BadRequest(
            "not a JPEG, PNG, WebP or GIF image".to_string(),
        ));
    };

    // Check the owner before storing anything.
    let mut tx = state.db(&user).await?;
    let exists = match owner {
        CoverOwner::Track(id) => db::sources::owner_exists(&mut tx, SourceOwner::Track(id)).await?,
        CoverOwner::Album(id) => db::sources::owner_exists(&mut tx, SourceOwner::Album(id)).await?,
    };
    if !exists {
        return Err(AppError::NotFound);
    }

    let sha256 = state.store.put_bytes(&bytes).await?;
    db::blobs::register(&mut tx, &sha256, state.store.default_volume(), Some(bytes.len() as i64)).await?;
    let cover = NewCover {
        blob_sha256: &sha256,
        mime: Some(image.mime),
        width: image.width,
        height: image.height,
        source: "upload",
    };
    let id = db::covers::add(&mut tx, owner, &cover).await?;
    tx.commit().await?;
    Ok(Json(AddedCover { id }))
}

#[derive(Debug, Deserialize)]
pub struct CopyCover {
    cover_id: Uuid,
}

/// `POST /api/tracks/{id}/covers/copy`, JSON `{"cover_id": "..."}`: adds a copy of
/// another cover (same image) to this track.
pub async fn copy_to_track(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CopyCover>,
) -> Result<Json<AddedCover>, AppError> {
    copy(state, user, CoverOwner::Track(id), body.cover_id).await
}

/// `POST /api/albums/{id}/covers/copy`, e.g. to use one of its tracks' covers.
pub async fn copy_to_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CopyCover>,
) -> Result<Json<AddedCover>, AppError> {
    copy(state, user, CoverOwner::Album(id), body.cover_id).await
}

async fn copy(state: AppState, user: CurrentUser, owner: CoverOwner, cover_id: Uuid) -> Result<Json<AddedCover>, AppError> {
    let mut tx = state.db(&user).await?;
    let id = db::covers::copy(&mut tx, owner, cover_id).await?;
    tx.commit().await?;
    Ok(Json(AddedCover { id }))
}

#[derive(Debug, Deserialize)]
pub struct Order {
    cover_ids: Vec<Uuid>,
}

/// `PUT /api/tracks/{id}/covers/order`, JSON `{"cover_ids": [...]}`: all its covers, the
/// main one first.
pub async fn order_track(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Order>,
) -> Result<Json<serde_json::Value>, AppError> {
    order(state, user, CoverOwner::Track(id), &body.cover_ids).await
}

/// `PUT /api/albums/{id}/covers/order`, like for a track.
pub async fn order_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Order>,
) -> Result<Json<serde_json::Value>, AppError> {
    order(state, user, CoverOwner::Album(id), &body.cover_ids).await
}

async fn order(state: AppState, user: CurrentUser, owner: CoverOwner, ids: &[Uuid]) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    db::covers::set_order(&mut tx, owner, ids).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// `DELETE /api/covers/{id}`: moves it to the trash (see `api::trash`).
pub async fn trash(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    if !db::covers::trash(&mut tx, id).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "trashed": id })))
}

/// `POST /api/albums/{id}/covers/apply-to-tracks`, JSON `{"cover_id": "..."}`: adds the
/// cover to every track of the album that doesn't have that image yet (see
/// `db::covers::apply_to_album_tracks`).
pub async fn apply_to_album_tracks(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(album_id): Path<Uuid>,
    Json(body): Json<CopyCover>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    if !db::sources::owner_exists(&mut tx, SourceOwner::Album(album_id)).await? {
        return Err(AppError::NotFound);
    }
    let (added, skipped) = db::covers::apply_to_album_tracks(&mut tx, album_id, body.cover_id).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "added": added, "skipped": skipped })))
}
