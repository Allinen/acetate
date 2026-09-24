//! HTTP API. One module per resource.

mod media;
mod sources;
mod tracks;
mod upload;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use sqlx::PgPool;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::ingest::Rejected;
use crate::model::sources::InvalidSource;
use crate::storage::BlobStore;

/// Shared state handed to every handler. Cloning it is cheap: `PgPool` is a
/// reference-counted handle, and `BlobStore` is just two paths.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub store: BlobStore,
    /// Directory with the web UI's static files.
    pub web_dir: std::path::PathBuf,
}

const UPLOAD_LIMIT: usize = 4 * 1024 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/tracks", get(tracks::list))
        .route("/api/tracks/{id}", get(tracks::get))
        .route("/api/tracks/{id}/sources", post(sources::add_track_source))
        .route("/api/source-shapes", get(sources::shapes))
        .route("/api/files/{id}/stream", get(media::stream_file))
        .route("/api/covers/{id}", get(media::cover))
        .route("/api/albums/{id}", get(sources::get_album))
        .route("/api/albums/{id}/sources", post(sources::add_album_source))
        .route("/api/artists/{id}", get(sources::get_artist))
        .route("/api/artists/{id}/sources", post(sources::add_artist_source))
        // Uploads are streamed to disk, so a large limit doesn't mean large memory use.
        .route(
            "/api/upload",
            post(upload::upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route(
            "/api/tracks/{id}/file",
            post(upload::replace_file).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        // Everything that isn't an API route: the web UI's static files.
        .fallback_service(ServeDir::new(&state.web_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Error type for handlers. `?` converts other errors into `Internal` via the `From` impl.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    BadRequest(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(err) => {
                if let Some(rejected) = err.downcast_ref::<Rejected>() {
                    // Refused input (bad file, missing tags...): the client's problem.
                    let status = match rejected {
                        Rejected::TrackNotFound(_) => StatusCode::NOT_FOUND,
                        Rejected::AlreadyStored { .. } => StatusCode::CONFLICT,
                        _ => StatusCode::UNPROCESSABLE_ENTITY,
                    };
                    (status, rejected.to_string())
                } else if let Some(invalid) = err.downcast_ref::<InvalidSource>() {
                    (StatusCode::UNPROCESSABLE_ENTITY, invalid.to_string())
                } else if let Some(msg) = foreign_key_violation(&err) {
                    (StatusCode::BAD_REQUEST, msg)
                } else {
                    tracing::error!("internal error: {err:#}");
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
                }
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

// Lets handlers use `?` on anything that converts into anyhow::Error (sqlx errors, io errors...).
impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        AppError::Internal(err.into())
    }
}

/// Inserting a value that doesn't exist in a lookup table (e.g. an unknown platform or
/// status) is a bad request, not a server error.
fn foreign_key_violation(err: &anyhow::Error) -> Option<String> {
    let db_err = err.downcast_ref::<sqlx::Error>()?.as_database_error()?;
    if !db_err.is_foreign_key_violation() {
        return None;
    }
    Some(format!(
        "invalid reference ({})",
        db_err.constraint().unwrap_or("unknown constraint")
    ))
}
