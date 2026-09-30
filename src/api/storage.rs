//! `GET /api/storage` (admins): what's stored on each volume, by kind (see
//! `db::blobs::totals`).

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;

#[derive(Serialize)]
pub struct Volume {
    name: String,
    /// Where new files go unless told otherwise.
    default: bool,
    parts: Vec<db::blobs::Total>,
}

pub async fn totals(State(state): State<AppState>, user: CurrentUser) -> Result<Json<Vec<Volume>>, AppError> {
    let mut tx = state.db(&user).await?;
    let mut totals = db::blobs::totals(&mut tx).await?;
    // Every volume configured, in order (also empty ones), then any others the database
    // still names.
    let mut volumes: Vec<Volume> = state
        .store
        .volumes()
        .iter()
        .map(|v| Volume { name: v.name.clone(), default: v.name == state.store.default_volume(), parts: Vec::new() })
        .collect();
    for total in totals.drain(..) {
        match volumes.iter_mut().find(|v| v.name == total.volume) {
            Some(volume) => volume.parts.push(total),
            None => volumes.push(Volume { name: total.volume.clone(), default: false, parts: vec![total] }),
        }
    }
    Ok(Json(volumes))
}
