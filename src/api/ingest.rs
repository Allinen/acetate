//! `POST /api/ingest`: one file plus a description of it (automated ingestion, see
//! `ingest::describe`).

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Multipart, Query, State};
use serde::Deserialize;

use super::covers::MAX_COVER_BYTES;
use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::ingest::describe::{NewDescription, Report};
use crate::ingest::{self, IngestOptions};
use crate::storage::PendingBlob;

#[derive(Debug, Deserialize)]
pub struct IngestParams {
    /// Work out what would happen, and change nothing.
    #[serde(default)]
    dry_run: bool,
}

/// `POST /api/ingest[?dry_run=true]`, multipart form:
/// * `description`: JSON (see `NewDescription`, and the README for an example);
/// * `file`: the audio file;
/// * any image parts named in the description's `album.covers`.
///
/// Returns what was done (or would be): the track, how each artist and the album were
/// found, warnings. The metadata comes from the file's tags; the description gives ids.
pub async fn ingest(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<IngestParams>,
    mut multipart: Multipart,
) -> Result<Json<Report>, AppError> {
    let bad_request = |err: axum::extract::multipart::MultipartError| AppError::BadRequest(err.to_string());
    let mut description: Option<NewDescription> = None;
    let mut file: Option<(String, PendingBlob)> = None;
    let mut images: HashMap<String, Vec<u8>> = HashMap::new();

    while let Some(mut field) = multipart.next_field().await.map_err(bad_request)? {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "description" => {
                let text = field.text().await.map_err(bad_request)?;
                description = Some(serde_json::from_str(&text).map_err(|err| {
                    AppError::BadRequest(format!("invalid description JSON: {err}"))
                })?);
            }
            "file" => {
                if file.is_some() {
                    return Err(AppError::BadRequest("send one file per request".to_string()));
                }
                let filename = field.file_name().unwrap_or_default().to_string();
                // Streamed to disk while hashing, like uploads.
                let mut writer = state.store.writer().await?;
                while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
                    writer.write(&chunk).await?;
                }
                file = Some((filename, writer.finish().await?));
            }
            _ => {
                let mut bytes = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(bad_request)? {
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() > MAX_COVER_BYTES {
                        return Err(AppError::BadRequest(format!("image \"{name}\" is too large")));
                    }
                }
                images.insert(name, bytes);
            }
        }
    }

    let Some(description) = description else {
        return Err(AppError::BadRequest("the `description` field is missing".to_string()));
    };
    let Some((filename, pending)) = file else {
        return Err(AppError::BadRequest("the `file` field is missing".to_string()));
    };
    let description = description
        .validate(|name| images.remove(name))
        .map_err(AppError::BadRequest)?;
    if let Some(name) = images.keys().next() {
        return Err(AppError::BadRequest(format!(
            "form part \"{name}\" isn't named in the description"
        )));
    }
    if let Some(volume) = &description.volume {
        super::check_volume(&state, &user, volume)?;
    }
    if description.restrict_to.is_some() && !user.has(Permission::ManageAccess) {
        return Err(AppError::Forbidden(
            "restricting tracks needs the manage_access permission".to_string(),
        ));
    }

    let options = IngestOptions {
        user: user.id(),
        uploaded_filename: filename,
        origin: description.origin,
        status: description.status.unwrap_or_else(|| "released".to_string()),
        sources: description.track_sources,
        restrict_to: description.restrict_to,
        hints: description.hints,
        local_path: None,
        given: Default::default(),
        volume: description.volume,
    };
    let report =
        ingest::ingest_with_report(&state.pool, &state.store, pending, &options, params.dry_run)
            .await?;
    Ok(Json(report))
}
