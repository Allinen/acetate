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
