use axum::Json;
use axum::extract::{Multipart, Path, State};
use serde::Serialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::ingest::{self, IngestOptions, IngestOutcome, Rejected, ReplaceOptions};
use crate::model::sources::{self, Kind, NewSource, ValidSource};

#[derive(Debug, Serialize)]
pub struct UploadResult {
    filename: String,
    /// "created", "duplicate", "skipped" (the file was refused, see `error`) or "error"
    status: &'static str,
    track_id: Option<Uuid>,
    error: Option<String>,
}

/// `POST /api/upload`, multipart form.
///
/// Text fields, before the file(s) they apply to:
/// * `status`: initial track status; applies to all following files.
/// * `restrict_to`: a JSON array of group ids: the following files' tracks are created
///   restricted to those groups (needs `manage_access`). `[]` = admins only; send
///   `null` to go back to unrestricted.
/// * `origin` (required for every file): where the next file came from, one source snapshot
///   as JSON (see `NewSource`), e.g.
///   `{"platform":"custom","data":{"label":"discord","description":"sent by X"}}`.
/// * `sources`: other places the next file's track can be found, a JSON array of snapshots.
///
/// `origin` and `sources` apply to the next file only. Then any number of file fields
/// (any field name). Each file is streamed to disk while being hashed, so memory use stays
/// flat regardless of file size.
///
/// Example:
/// ```sh
/// curl -F 'origin={"platform":"soundcloud","data":{"song_id":555}}' \
///      -F 'sources=[{"platform":"netease","data":{"song_id":123}}]' -F file=@song.flac \
///      localhost:3000/api/upload
/// ```
pub async fn upload(
    State(state): State<AppState>,
    user: CurrentUser,
    mut multipart: Multipart,
) -> Result<Json<Vec<UploadResult>>, AppError> {
    let mut status = "released".to_string();
    let mut restrict_to: Option<Vec<Uuid>> = None;
    let mut next_origin: Option<ValidSource> = None;
    let mut next_sources: Vec<ValidSource> = Vec::new();
    let mut results = Vec::new();

    while let Some(mut field) = multipart.next_field().await.map_err(bad_request)? {
        let name = field.name().unwrap_or_default().to_string();

        // Fields without a filename are plain text fields.
        let Some(filename) = field.file_name().map(str::to_string) else {
            let value = field.text().await.map_err(bad_request)?;
            match name.as_str() {
                "status" => status = value,
                "restrict_to" => {
                    if !user.has(Permission::ManageAccess) {
                        return Err(AppError::Forbidden(
                            "restricting tracks needs the manage_access permission".to_string(),
                        ));
                    }
                    restrict_to = serde_json::from_str(&value).map_err(|err| {
                        AppError::BadRequest(format!("restrict_to: expected an array of group ids ({err})"))
                    })?;
                }
                "origin" => next_origin = Some(parse_source(&value)?),
                "sources" => next_sources = parse_sources(&value)?,
                other => return Err(AppError::BadRequest(format!("unknown field '{other}'"))),
            }
            continue;
        };

        // `take` moves the value out and leaves None / an empty Vec behind,
        // so these only apply to this one file.
        let sources = std::mem::take(&mut next_sources);
        let Some(origin) = next_origin.take() else {
            // Not reading the field's data; the next `next_field` call skips past it.
            results.push(UploadResult {
                filename,
                status: "skipped",
                track_id: None,
                error: Some("`origin` is required: where did this file come from?".to_string()),
            });
            continue;
        };

        let mut writer = state.store.writer().await?;
        while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
            writer.write(&chunk).await?;
        }
        let pending = writer.finish().await?;

        let options = IngestOptions {
            user: user.id(),
            uploaded_filename: filename.clone(),
            status: status.clone(),
            origin,
            sources,
            restrict_to: restrict_to.clone(),
        };
        // One bad file shouldn't fail the whole batch, so errors are reported per file.
        let (status, track_id, error) =
            match ingest::ingest(&state.pool, &state.store, pending, options).await {
                Ok(IngestOutcome::Created(id)) => ("created", Some(id), None),
                Ok(IngestOutcome::Duplicate(id)) => ("duplicate", id, None),
                Err(err) if err.downcast_ref::<Rejected>().is_some() => {
                    tracing::warn!("skipped {filename}: {err}");
                    ("skipped", None, Some(err.to_string()))
                }
                Err(err) => {
                    tracing::error!("ingest of {filename} failed: {err:#}");
                    ("error", None, Some("internal error".to_string()))
                }
            };
        results.push(UploadResult {
            filename,
            status,
            track_id,
            error,
        });
    }

    Ok(Json(results))
}

#[derive(Debug, Serialize)]
pub struct ReplaceResult {
    track_id: Uuid,
    old_file_id: Uuid,
    new_file_id: Uuid,
}

/// `POST /api/tracks/{id}/file`, multipart form: replace the track's file, e.g. with a
/// higher quality version. Text fields `origin` (required, as for uploads) and `reason`
/// (optional), then one file field.
///
/// Example: `curl -F reason="flac upgrade" -F file=@song.flac localhost:3000/api/tracks/<id>/file`
pub async fn replace_file(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(track_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<Json<ReplaceResult>, AppError> {
    let mut origin: Option<ValidSource> = None;
    let mut reason = String::new();

    while let Some(mut field) = multipart.next_field().await.map_err(bad_request)? {
        let name = field.name().unwrap_or_default().to_string();
        let Some(filename) = field.file_name().map(str::to_string) else {
            let value = field.text().await.map_err(bad_request)?;
            match name.as_str() {
                "origin" => origin = Some(parse_source(&value)?),
                "reason" => reason = value,
                other => return Err(AppError::BadRequest(format!("unknown field '{other}'"))),
            }
            continue;
        };

        let Some(origin) = origin else {
            return Err(AppError::BadRequest(
                "`origin` is required (before the file field): where did this file come from?"
                    .to_string(),
            ));
        };

        let mut writer = state.store.writer().await?;
        while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
            writer.write(&chunk).await?;
        }
        let pending = writer.finish().await?;

        let options = ReplaceOptions {
            user: user.id(),
            uploaded_filename: filename,
            origin,
            reason,
        };
        let outcome =
            ingest::replace_file(&state.pool, &state.store, track_id, pending, options).await?;
        return Ok(Json(ReplaceResult {
            track_id,
            old_file_id: outcome.old_file_id,
            new_file_id: outcome.new_file_id,
        }));
    }

    Err(AppError::BadRequest("no file field in the form".to_string()))
}

/// Parses and validates one source snapshot from a form field. Invalid data rejects the
/// whole request, before the file it belongs to is uploaded.
fn parse_source(json: &str) -> Result<ValidSource, AppError> {
    let source: NewSource = serde_json::from_str(json)
        .map_err(|err| AppError::BadRequest(format!("invalid source JSON: {err}")))?;
    sources::validate(Kind::Track, source).map_err(|err| AppError::BadRequest(err.to_string()))
}

/// Same as `parse_source`, for a JSON array of snapshots.
fn parse_sources(json: &str) -> Result<Vec<ValidSource>, AppError> {
    let list: Vec<NewSource> = serde_json::from_str(json)
        .map_err(|err| AppError::BadRequest(format!("invalid `sources` JSON: {err}")))?;
    list.into_iter()
        .map(|source| sources::validate(Kind::Track, source))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| AppError::BadRequest(err.to_string()))
}

fn bad_request(err: axum::extract::multipart::MultipartError) -> AppError {
    AppError::BadRequest(err.to_string())
}
