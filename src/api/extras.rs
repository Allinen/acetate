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
use crate::db::extras::{About, Access, Extra, FromTrack, Link, LogEntry, MergeReport};

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

/// `POST /api/extras/{id}/to-track`: a YouTube video kept as an extra becomes a track, from
/// what YouTube says of it (read by yt-dlp, as the editor's import does): its title,
/// artists (or the channel), date, the video as the track's source and origin, its
/// thumbnail as the cover. The track plays the extra's file (the same bytes); the extra is
/// taken off (kept). Needs `upload` too.
///
/// The body is that of `POST /api/ingest-problems/{id}/import` (the track editor's
/// changes to the new track, `preview`: only say what would happen, the draft to edit)
/// with `link`: the video's link or id (absent: the id at the end of the extra's name, or
/// its file's, `Title [id].mp4`). The answer is that one's too, with `no_cover`: why the
/// thumbnail couldn't be had.
pub async fn to_track(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(with): Json<super::problems::ImportWith>) -> Result<Json<super::problems::Imported>, AppError> {
    if !user.has(Permission::Upload) {
        return Err(AppError::Forbidden("making a track of an extra needs the upload permission".into()));
    }
    let extra = {
        let mut tx = state.db(&user).await?;
        db::extras::get(&mut tx, id).await?.ok_or(AppError::NotFound)?
    };
    if extra.hidden || extra.merged_into.is_some() {
        return Err(AppError::BadRequest("it is taken off".into()));
    }
    if extra.from_track.is_some() {
        return Err(AppError::BadRequest("it was a track: make that one a track again instead".into()));
    }
    if with.track.is_some() && !user.has(Permission::EditMetadata) {
        return Err(AppError::Forbidden("changing the new track needs the `edit_metadata` permission".into()));
    }
    let files: Vec<&db::extras::ExtraFile> = extra.files.iter().filter(|f| !f.hidden).collect();
    let [file] = files[..] else {
        return Err(AppError::BadRequest(format!("it has {} files; only an extra with one can become a track", files.len())));
    };
    let video_id = match with.link.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        Some(link) => super::metadata::video_id(link).ok_or_else(|| AppError::BadRequest("not a YouTube video's link or id".into()))?,
        None => crate::ingest::youtube::id_in_name(&extra.name)
            .or_else(|| crate::ingest::youtube::id_in_name(&file.name))
            .ok_or_else(|| AppError::BadRequest(format!("no YouTube id at the end of “{}” (“… [id]”): give the video's link", extra.name)))?,
    };
    let json = super::metadata::fetch_youtube(&state, &user, &video_id).await?;
    let video = crate::ingest::youtube::parse(&json).map_err(AppError::BadRequest)?;
    if video.id != video_id {
        return Err(AppError::BadRequest(format!("YouTube answered for another video ({})", video.id)));
    }
    Ok(Json(make_track(&state, &user, &extra, file, video, with).await?))
}

/// `to_track` once the video is known: the track, from `file` of `extra`.
async fn make_track(state: &AppState, user: &CurrentUser, extra: &Extra, file: &db::extras::ExtraFile, video: crate::ingest::youtube::Video, with: super::problems::ImportWith) -> Result<super::problems::Imported, AppError> {
    use crate::ingest::describe::{ArtistHint, Described, DescribedCredit, Hints};
    use crate::model::sources::{self, Kind, NewSource};
    let invalid = |err: sources::InvalidSource| AppError::BadRequest(err.to_string());
    let track_source = sources::validate(Kind::Track, NewSource { platform: "youtube".into(), source_id: None, captured_at: None, data: video.source.clone() }).map_err(invalid)?;
    let channel_source = match &video.channel {
        Some(channel) => Some(sources::validate(Kind::Artist, NewSource { platform: "youtube".into(), source_id: None, captured_at: None, data: serde_json::json!({ "channel": channel }) }).map_err(invalid)?),
        None => None,
    };
    let artists: Vec<ArtistHint> = video
        .artists
        .iter()
        .enumerate()
        .map(|(n, name)| ArtistHint { name: name.clone(), id: None, sources: channel_source.clone().filter(|_| n == 0).into_iter().collect(), platform_name: None })
        .collect();
    let described = Described {
        title: video.title.clone(),
        credits: video.artists.iter().enumerate().map(|(n, name)| DescribedCredit { name: name.clone(), role: "primary".into(), artist: Some(n) }).collect(),
        release_date: video.release_date.clone(),
        ..Default::default()
    };
    // Its thumbnail, if it can be had (up to 15 seconds; the track is made without it
    // otherwise, and the answer says why there is no cover).
    let mut covers = Vec::new();
    let mut no_cover = None;
    if let Some(url) = &video.thumbnail {
        match fetch_thumbnail(url).await {
            Ok(bytes) => covers.push(bytes),
            Err(why) => no_cover = Some(why),
        }
    }
    let restrict_to = extra.is_restricted.then(|| extra.groups.iter().map(|g| g.id).collect::<Vec<_>>());
    if restrict_to.is_some() && !user.has(Permission::ManageAccess) {
        return Err(AppError::Forbidden("this extra is restricted, and so would its track be: that needs the manage_access permission".into()));
    }
    let options = crate::ingest::IngestOptions {
        user: user.id(),
        uploaded_filename: file.name.clone(),
        origin: track_source.clone(),
        status: "released".into(),
        sources: vec![track_source],
        restrict_to,
        hints: Hints { artists, described: Some(described), track_covers: covers, no_duplicate_merge: true, ..Default::default() },
        local_path: None,
        edited_after: with.track.is_some(),
        volume: None,
        tags_from: None,
    };
    let staged = crate::ingest::StagedFile::stored(&state.store, &file.sha256, file.size_bytes as u64).await;
    let mut imported = super::problems::import_with(state, user, &staged, &options, with, &file.name, Some(extra.id)).await?;
    imported.no_cover = no_cover;
    Ok(imported)
}

/// Hosts YouTube's thumbnails are on.
fn is_thumbnail_host(host: &str) -> bool {
    host == "ytimg.com" || host.ends_with(".ytimg.com")
}

/// A video's thumbnail: only from YouTube's image hosts (also after a redirect, of which
/// there are at most 3), an image of at most 10 MB. Err says why not.
async fn fetch_thumbnail(url: &str) -> Result<Vec<u8>, String> {
    // The editor previews again at every change: fetched once.
    static RECENT: std::sync::Mutex<Vec<(String, std::time::Instant, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());
    {
        let mut recent = RECENT.lock().unwrap();
        recent.retain(|(_, at, _)| at.elapsed() < std::time::Duration::from_secs(600));
        if let Some((_, _, bytes)) = recent.iter().find(|(kept, _, _)| kept == url) {
            return Ok(bytes.clone());
        }
    }
    let bytes = fetch_thumbnail_now(url).await?;
    let mut recent = RECENT.lock().unwrap();
    if recent.len() >= 20 {
        recent.remove(0);
    }
    recent.push((url.to_string(), std::time::Instant::now(), bytes.clone()));
    Ok(bytes)
}

async fn fetch_thumbnail_now(url: &str) -> Result<Vec<u8>, String> {
    let on_youtube = |url: &reqwest::Url| url.scheme() == "https" && url.host_str().is_some_and(is_thumbnail_host);
    let url: reqwest::Url = url.parse().map_err(|_| "its thumbnail's address isn't one".to_string())?;
    if !on_youtube(&url) {
        return Err(format!("its thumbnail is not on YouTube's image hosts ({})", url.host_str().unwrap_or("?")));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 3 || !on_youtube(attempt.url()) { attempt.stop() } else { attempt.follow() }
        }))
        .build()
        .map_err(|err| err.to_string())?;
    let answer = client.get(url).send().await.map_err(|err| format!("its thumbnail couldn't be fetched: {err}"))?;
    if !answer.status().is_success() {
        return Err(format!("its thumbnail couldn't be fetched: {}", answer.status()));
    }
    let bytes = answer.bytes().await.map_err(|err| format!("its thumbnail couldn't be read: {err}"))?;
    if bytes.len() > 10 << 20 {
        return Err("its thumbnail is over 10 MB".into());
    }
    if crate::model::image_info(&bytes).is_none() {
        return Err("its thumbnail isn't an image".into());
    }
    Ok(bytes.to_vec())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Merge {
    /// The extras that go into this one.
    sources: Vec<Uuid>,
    #[serde(default)]
    reason: String,
    /// Only say what would happen (nothing is kept).
    #[serde(default)]
    preview: bool,
}

/// `POST /api/extras/{id}/merge`, JSON `{"sources": ["…"], "reason", "preview"}`: the
/// extras in `sources` are merged into this one (see `db::extras::merge`): their files and
/// links move here, and each goes to the trash (taking it out undoes the merge). Answers
/// what moved and what stayed.
pub async fn merge(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Merge>) -> Result<Json<MergeReport>, AppError> {
    let mut tx = state.db(&user).await?;
    let done = db::extras::merge(&mut tx, id, &body.sources, &body.reason).await?;
    if body.preview {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(done))
}

/// `POST /api/extras/{id}/undo-track`, JSON `{"reason"}` (optional): undoes `to-track`. The
/// track it made is hidden as a track made into an extra is (so this extra is "made from"
/// it, and can be made a track again), and the extra is brought back. Answers `{track_id}`.
pub async fn undo_track(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, body: Option<Json<Hide>>) -> Result<Json<serde_json::Value>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let track = db::extras::undo_made_track(&mut tx, id, &reason).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "track_id": track })))
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

    use crate::api::uploads::tests::{app, send, state, user};

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

    /// A video kept as an extra becomes a track from what YouTube says of it: looked at
    /// first (nothing kept), then made (the extra taken off), and not twice.
    #[sqlx::test]
    async fn an_extra_becomes_a_track_from_its_video(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
        use crate::auth::{CurrentUser, Login, Permission};
        use crate::db;

        let (owner, state, _dir) = state(connect.clone()).await?;
        let app = crate::api::router(state.clone());
        let alice = user(&owner, "alice").await?;
        let audio = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/01 - Glass Harbor.flac"))?;
        let name = "Glass Harbor [dQw4w9WgXcQ].flac";
        let mut form = format!("--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\r\n").into_bytes();
        form.extend(&audio);
        form.extend(b"\r\n--b--\r\n");
        let (_, made) = send(&app, "POST", "/api/extras", &alice, Body::from(serde_json::json!({ "name": name, "kind": "video" }).to_string()), Some("application/json")).await;
        let id: uuid::Uuid = made["id"].as_str().unwrap().parse()?;
        let (status, got) = send(&app, "POST", &format!("/api/extras/{id}/files"), &alice, Body::from(form), Some("multipart/form-data; boundary=b")).await;
        assert_eq!(status, StatusCode::OK, "{got}");

        let alice_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'").fetch_one(&owner).await?;
        let user = CurrentUser { login: Some(Login { id: alice_id, username: "alice".into(), session_id: uuid::Uuid::nil() }), permissions: vec![Permission::Admin] };
        let extra = {
            let mut tx = state.db(&user).await?;
            db::extras::get(&mut tx, id).await?.unwrap()
        };
        let video = crate::ingest::youtube::parse(&serde_json::json!({
            "id": "dQw4w9WgXcQ", "title": "Mira Vale - Glass Harbor (Official Video)", "channel": "Mira Vale", "channel_id": "UCabc",
            "upload_date": "20210604"
        }))
        .unwrap();
        let tracks = || async { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks").fetch_one(&owner).await };

        let with = |preview: bool, reason: &str| super::super::problems::ImportWith { preview, reason: reason.into(), ..Default::default() };
        let looked = super::make_track(&state, &user, &extra, &extra.files[0], video.clone(), with(true, "")).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let looked = serde_json::to_value(&looked)?;
        assert_eq!(looked["track"]["title"], "Mira Vale - Glass Harbor (Official Video)");
        assert!(looked["draft"].is_object(), "a draft to edit: {looked}");
        assert_eq!(tracks().await?, 0, "only looked at");
        let hidden: bool = sqlx::query_scalar("SELECT hidden_at IS NOT NULL FROM extras WHERE id = $1").bind(id).fetch_one(&owner).await?;
        assert!(!hidden);

        let made = super::make_track(&state, &user, &extra, &extra.files[0], video.clone(), with(false, "from YouTube")).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let track = serde_json::to_value(&made)?["track_id"].as_str().map(|t| t.parse::<uuid::Uuid>()).transpose()?.expect("a track");
        assert_eq!(tracks().await?, 1);
        let (title, release, artist): (String, Option<String>, String) = sqlx::query_as(
            "SELECT t.title, t.release_date::text, a.name FROM tracks t JOIN track_credits c ON c.track_id = t.id AND c.role = 'primary' JOIN artists a ON a.id = c.artist_id WHERE t.id = $1",
        )
        .bind(track)
        .fetch_one(&owner)
        .await?;
        assert_eq!((title.as_str(), release.as_deref(), artist.as_str()), ("Mira Vale - Glass Harbor (Official Video)", Some("2021-06-04"), "Mira Vale"));
        let sources: Vec<(String, Option<String>)> = sqlx::query_as("SELECT s.platform, s.external_id FROM sources s ORDER BY s.platform, s.external_id").fetch_all(&owner).await?;
        assert!(sources.contains(&("youtube".to_string(), Some("dQw4w9WgXcQ".to_string()))), "the video is its source: {sources:?}");
        assert!(sources.contains(&("youtube".to_string(), Some("UCabc".to_string()))), "the channel is the artist's: {sources:?}");
        let hidden: bool = sqlx::query_scalar("SELECT hidden_at IS NOT NULL FROM extras WHERE id = $1").bind(id).fetch_one(&owner).await?;
        assert!(hidden, "the extra is taken off");

        // Again (with the extra as it was read before): it is taken off now.
        let again = super::make_track(&state, &user, &extra, &extra.files[0], video, with(false, "")).await;
        assert!(matches!(again, Err(super::AppError::BadRequest(why)) if why == "it is taken off"));
        assert_eq!(tracks().await?, 1);

        // Not an original file, whatever the extra's file was called.
        let og: Option<String> = sqlx::query_scalar("SELECT original_filename FROM files WHERE id = (SELECT file_id FROM tracks WHERE id = $1)").bind(track).fetch_one(&owner).await?;
        assert_eq!(og, None);

        // The track says which extra it was made from.
        let (status, got) = send(&app, "GET", &format!("/api/tracks/{track}"), &alice, Body::empty(), None).await;
        assert_eq!((status, got["made_from_extra"].as_str()), (StatusCode::OK, Some(id.to_string().as_str())), "{got}");

        // Undone: the extra is back, and the track hidden as one made into an extra is.
        let extra = {
            let mut tx = state.db(&user).await?;
            db::extras::get(&mut tx, id).await?.unwrap()
        };
        assert_eq!(extra.made_track, Some(track));
        let (status, got) = send(&app, "POST", &format!("/api/extras/{id}/undo-track"), &alice, Body::from("{}"), Some("application/json")).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        let extra = {
            let mut tx = state.db(&user).await?;
            db::extras::get(&mut tx, id).await?.unwrap()
        };
        assert_eq!((extra.hidden, extra.from_track, extra.made_track), (false, Some(track), None));
        let (status, _) = send(&app, "POST", &format!("/api/extras/{id}/undo-track"), &alice, Body::from("{}"), Some("application/json")).await;
        assert_ne!(status, StatusCode::OK, "once");
        Ok(())
    }
}
