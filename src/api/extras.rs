//! Extras (see `db::extras`): vocals, stems, samples, clips, project files. Listed with the
//! tracks, albums and artists they're linked to; made, changed and filled here.

use axum::Json;
use axum::extract::{Multipart, Path, Request, State};
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use super::media::serve_blob;
use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::extras::{Access, Extra, FileFacts, Link, LogEntry};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewExtra {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    links: Vec<Link>,
    /// Who sees it (needs `manage_access`); absent: like the first linked track.
    access: Option<Access>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/extras`, JSON `{"name": "…", "kind": "vocals", "links": [{"kind": "track",
/// "id": "…"}], "access": {"restricted": true, "groups": [...]}, "reason": "…"}`. Answers
/// `{id, access}` (the access it got: without `access`, the first linked track's).
pub async fn create(State(state): State<AppState>, user: CurrentUser, Json(body): Json<NewExtra>) -> Result<Json<serde_json::Value>, AppError> {
    if body.access.is_some() && !user.has(Permission::ManageAccess) {
        return Err(AppError::Forbidden("choosing who sees an extra needs the manage_access permission (without it, it's seen like its track)".into()));
    }
    let mut tx = state.db(&user).await?;
    let (id, access) = db::extras::create(&mut tx, &body.name, &body.kind, &body.links, body.access, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id, "access": access })))
}

/// `GET /api/extras/{id}`.
pub async fn get(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Extra>, AppError> {
    let mut tx = state.db(&user).await?;
    db::extras::get(&mut tx, id).await?.map(Json).ok_or(AppError::NotFound)
}

/// `GET /api/extras/{id}/history`: everything done to it, newest first.
pub async fn history(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Vec<LogEntry>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::extras::history(&mut tx, id).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/extras/{id}`, JSON `{"name": "…", "kind": "…", "reason": "…"}`.
pub async fn edit(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Edit>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::extras::edit(&mut tx, id, &body.name, &body.kind, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetAccess {
    restricted: bool,
    #[serde(default)]
    groups: Vec<Uuid>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/extras/{id}/access`, JSON `{"restricted": true, "groups": [...]}` (needs
/// `manage_access`).
pub async fn set_access(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<SetAccess>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let access = Access { restricted: body.restricted, groups: body.groups };
    let changed = db::extras::set_access(&mut tx, id, &access, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hide {
    hidden: bool,
    reason: String,
}

/// `POST /api/extras/{id}/hidden`, JSON `{"hidden": true}`: taken off (kept) or brought back.
pub async fn set_hidden(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Hide>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::extras::set_hidden(&mut tx, id, body.hidden, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// `POST /api/extra-files/{id}/hidden`, JSON `{"hidden": true}`: a file taken off or back.
pub async fn set_file_hidden(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Hide>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::extras::set_file_hidden(&mut tx, id, body.hidden, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkBody {
    link: Link,
    /// Undo the link instead.
    #[serde(default)]
    undo: bool,
    #[serde(default)]
    reason: String,
}

/// `POST /api/extras/{id}/links`, JSON `{"link": {"kind": "track", "id": "…"}, "undo":
/// false}`: linked to it (or, `undo`, not anymore). Answers `{changed}`.
pub async fn link(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<LinkBody>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = if body.undo {
        db::extras::unlink(&mut tx, id, body.link, &body.reason).await?
    } else {
        db::extras::link(&mut tx, id, body.link, &body.reason).await?
    };
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "changed": changed })))
}

/// `POST /api/extras/{id}/files`, multipart: `reason` (optional), then any number of file
/// fields. Each is stored (once, by its bytes) and added; audio gets its format and length.
/// Answers `{files: [id…]}`. Needs `upload` (and `edit_metadata`).
pub async fn add_files(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, mut multipart: Multipart) -> Result<Json<serde_json::Value>, AppError> {
    if !user.has(Permission::EditMetadata) {
        return Err(AppError::Forbidden("adding files to an extra needs edit_metadata".into()));
    }
    // Seen (and there) before anything is stored.
    {
        let mut tx = state.db(&user).await?;
        db::extras::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    }
    let mut reason = String::new();
    let mut added = Vec::new();
    while let Some(mut field) = multipart.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        let Some(filename) = field.file_name().map(str::to_string) else {
            let name = field.name().unwrap_or_default().to_string();
            let value = field.text().await.map_err(|e| AppError::BadRequest(e.to_string()))?;
            match name.as_str() {
                "reason" => reason = value,
                other => return Err(AppError::BadRequest(format!("unknown field '{other}'"))),
            }
            continue;
        };
        let mut writer = state.store.writer().await?;
        while let Some(chunk) = field.chunk().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
            writer.write(&chunk).await?;
        }
        let pending = writer.finish().await?;
        let facts = probe(pending.path(), &filename).await;
        let (sha256, size) = (pending.sha256.clone(), pending.size as i64);
        let (volume, _) = state.store.commit(pending, state.store.default_volume()).await?;
        let mut tx = state.db(&user).await?;
        db::blobs::register(&mut tx, &sha256, &volume, Some(size)).await?;
        added.push(db::extras::add_file(&mut tx, id, &sha256, &filename, size, &facts, &reason).await?);
        tx.commit().await?;
    }
    if added.is_empty() {
        return Err(AppError::BadRequest("no file field in the form".into()));
    }
    Ok(Json(serde_json::json!({ "files": added })))
}

/// A file's audio facts, if it's audio (anything else: none).
async fn probe(path: &std::path::Path, filename: &str) -> FileFacts {
    let path = path.to_path_buf();
    let extension = std::path::Path::new(filename).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    let probed = tokio::task::spawn_blocking(move || crate::ingest::probe::probe_file(&path, extension.as_deref())).await;
    match probed {
        Ok(Ok(p)) => FileFacts {
            container: p.container,
            codec: p.codec,
            sample_rate: p.sample_rate.and_then(|v| i32::try_from(v).ok()),
            channels: p.channels.and_then(|v| i16::try_from(v).ok()),
            bit_depth: p.bit_depth.and_then(|v| i16::try_from(v).ok()),
            duration_ms: p.duration_ms.and_then(|v| i64::try_from(v).ok()),
        },
        _ => FileFacts::default(),
    }
}

/// `GET /api/extra-files/{id}/stream`: the file as stored (Range requests work).
pub async fn stream(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, request: Request) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    let (sha256, name) = db::extras::file_blob(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    drop(tx);
    serve_blob(state.store.path_for(&sha256).await, &mime_for(&name), request).await
}

/// A file's type, by its name.
fn mime_for(name: &str) -> mime::Mime {
    let extension = std::path::Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    match extension.as_deref() {
        Some("flac") => "audio/flac",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "mp4" | "aac" | "alac") => "audio/mp4",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("aif" | "aiff") => "audio/aiff",
        Some("txt" | "md") => "text/plain",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
    .parse()
    .unwrap_or(mime::APPLICATION_OCTET_STREAM)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromProblem {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    links: Vec<Link>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/ingest-problems/{id}/extra`, JSON `{"name", "kind", "links", "reason"}`: a
/// file that didn't import becomes an extra (seen like the first linked track), and the
/// entry is resolved. Answers `{id}`.
pub async fn from_problem(State(state): State<AppState>, user: CurrentUser, Path(problem): Path<Uuid>, Json(body): Json<FromProblem>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let open = db::problems::get_open(&mut tx, problem).await?.ok_or(AppError::NotFound)?;
    let path = state.store.path_for(&open.sha256).await;
    let facts = probe(&path, &open.uploaded_filename).await;
    let (id, _) = db::extras::create(&mut tx, &body.name, &body.kind, &body.links, None, &body.reason).await?;
    db::extras::add_file(&mut tx, id, &open.sha256, &open.uploaded_filename, open.size_bytes, &facts, &body.reason).await?;
    db::problems::resolve(&mut tx, problem, "made an extra").await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id })))
}
