//! Tracks: listing, details, and editing (through revisions, see `db::revisions`).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::revisions::{self, MergePicks, MergedTracks, Source, Written};
use crate::model::{AlbumMembership, Relation, TrackFacts, TrackMetadata, diff_relations};

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Id of the last track on the previous page.
    before: Option<Uuid>,
    limit: Option<i64>,
    /// Substring search in title and artist (case-insensitive).
    q: Option<String>,
}

// Only tracks the user can see (row-level security).
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<db::tracks::TrackSummary>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let query = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut tx = state.db(&user).await?;
    let tracks = match query {
        Some(q) => db::tracks::search_tracks(&mut tx, q, params.before, limit).await?,
        None => db::tracks::list_tracks(&mut tx, params.before, limit).await?,
    };
    Ok(Json(tracks))
}

/// Revisions and previous files are only included with the `view_history` permission.
/// A merged track redirects (308) to the track it was merged into.
pub async fn get(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let with_history = user.has(Permission::ViewHistory);
    let mut tx = state.db(&user).await?;
    if let Some(target) = db::tracks::merged_into(&mut tx, id).await? {
        return Ok(Redirect::permanent(&format!("/api/tracks/{target}")).into_response());
    }
    let Some(mut track) = db::tracks::get_track(&mut tx, id, with_history).await? else {
        return Err(AppError::NotFound);
    };
    if with_history {
        track.merges = revisions::merges_into(&mut tx, id).await?;
    }
    Ok(Json(track).into_response())
}


// ---- Editing ----
//
// Every edit sends `base_revision_id`: the revision the client's form was filled from.
// If someone else saved in the meantime, the edit is refused with 409 instead of silently
// undoing their change. Edits send the full new state (missing lists = empty).

#[derive(Debug, Deserialize)]
pub struct EditVariant {
    base_revision_id: Uuid,
    data: TrackMetadata,
    #[serde(default)]
    reason: String,
    /// This state existed in the real world (e.g. an old official title), rather than
    /// being a correction.
    #[serde(default)]
    is_canon: bool,
}

/// `PUT /api/tracks/{id}/variants/{variant_id}`: new metadata (title, date, genres,
/// comment, credits) for a variant. One internal revision.
pub async fn edit_variant(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((track_id, variant_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<EditVariant>,
) -> Result<Json<Written>, AppError> {
    let mut tx = state.db(&user).await?;
    let written = revisions::write_internal(
        &mut tx,
        track_id,
        variant_id,
        Some(body.base_revision_id),
        &body.data,
        Source::Edit,
        &body.reason,
        body.is_canon,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Deserialize)]
pub struct EditFacts {
    base_revision_id: Uuid,
    /// Required (unlike the lists): leaving it out shouldn't silently mean "released".
    status: String,
    #[serde(default)]
    albums: Vec<AlbumMembership>,
    #[serde(default)]
    relations: Vec<Relation>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/tracks/{id}/facts`: new status, albums and relations. One external revision
/// (plus one on each track whose relation to this one changed). Changing relations also
/// needs the `edit_relations` permission.
pub async fn edit_facts(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    Json(body): Json<EditFacts>,
) -> Result<Json<Written>, AppError> {
    let mut tx = state.db(&user).await?;
    if !user.has(Permission::EditRelations) {
        let current = revisions::current_relations(&mut tx, track_id).await?;
        let (added, removed) = diff_relations(&current, &body.relations);
        if !added.is_empty() || !removed.is_empty() {
            return Err(AppError::Forbidden(
                "changing relations requires the `edit_relations` permission".to_string(),
            ));
        }
    }
    let facts = TrackFacts {
        status: body.status,
        albums: body.albums,
        relations: body.relations,
    };
    let written = revisions::write_external(
        &mut tx,
        track_id,
        Some(body.base_revision_id),
        &facts,
        Source::Edit,
        &body.reason,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Deserialize)]
pub struct NewVariant {
    label: String,
    /// The variant to start as a copy of. Default: the primary variant.
    copy_from: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct CreatedVariant {
    variant_id: Uuid,
    revision_id: Uuid,
}

/// `POST /api/tracks/{id}/variants`: a new variant, starting as a copy of another.
pub async fn create_variant(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    Json(body): Json<NewVariant>,
) -> Result<Json<CreatedVariant>, AppError> {
    let mut tx = state.db(&user).await?;
    let (variant_id, written) =
        revisions::create_variant(&mut tx, track_id, &body.label, body.copy_from).await?;
    tx.commit().await?;
    Ok(Json(CreatedVariant {
        variant_id,
        revision_id: written.revision_id,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RenameVariant {
    label: String,
}

/// `PATCH /api/tracks/{id}/variants/{variant_id}`: rename a variant.
pub async fn rename_variant(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((track_id, variant_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RenameVariant>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.db(&user).await?;
    revisions::rename_variant(&mut tx, track_id, variant_id, &body.label).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PrimaryVariant {
    variant_id: Uuid,
}

/// `PUT /api/tracks/{id}/primary-variant`: which variant is shown.
pub async fn set_primary_variant(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    Json(body): Json<PrimaryVariant>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.db(&user).await?;
    revisions::set_primary_variant(&mut tx, track_id, body.variant_id).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Restore {
    reason: String,
}

/// `POST /api/revisions/{id}/restore`: restores a track revision (internal or external)
/// by writing a new one with its data. Body optional: `{"reason": "..."}`.
pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(revision_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<Written>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let written = revisions::restore_track_revision(&mut tx, revision_id, &reason).await?;
    tx.commit().await?;
    Ok(Json(written))
}

// ---- Merging ----

#[derive(Debug, Deserialize)]
pub struct Merge {
    /// The duplicate, merged into the track in the path.
    from: Uuid,
    #[serde(default)]
    base: MergeBase,
    /// Which side wins each field; missing = "left" (the track in the path).
    #[serde(default)]
    pick: MergePicks,
    #[serde(default)]
    reason: String,
    /// Also merge the singles the two are each alone on (see
    /// `db::album_merges::merge_tracks_and_singles`). Needs `edit_metadata` too.
    #[serde(default)]
    merge_singles: bool,
}

/// The primary variants' latest revisions the choice was made from (409 if one changed).
#[derive(Debug, Default, Deserialize)]
pub struct MergeBase {
    into_revision_id: Option<Uuid>,
    from_revision_id: Option<Uuid>,
}

/// `POST /api/tracks/{id}/merge`, JSON `{"from": "<duplicate>", "base": {...},
/// "pick": {"title": "right", "file": "right", ...}, "reason": ""}`: merges the duplicate
/// into this track (see `db::revisions::merge_tracks`). Also needs `replace_file`, since
/// it changes which file a track plays.
pub async fn merge(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Merge>,
) -> Result<Json<MergedTracks>, AppError> {
    if !user.has(Permission::ReplaceFile) {
        return Err(AppError::Forbidden(
            "merging changes which file a track plays, which needs the `replace_file` permission"
                .to_string(),
        ));
    }
    if body.merge_singles && !user.has(Permission::EditMetadata) {
        return Err(AppError::Forbidden("merging the singles along needs the `edit_metadata` permission".to_string()));
    }
    let mut tx = state.db(&user).await?;
    let merged = crate::db::album_merges::merge_tracks_and_singles(
        &mut tx,
        id,
        body.from,
        &body.pick,
        body.base.into_revision_id,
        body.base.from_revision_id,
        &body.reason,
        body.merge_singles,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(merged))
}

/// `POST /api/track-merges/{id}/undo` (body optional: `{"reason": "..."}`): gives back both
/// tracks as they were, if nothing depends on the merge anymore (400 listing what's in
/// the way otherwise). Returns the id of the track that was merged away (back now).
pub async fn undo_merge(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(merge_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !user.has(Permission::ReplaceFile) {
        return Err(AppError::Forbidden(
            "undoing a merge changes which file a track plays, which needs the `replace_file` permission"
                .to_string(),
        ));
    }
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let restored = revisions::undo_track_merge(&mut tx, merge_id, &reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "restored_track_id": restored })))
}

#[derive(Debug, Deserialize)]
pub struct UseFile {
    /// Play the file this track plays (the two share it from then on).
    #[serde(default)]
    file_of: Option<Uuid>,
    /// Or: play this previous file of the track again.
    #[serde(default)]
    previous_file: Option<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/tracks/{id}/use-file`, JSON `{"file_of": "<track id>"}` or
/// `{"previous_file": "<file id>"}` (plus an optional `reason`): switches which file the
/// track plays, without uploading anything. The file it played becomes a previous file.
/// Both tracks must be visible to the user.
pub async fn use_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UseFile>,
) -> Result<Json<db::track_files::FileSwitch>, AppError> {
    let mut tx = state.db(&user).await?;
    let switched = match (body.file_of, body.previous_file) {
        (Some(other), None) => db::track_files::use_file_of(&mut tx, id, other, &body.reason).await?,
        (None, Some(file)) => db::track_files::use_previous_file(&mut tx, id, file, &body.reason).await?,
        _ => return Err(AppError::BadRequest("give either `file_of` or `previous_file`".to_string())),
    };
    tx.commit().await?;
    Ok(Json(switched))
}
