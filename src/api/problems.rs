//! Files that didn't import (see `db::problems`): listed for review, with their audio.

use std::collections::HashMap;

use anyhow::Context;
use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::media::serve_blob;
use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::problems::Problem;
use crate::db::track_edits::TrackEdited;
use crate::db::tracks::TrackDetail;
use crate::ingest::describe::{NewDescription, Report, ReportStatus, ValidDescription};
use crate::ingest::{self, AddAs, Given, IngestOptions, Rejected, ReplaceOptions};
use crate::model::draft::{Draft, replace_ids};

#[derive(Debug, Deserialize)]
pub struct ListParams {
    /// Show resolved entries instead of open ones.
    #[serde(default)]
    resolved: bool,
    before: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /api/ingest-problems?resolved=false&before=...`: files that didn't import, newest
/// first. Your own uploads; admins see all (also the CLI's).
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<Problem>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = state.db(&user).await?;
    let mut problems = db::problems::list(&mut tx, params.resolved, params.before, limit).await?;
    // An error on our side can mention internals; that's for admins.
    if !user.has(Permission::Admin) {
        for problem in &mut problems {
            if problem.kind == "failed" {
                problem.reason = "an error on the server (an admin can see the details)".to_string();
            }
        }
    }
    Ok(Json(problems))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Resolve {
    /// What was done, e.g. "fixed the tags, imported again" or "not music".
    resolution: String,
}

/// `POST /api/ingest-problems/{id}/resolve`, JSON `{"resolution": "..."}` (optional).
/// Entries also resolve by themselves when the same file imports fine later.
pub async fn resolve(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    body: Option<Json<Resolve>>,
) -> Result<StatusCode, AppError> {
    let resolution = body.map(|Json(b)| b.resolution).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    if !db::problems::resolve(&mut tx, id, &resolution).await? {
        return Err(AppError::NotFound);
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/ingest-problems/{id}/audio`: the file as it was received (Range requests
/// work, for seeking).
pub async fn audio(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    request: Request,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    let found = db::problems::audio(&mut tx, id).await?;
    drop(tx);
    let Some((sha256, filename)) = found else {
        return Err(AppError::NotFound);
    };
    // The file wasn't probed (or probing it failed), so the type comes from its name.
    let extension = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    let mime: mime::Mime = match extension.as_deref() {
        Some("flac") => "audio/flac",
        Some("mp3") => "audio/mpeg",
        Some("m4a" | "mp4" | "aac" | "alac") => "audio/mp4",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("aif" | "aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
    .parse()
    .unwrap_or(mime::APPLICATION_OCTET_STREAM);
    serve_blob(state.store.path_for(&sha256).await, &mime, request).await
}

/// An open entry, ready to import again: the file in the blob store, and what it was
/// imported with (status, restriction, platform ids...).
async fn load_open(
    state: &AppState,
    user: &CurrentUser,
    id: Uuid,
) -> Result<(ingest::StagedFile, db::problems::OpenProblem, ValidDescription), AppError> {
    let mut tx = state.db(user).await?;
    let problem = db::problems::get_open(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    drop(tx);
    let Some(json) = problem.description.clone() else {
        return Err(AppError::BadRequest("this entry doesn't say where the file came from".to_string()));
    };
    let description: NewDescription = serde_json::from_value(json)
        .map_err(|err| AppError::BadRequest(format!("the stored description doesn't fit: {err}")))?;
    // The covers sent with it, kept as blobs and named by their hash.
    let mut covers = std::collections::HashMap::new();
    for sha256 in &problem.cover_blobs {
        let bytes = tokio::fs::read(state.store.path_for(sha256).await)
            .await
            .map_err(|err| AppError::Internal(anyhow::anyhow!("reading the kept cover {sha256}: {err}")))?;
        covers.insert(sha256.clone(), bytes);
    }
    let description = description
        .validate(|name| name.strip_prefix(crate::ingest::describe::STORED_COVER).and_then(|sha256| covers.get(sha256).cloned()))
        .map_err(AppError::BadRequest)?;
    // Already stored: it stays on the volume it's on.
    let file = ingest::StagedFile::stored(&state.store, &problem.sha256, problem.size_bytes as u64).await;
    Ok((file, problem, description))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImportWith {
    /// Used instead of the tags' title (for a file whose tags have none).
    title: Option<String>,
    /// Used instead of the tags' artists. Each is split on `;` like the ARTIST tag.
    artists: Vec<String>,
    /// The file is an original file (an og file) with this name, when its tags don't say.
    original_filename: Option<String>,
    /// The editor's changes to the new track: one of `/api/tracks/edit`'s `tracks`, made on
    /// the track a preview answered, with its ids (`track_id` can be left out).
    track: Option<Value>,
    /// What the track's changes refer to that's to be made first, as in `/api/tracks/edit`.
    new_artists: Vec<Value>,
    new_albums: Vec<Value>,
    new_groupings: Vec<Value>,
    /// The edit's reason (default: that it was made while importing the file).
    reason: String,
    /// The `draft` a preview answered, whose ids `track` uses.
    draft: Option<Draft>,
    /// Only say what would happen (nothing is kept).
    preview: bool,
}

/// What importing a file that didn't import did (or would).
#[derive(Debug, Serialize)]
pub struct Imported {
    /// The new track, or (a duplicate) the one that has the file already; None for one
    /// the user can't see.
    track_id: Option<Uuid>,
    report: Report,
    /// The new track as the import made it, before the edit: what the editor works on.
    /// None for a duplicate.
    track: Option<TrackDetail>,
    /// Its ids, to send back with an edit made on it.
    draft: Option<Draft>,
    /// What the edit did (or would), the new track's open flags after it included. Its
    /// `error`: why it can't be (then nothing is kept).
    edited: Option<TrackEdited>,
    /// What the track still lacks to be imported ("title", "artist"): a preview says so,
    /// importing refuses.
    missing: Vec<&'static str>,
}

/// `POST /api/ingest-problems/{id}/import`: imports the kept file again, with what it was
/// imported with the first time (its source included), and the track editor's changes to
/// the new track, in one transaction. JSON:
///
/// * `track`, `new_artists`, `new_albums`, `new_groupings`, `reason`: the changes, as for
///   `POST /api/tracks/edit` (one track; needs `edit_metadata`). A new track's ids only
///   come from a preview, so changes are made on the `track` a preview answers, and sent
///   with its `draft` (see `model::draft`); 409 if the import has come out differently
///   since.
/// * `preview`: the same, rolled back. Answers the track before the changes (to edit), its
///   draft, and what the changes did, the flags after them included.
/// * `original_filename`: it's an original file with this name, when its tags don't say.
/// * `title`, `artists`: values for what the tags lack, written into the track's first
///   revision (which says they were given by hand).
///
/// The file and its raw tags aren't changed. Revision 0 is what the tags say (even no
/// title); the changes come after it. Refused if the track has no title or no artist in
/// the end (from the tags, or the changes). A file whose bytes are a track's already isn't
/// changed: that's said, and nothing is edited.
pub async fn import(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(with): Json<ImportWith>,
) -> Result<Json<Imported>, AppError> {
    let (file, problem, mut description) = load_open(&state, &user, id).await?;
    if let Some(name) = with.original_filename.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        description.hints.original_filename = Some(name.to_string());
    }
    if description.restrict_to.is_some() && !user.has(Permission::ManageAccess) {
        return Err(AppError::Forbidden(
            "this file was to be restricted; importing it needs the manage_access permission".to_string(),
        ));
    }
    let editing = with.track.is_some();
    if editing && !user.has(Permission::EditMetadata) {
        return Err(AppError::Forbidden("changing the new track needs the `edit_metadata` permission".to_string()));
    }
    // A new track, which the changes are for: not merged into one that has the same file.
    if editing {
        description.hints.no_duplicate_merge = true;
    }
    let uploaded_filename = problem.uploaded_filename;
    let options = IngestOptions {
        user: user.id(),
        uploaded_filename: uploaded_filename.clone(),
        origin: description.origin,
        status: description.status.unwrap_or_else(|| "released".to_string()),
        sources: description.track_sources,
        restrict_to: description.restrict_to,
        hints: description.hints,
        local_path: problem.path,
        given: Given {
            title: with.title,
            artists: with.artists,
            edited_after: true,
        },
        volume: None,
        tags_from: None, // a tagged copy was only there for the original request
    };

    let mut tx = state.db(&user).await?;
    let report = ingest::ingest_in(&mut tx, &state.store, &file, &options, with.preview)
        .await
        .map_err(ingest::as_rejected)?;
    let new_track = report.track_id.filter(|_| report.status == ReportStatus::Created);
    let Some(track_id) = new_track else {
        // Already stored: its track got the sources, as any import of it does.
        if with.preview {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
        return Ok(Json(Imported { track_id: report.track_id, report, track: None, draft: None, edited: None, missing: Vec::new() }));
    };

    // The track before the changes, and its ids by place.
    let track = db::tracks::get_track(&mut tx, track_id, user.has(Permission::ViewHistory))
        .await?
        .ok_or_else(|| AppError::BadRequest("the new track would be restricted so that you can't see it: import it without changes".to_string()))?;
    let draft = Draft::of(&serde_json::to_value(&track).map_err(anyhow::Error::from)?);

    // The changes, their ids the new track's.
    let map = match &with.draft {
        Some(sent) => sent.map_to(&draft).map_err(AppError::Conflict)?,
        None => HashMap::new(),
    };
    let mut entry = with.track.unwrap_or_else(|| json!({}));
    replace_ids(&mut entry, &map);
    let mut new_albums = Value::Array(with.new_albums);
    replace_ids(&mut new_albums, &map);
    let Some(fields) = entry.as_object_mut() else {
        return Err(AppError::BadRequest("`track` is the new track's changes, an object".to_string()));
    };
    match fields.get("track_id").map(|t| t.as_str().and_then(|t| t.parse::<Uuid>().ok())) {
        None => {}
        Some(Some(id)) if id == track_id => {}
        Some(_) => return Err(AppError::BadRequest("`track.track_id` isn't the draft's track".to_string())),
    }
    fields.insert("track_id".into(), json!(track_id));
    let reason = match with.reason.trim() {
        "" => format!("while importing \"{uploaded_filename}\""),
        reason => reason.to_string(),
    };
    let edit: db::track_edits::Edit = serde_json::from_value(json!({
        "reason": reason,
        "new_artists": with.new_artists,
        "new_albums": new_albums,
        "new_groupings": with.new_groupings,
        "tracks": [entry],
    }))
    .map_err(|err| AppError::BadRequest(format!("the changes don't fit: {err}")))?;

    // Each track of an edit fails on its own (its savepoint rolled back): the error is kept
    // to refuse the whole import with.
    let may = db::track_edits::May { relations: user.has(Permission::EditRelations), sources: user.has(Permission::EditSources) };
    let failed = std::sync::Mutex::new(None);
    let mut edited = db::track_edits::edit(&mut tx, &edit, may, |err| {
        let failure = super::failure_of(&err);
        *failed.lock().unwrap() = Some(err);
        failure
    })
    .await?;
    let mut edited = edited.tracks.pop().context("the edit answers its track")?;

    // A title and an artist, from the tags or the changes (a collective's account counts:
    // its grouping has the track, as when importing).
    let lacks = sqlx::query!(
        r#"SELECT btrim(title) = '' AS "untitled!",
                  NOT EXISTS (SELECT 1 FROM track_credits WHERE track_id = t.id) AS "uncredited!"
           FROM tracks t WHERE id = $1"#,
        track_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut missing = Vec::new();
    if lacks.untitled {
        missing.push("title");
    }
    if lacks.uncredited && report.collectives.is_empty() {
        missing.push("artist");
    }
    if missing.is_empty() {
        ingest::flag_possible_duplicates(&mut tx, track_id, &report.same_song_as).await?;
        db::track_edits::read_flags(&mut tx, std::slice::from_mut(&mut edited)).await?;
    }

    if with.preview {
        tx.rollback().await?;
    } else {
        if let Some(err) = failed.into_inner().unwrap() {
            return Err(AppError::Internal(err));
        }
        if !missing.is_empty() {
            return Err(AppError::Internal(
                Rejected::Invalid(format!("a track needs a title and an artist, and this one has no {}", missing.join(" and no "))).into(),
            ));
        }
        tx.commit().await?;
    }
    Ok(Json(Imported { track_id: Some(track_id), report, track: Some(track), draft: Some(draft), edited: Some(edited), missing }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddToTrack {
    track_id: Uuid,
    /// `current_file` (the track's file becomes a previous one) or `previous_file`.
    add_as: AddAs,
    #[serde(default)]
    reason: String,
    /// The file is an original file (an og file) with this name, when its tags don't say.
    #[serde(default)]
    original_filename: Option<String>,
}

/// `POST /api/ingest-problems/{id}/add-to-track`, JSON `{"track_id": "...", "add_as":
/// "current_file" | "previous_file", "reason": "..."}`: for a file that belongs to a track
/// that's already there, e.g. a better copy without tags. Needs `replace_file`.
pub async fn add_to_track(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AddToTrack>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (file, problem, description) = load_open(&state, &user, id).await?;
    let options = ReplaceOptions {
        user: user.id(),
        uploaded_filename: problem.uploaded_filename,
        origin: description.origin,
        reason: body.reason,
        volume: None,
        original_filename: body.original_filename.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
    };
    let outcome = ingest::add_file_to_track(&state.pool, &file, body.track_id, body.add_as, &options).await?;
    Ok(Json(serde_json::json!({
        "track_id": body.track_id,
        "file_id": outcome.new_file_id,
        "current_file_id": match body.add_as {
            AddAs::CurrentFile => outcome.new_file_id,
            AddAs::PreviousFile => outcome.old_file_id,
        },
    })))
}
