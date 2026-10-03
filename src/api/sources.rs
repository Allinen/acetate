//! External sources of tracks, albums and artists: adding snapshots (which is also how a
//! source is corrected: a new snapshot, the old one stays), and moving a wrong source to
//! the trash.

use axum::Json;
use axum::extract::{Path, State};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::sources::AddedSnapshot;
use crate::model::sources::{self, NewSource, SourceOwner};

/// `DELETE /api/sources/{id}`: moves a source to the trash (see `api::trash`). Refused for
/// a source that is where one of the files came from.
pub async fn trash(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    if !db::sources::trash(&mut tx, id).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "trashed": id })))
}

/// `POST /api/tracks/{id}/sources`, JSON body, e.g.
/// `{"platform": "soundcloud", "captured_at": "2024-01-01T00:00:00Z",
///   "data": {"song_id": 123, "song_permalink": ["https://soundcloud.com/a/b"]}}`
pub async fn add_track_source(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, &user, SourceOwner::Track(id), source).await
}

/// `POST /api/albums/{id}/sources`, same body as for tracks.
pub async fn add_album_source(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, &user, SourceOwner::Album(id), source).await
}

/// `POST /api/groupings/{id}/sources`, same body as for artists (a grouping's source is an
/// account, shaped like an artist's).
pub async fn add_grouping_source(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, &user, SourceOwner::Grouping(id), source).await
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    platform: String,
    /// The account's ids, any of them (e.g. a YouTube channel's handle and its UC… id).
    ids: Vec<String>,
}

/// `POST /api/sources/accounts`, JSON `{"platform": "soundcloud", "ids": ["123"]}`: the
/// artists and groupings (collectives) with one of these accounts (their source's id
/// field), as `[{"kind": "artist" | "grouping", "id", "name", "account"}]`.
pub async fn accounts(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Accounts>) -> Result<Json<Vec<db::sources::AccountOwner>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::sources::accounts(&mut tx, &body.platform, &body.ids).await?))
}

/// `POST /api/artists/{id}/sources`, same body as for tracks.
pub async fn add_artist_source(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(source): Json<NewSource>,
) -> Result<Json<AddedSnapshot>, AppError> {
    add(&state, &user, SourceOwner::Artist(id), source).await
}

async fn add(
    state: &AppState,
    user: &CurrentUser,
    owner: SourceOwner,
    source: NewSource,
) -> Result<Json<AddedSnapshot>, AppError> {
    let source = sources::validate(owner.kind(), source)?;
    let mut tx = state.db(user).await?;
    if !db::sources::owner_exists(&mut tx, owner).await? {
        return Err(AppError::NotFound);
    }
    let added = db::sources::add_snapshot(&mut tx, owner, &source).await?;
    tx.commit().await?;
    Ok(Json(added))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetOrigin {
    /// Its newest snapshot…
    #[serde(default)]
    source_id: Option<Uuid>,
    /// …or this snapshot (e.g. the one before a change, to undo it).
    #[serde(default)]
    snapshot_id: Option<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/files/{id}/origin`, JSON `{"source_id": "…"}` or `{"snapshot_id": "…"}` (plus
/// an optional `reason`): the file came from there (see `db::sources::set_file_origin`).
/// Returns the change's id (null: it came from there already).
pub async fn set_origin(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetOrigin>,
) -> Result<Json<serde_json::Value>, AppError> {
    let to = match (body.source_id, body.snapshot_id) {
        (Some(source), None) => db::sources::OriginTarget::Source(source),
        (None, Some(snapshot)) => db::sources::OriginTarget::Snapshot(snapshot),
        _ => return Err(AppError::BadRequest("give either `source_id` or `snapshot_id`".into())),
    };
    let mut tx = state.db(&user).await?;
    let change = db::sources::set_file_origin(&mut tx, id, to, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "change_id": change })))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetOriginalFilename {
    /// The name; null: not an original file.
    original_filename: Option<String>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/files/{id}/original-filename`, JSON `{"original_filename": "…" | null,
/// "reason": "…"}` (needs `edit_files`): whether the file is an original file, and its
/// name (see `db::og_files`). Answers whether anything changed.
pub async fn set_original_filename(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetOriginalFilename>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::og_files::set(&mut tx, id, body.original_filename.as_deref(), &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, serde::Deserialize)]
pub struct MarkOriginal {
    track_ids: Vec<Uuid>,
    #[serde(default)]
    reason: String,
    /// Only say what would change (nothing is kept).
    #[serde(default)]
    preview: bool,
}

/// `POST /api/tracks/mark-original`, JSON `{"track_ids": [...], "reason": "…", "preview":
/// true}` (needs `edit_files`): each track's file marked as an original file, under the
/// name it was uploaded with. `preview`: the same, answered and not kept.
pub async fn mark_original(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<MarkOriginal>,
) -> Result<Json<db::og_files::Marked>, AppError> {
    if body.track_ids.len() > 10_000 {
        return Err(AppError::BadRequest("at most 10000 tracks at once".into()));
    }
    let mut tx = state.db(&user).await?;
    let marked = db::og_files::mark_original(&mut tx, &body.track_ids, &body.reason).await?;
    if body.preview {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(marked))
}

/// `GET /api/source-shapes`: every supported platform/kind and its fields, so clients
/// (like the web UI) can build forms without hard-coding them.
pub async fn shapes() -> Json<serde_json::Value> {
    use crate::model::sources::{FieldType, Kind, SHAPES};

    let shapes: Vec<serde_json::Value> = SHAPES
        .iter()
        .map(|shape| {
            let kind = match shape.kind {
                Kind::Track => "track",
                Kind::Album => "album",
                Kind::Artist => "artist",
            };
            let fields: Vec<serde_json::Value> = shape
                .fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "type": match f.field_type {
                            FieldType::Int => "int",
                            FieldType::Str => "str",
                            FieldType::StrList => "strs",
                        },
                        "required": f.required,
                    })
                })
                .collect();
            serde_json::json!({
                "platform": shape.platform,
                "kind": kind,
                "id_field": shape.id_field,
                "fields": fields,
            })
        })
        .collect();
    Json(serde_json::Value::Array(shapes))
}
