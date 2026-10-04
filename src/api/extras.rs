//! Extras (see `db::extras`): vocals, stems, samples, clips, project files. Listed with the
//! tracks, albums, artists and groupings they're linked to; made, changed and filled here.

use axum::Json;
use axum::extract::{Multipart, Path, Request, State};
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use super::media::serve_blob;
use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::extras::{About, Access, Extra, FromTrack, Link, LogEntry};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewExtra {
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    links: Vec<Link>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    source: String,
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
    let (id, access) = db::extras::create(&mut tx, &body.name, &body.kind, &About { description: body.description.clone(), source: body.source.clone() }, &body.links, body.access, &body.reason).await?;
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
    description: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/extras/{id}`, JSON `{"name", "kind", "description", "source", "reason"}`.
pub async fn edit(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Edit>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let changed = db::extras::edit(&mut tx, id, &body.name, &body.kind, &About { description: body.description.clone(), source: body.source.clone() }, &body.reason).await?;
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

/// `POST /api/extras/{id}/files`, multipart: `reason` and `imported_from` (where the files
/// were on the sender's disk; both optional, before the files), then any number of file
/// fields. Each is stored and added, unless an extra has the same bytes already; audio gets
/// its format and length. Answers `{files: [id…], already: [{file, extra_id, name, hidden}…]}`
/// (`already`: the files left out, by their names, and the extras that have them). Needs
/// `upload` (and `edit_metadata`).
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
    let mut imported_from = None;
    let mut added = Vec::new();
    let mut already = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|e| AppError::BadRequest(e.to_string()))? {
        let Some(filename) = field.file_name().map(str::to_string) else {
            let name = field.name().unwrap_or_default().to_string();
            let value = field.text().await.map_err(|e| AppError::BadRequest(e.to_string()))?;
            match name.as_str() {
                "reason" => reason = value,
                "imported_from" => imported_from = Some(value).filter(|v| !v.trim().is_empty()),
                other => return Err(AppError::BadRequest(format!("unknown field '{other}'"))),
            }
            continue;
        };
        let pending = super::uploads::receive(&state, &user, field).await?;
        let (sha256, size) = (pending.sha256.clone(), pending.size as i64);
        let mut tx = state.db(&user).await?;
        if let Some(stored) = db::extras::stored_as(&mut tx, &sha256).await? {
            already.push(serde_json::json!({ "file": filename, "extra_id": stored.extra_id, "name": stored.name, "hidden": stored.hidden }));
            continue;
        }
        let (volume, stored) = state.store.commit(pending, state.store.default_volume()).await?;
        let mut facts = crate::ingest::video::examine(&state.store, &stored, &filename).await;
        facts.imported_from = imported_from.clone();
        db::blobs::register(&mut tx, &sha256, &volume, Some(size)).await?;
        added.push(db::extras::add_file(&mut tx, id, &sha256, &filename, size, &facts, &reason).await?);
        tx.commit().await?;
    }
    if added.is_empty() && already.is_empty() {
        return Err(AppError::BadRequest("no file field in the form".into()));
    }
    Ok(Json(serde_json::json!({ "files": added, "already": already })))
}

/// `GET /api/extra-files/by-sha256/{sha256}`: the extra that has a file with these bytes
/// (`{extra_id, name, hidden}`), or 404. For a sender to skip a big file stored already
/// before uploading it (`acetate ingest --server`).
pub async fn by_sha256(State(state): State<AppState>, user: CurrentUser, Path(sha256): Path<String>) -> Result<Json<db::extras::StoredAs>, AppError> {
    let mut tx = state.db(&user).await?;
    db::extras::stored_as(&mut tx, &sha256.to_lowercase()).await?.map(Json).ok_or(AppError::NotFound)
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
    description: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    reason: String,
}

/// `POST /api/ingest-problems/{id}/extra`, JSON `{"name", "kind", "links", "reason"}`: a
/// file that didn't import becomes an extra (seen like the first linked track), and the
/// entry is resolved. Answers `{id}`. 409 if an extra has the same bytes already.
pub async fn from_problem(State(state): State<AppState>, user: CurrentUser, Path(problem): Path<Uuid>, Json(body): Json<FromProblem>) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let open = db::problems::get_open(&mut tx, problem).await?.ok_or(AppError::NotFound)?;
    if let Some(stored) = db::extras::stored_as(&mut tx, &open.sha256).await? {
        return Err(AppError::Conflict(format!("{} has this file already", stored.described())));
    }
    let path = state.store.path_for(&open.sha256).await;
    let facts = crate::ingest::video::examine(&state.store, &path, &open.uploaded_filename).await;
    let (id, _) = db::extras::create(&mut tx, &body.name, &body.kind, &About { description: body.description.clone(), source: body.source.clone() }, &body.links, None, &body.reason).await?;
    db::extras::add_file(&mut tx, id, &open.sha256, &open.uploaded_filename, open.size_bytes, &facts, &body.reason).await?;
    db::problems::resolve(&mut tx, problem, "made an extra").await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToExtra {
    #[serde(default)]
    kind: String,
    /// Its name (absent: the track's title).
    name: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    source: String,
    /// The song it's part of (it's linked to it).
    belongs_to: Option<Uuid>,
    #[serde(default)]
    reason: String,
    /// Only say what would happen (nothing is kept).
    #[serde(default)]
    preview: bool,
}

/// `POST /api/tracks/{id}/to-extra`, JSON `{"kind", "name", "description", "source",
/// "belongs_to", "reason", "preview"}`: the track becomes an extra (its file, the same
/// bytes); the track stays but nobody sees it (see `db::extras::track_to_extra`).
pub async fn track_to_extra(State(state): State<AppState>, user: CurrentUser, Path(track): Path<Uuid>, Json(body): Json<ToExtra>) -> Result<Json<FromTrack>, AppError> {
    let mut tx = state.db(&user).await?;
    let done = db::extras::track_to_extra(&mut tx, track, &body.kind, body.name.as_deref(), &About { description: body.description.clone(), source: body.source.clone() }, body.belongs_to, &body.reason).await?;
    if body.preview {
        tx.rollback().await?;
        return Ok(Json(done));
    }
    // A video imported as a track (its sound read): watched now, so its facts and thumbnail.
    if crate::ingest::video::is_video_name(&done.file) {
        let path = state.store.path_for(&done.sha256).await;
        let facts = crate::ingest::video::examine(&state.store, &path, &done.file).await;
        if facts.width.is_some() {
            db::extras::set_file_facts(&mut tx, done.file_id, &facts).await?;
        }
    }
    tx.commit().await?;
    Ok(Json(done))
}

/// `POST /api/extras/{id}/back-to-track`, JSON `{"reason"}` (optional): undoes
/// `to-extra`: the track is seen again, the extra taken off. Answers `{track_id}`.
pub async fn back_to_track(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, body: Option<Json<Hide>>) -> Result<Json<serde_json::Value>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let track = db::extras::back_to_track(&mut tx, id, &reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "track_id": track })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::StatusCode;
    use sha2::{Digest, Sha256};
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use crate::api::uploads::tests::{app, send, user};

    /// A form with one file (and an `imported_from`).
    fn form(name: &str, bytes: &str) -> Body {
        Body::from(format!(
            "--b\r\nContent-Disposition: form-data; name=\"imported_from\"\r\n\r\n/music/{name}\r\n\
             --b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\r\n{bytes}\r\n--b--\r\n"
        ))
    }

    #[sqlx::test]
    async fn the_same_bytes_are_not_added_to_extras_twice(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
        let (owner, app, _dir) = app(connect).await?;
        let alice = user(&owner, "alice").await?;
        let multipart = Some("multipart/form-data; boundary=b");
        let sha = hex::encode(Sha256::digest(b"some stems"));
        let (status, _) = send(&app, "GET", &format!("/api/extra-files/by-sha256/{sha}"), &alice, Body::empty(), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let make = |name: &str| serde_json::json!({ "name": name, "kind": "stems" }).to_string();
        let (_, first) = send(&app, "POST", "/api/extras", &alice, Body::from(make("First")), Some("application/json")).await;
        let first = first["id"].as_str().unwrap().to_string();
        let (status, got) = send(&app, "POST", &format!("/api/extras/{first}/files"), &alice, form("a.zip", "some stems"), multipart).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!((got["files"].as_array().unwrap().len(), got["already"].as_array().unwrap().len()), (1, 0));
        let imported_from: Option<String> = sqlx::query_scalar("SELECT imported_from FROM extra_files").fetch_one(&owner).await?;
        assert_eq!(imported_from.as_deref(), Some("/music/a.zip"));

        // Found by its hash; again (another name, another extra): left out, saying where it is.
        let (_, found) = send(&app, "GET", &format!("/api/extra-files/by-sha256/{sha}"), &alice, Body::empty(), None).await;
        assert_eq!((found["extra_id"].as_str(), found["name"].as_str(), found["hidden"].as_bool()), (Some(first.as_str()), Some("First"), Some(false)));
        let (_, second) = send(&app, "POST", "/api/extras", &alice, Body::from(make("Second")), Some("application/json")).await;
        let second = second["id"].as_str().unwrap().to_string();
        let (status, got) = send(&app, "POST", &format!("/api/extras/{second}/files"), &alice, form("b.zip", "some stems"), multipart).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(got["files"].as_array().unwrap().len(), 0, "{got}");
        assert_eq!((got["already"][0]["file"].as_str(), got["already"][0]["name"].as_str()), (Some("b.zip"), Some("First")));
        let files: i64 = sqlx::query_scalar("SELECT count(*) FROM extra_files").fetch_one(&owner).await?;
        assert_eq!(files, 1);
        Ok(())
    }
}
