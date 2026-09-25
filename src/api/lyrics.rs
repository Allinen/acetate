//! Lyrics: adding and editing a track's lyrics entries, through revisions (see
//! `db::revisions`). Entries are never deleted; a wrong one is edited.

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db::revisions::{self, Source, Written};
use crate::model::LyricsData;

#[derive(Debug, Deserialize)]
pub struct NewLyrics {
    #[serde(flatten)]
    data: LyricsData,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Serialize)]
pub struct CreatedLyrics {
    id: Uuid,
    #[serde(flatten)]
    written: Written,
}

/// `POST /api/tracks/{id}/lyrics`, JSON `{"body": "...", "is_official": false, "language": "en", "reason": ""}`
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    Json(body): Json<NewLyrics>,
) -> Result<Json<CreatedLyrics>, AppError> {
    let mut tx = state.db(&user).await?;
    let (id, written) =
        revisions::create_lyrics(&mut tx, track_id, "manual", &body.data, Source::Edit, &body.reason)
            .await?;
    tx.commit().await?;
    Ok(Json(CreatedLyrics { id, written }))
}

#[derive(Debug, Deserialize)]
pub struct EditLyrics {
    /// The revision the edit was made from (409 if it isn't the latest anymore).
    base_revision_id: Option<Uuid>,
    #[serde(flatten)]
    data: LyricsData,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/lyrics/{id}`, JSON `{"base_revision_id": "...", "body": "...", "is_official": true, ...}`
pub async fn edit(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<EditLyrics>,
) -> Result<Json<Written>, AppError> {
    let mut tx = state.db(&user).await?;
    let written = revisions::write_lyrics(
        &mut tx,
        id,
        body.base_revision_id,
        &body.data,
        Source::Edit,
        &body.reason,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Restore {
    reason: String,
}

/// `POST /api/lyrics-revisions/{id}/restore`. Body optional: `{"reason": "..."}`.
pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(revision_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<Written>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let written = revisions::restore_lyrics_revision(&mut tx, revision_id, &reason).await?;
    tx.commit().await?;
    Ok(Json(written))
}
