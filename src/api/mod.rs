//! HTTP API. One module per resource.
//!
//! Every `/api` route declares the permission it needs, in `router` below, so it's the one
//! place to check who can do what. The exceptions are logging in/out and opening share
//! links, which anyone can do. Requests without a login are made by a guest, who has no
//! permissions.

mod auth;
mod catalog;
mod media;
mod rate_limit;
mod review;
mod shares;
mod sources;
mod tracks;
mod upload;
mod users;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use std::sync::Arc;

use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::auth::CurrentUser;
// Lets the router say `Upload` instead of `Permission::Upload`.
use crate::auth::Permission::*;
use crate::db;
use crate::db::revisions::EditRejected;
use crate::ingest::Rejected;
use crate::model::sources::InvalidSource;
use crate::storage::BlobStore;
use auth::require;
pub use rate_limit::RateLimits;

/// Shared state handed to every handler. Cloning it is cheap: `PgPool` is a
/// reference-counted handle, and `BlobStore` is just two paths.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub store: BlobStore,
    /// Directory with the web UI's static files.
    pub web_dir: std::path::PathBuf,
    /// Mark the session cookie `Secure` (only sent over HTTPS).
    pub secure_cookies: bool,
    /// Take the client's address from `X-Forwarded-For` (set by a reverse proxy).
    pub trust_proxy: bool,
    /// Rate limits for logins and share links. `Arc`: shared by all clones of the state,
    /// i.e. all requests, instead of each clone getting its own copy.
    pub rate_limits: Arc<RateLimits>,
}

impl AppState {
    /// A transaction acting as `user`. All database work for a request goes through
    /// this (see `db::begin`).
    pub async fn db(&self, user: &CurrentUser) -> sqlx::Result<Transaction<'static, Postgres>> {
        db::begin(&self.pool, user.id()).await
    }
}

const UPLOAD_LIMIT: usize = 4 * 1024 * 1024 * 1024;

pub fn router(state: AppState) -> Router {
    // Only the auth routes go without `require`: everyone, guests included, can use them.
    // Anything else without `require` would be open to guests, so don't. (The other routes
    // anyone can use, login and share links, are in `public` below.)
    let api = Router::new()
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/auth/sessions", get(auth::sessions).delete(auth::end_other_sessions))
        .route("/api/auth/sessions/{id}", delete(auth::end_session))
        .route("/api/users", require(&[ManageUsers], get(users::list).post(users::create)))
        .route("/api/users/{id}/groups", require(&[ManageUsers], put(users::set_user_groups)))
        .route("/api/shares", require(&[Share], get(shares::list).post(shares::create)))
        .route("/api/shares/{id}", require(&[Share], delete(shares::revoke)))
        .route("/api/search", require(&[Browse], get(catalog::search)))
        .route("/api/vocab", require(&[Browse], get(catalog::vocab)))
        .route("/api/tracks", require(&[Browse], get(tracks::list)))
        .route("/api/tracks/{id}", require(&[Browse], get(tracks::get)))
        // Editing. Changing relations also needs `edit_relations` (checked in the handler).
        .route("/api/tracks/{id}/facts", require(&[EditMetadata], put(tracks::edit_facts)))
        .route("/api/tracks/{id}/variants", require(&[EditMetadata], post(tracks::create_variant)))
        .route(
            "/api/tracks/{id}/variants/{variant_id}",
            require(&[EditMetadata], put(tracks::edit_variant).patch(tracks::rename_variant)),
        )
        .route(
            "/api/tracks/{id}/primary-variant",
            require(&[EditMetadata], put(tracks::set_primary_variant)),
        )
        .route("/api/revisions/{id}/restore", require(&[RestoreRevision], post(tracks::restore)))
        .route(
            "/api/artist-revisions/{id}/restore",
            require(&[RestoreRevision], post(catalog::restore_artist)),
        )
        .route(
            "/api/album-revisions/{id}/restore",
            require(&[RestoreRevision], post(catalog::restore_album)),
        )
        .route("/api/review", require(&[EditMetadata], get(review::list)))
        .route("/api/review/{id}/resolve", require(&[EditMetadata], post(review::resolve)))
        .route("/api/tracks/{id}/sources", require(&[EditSources], post(sources::add_track_source)))
        .route("/api/source-shapes", require(&[Browse], get(sources::shapes)))
        // Previous (replaced) files additionally need `view_history`, checked in the handler.
        .route("/api/files/{id}/stream", require(&[Stream], get(media::stream_file)))
        .route("/api/covers/{id}", require(&[Browse], get(media::cover)))
        .route(
            "/api/albums",
            require(&[Browse], get(catalog::search_albums))
                .merge(require(&[EditMetadata], post(catalog::create_album))),
        )
        .route(
            "/api/albums/{id}",
            require(&[Browse], get(catalog::get_album))
                .merge(require(&[EditMetadata], put(catalog::edit_album))),
        )
        .route("/api/albums/{id}/sources", require(&[EditSources], post(sources::add_album_source)))
        .route(
            "/api/artists",
            require(&[Browse], get(catalog::search_artists))
                .merge(require(&[EditMetadata], post(catalog::create_artist))),
        )
        .route(
            "/api/artists/{id}",
            require(&[Browse], get(catalog::get_artist))
                .merge(require(&[EditMetadata], put(catalog::edit_artist))),
        )
        .route("/api/artists/{id}/tracks", require(&[Browse], get(catalog::artist_tracks)))
        .route("/api/artists/{id}/merge", require(&[EditMetadata], post(catalog::merge_artist)))
        .route("/api/artists/{id}/sources", require(&[EditSources], post(sources::add_artist_source)))
        // Uploads are streamed to disk, so a large limit doesn't mean large memory use.
        .route(
            "/api/upload",
            require(&[Upload], post(upload::upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT))),
        )
        .route(
            "/api/tracks/{id}/file",
            require(
                &[ReplaceFile],
                post(upload::replace_file).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
            ),
        )
        // `route_layer`: only for the routes above, so unknown paths are still a plain 404.
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::authenticate));

    // Public: no session needed, and a stale session cookie doesn't get in the way.
    // A share link's token is its own authorization.
    let share_links = Router::new()
        .route("/api/share/{token}", get(shares::view))
        .route("/api/share/{token}/tracks/{track_id}/stream", get(shares::stream))
        .route("/api/share/{token}/covers/{cover_id}", get(shares::cover))
        .route_layer(middleware::from_fn_with_state(state.clone(), rate_limit::limit_shares));
    let public = Router::new()
        .route("/api/auth/login", post(auth::login))
        .merge(share_links)
        // The web UI's page for a share link; the page itself reads the token from the URL.
        .route_service("/s/{token}", ServeFile::new(state.web_dir.join("index.html")));

    Router::new()
        .merge(public)
        .merge(api)
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
    /// Not logged in (or the session is invalid): 401.
    Unauthorized(&'static str),
    /// Logged in, but not allowed: 403.
    Forbidden(String),
    /// Rate limited: 429, try again after this long.
    TooManyRequests(std::time::Duration),
    Conflict(String),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::TooManyRequests(wait) => {
                // Round up, so "try again in 0 seconds" never happens.
                let secs = wait.as_secs() + 1;
                let body = json!({ "error": format!("too many requests, try again in {secs} seconds") });
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, secs.to_string())],
                    Json(body),
                )
                    .into_response();
            }
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.to_string()),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            AppError::Internal(err) => {
                if let Some(rejected) = err.downcast_ref::<Rejected>() {
                    // Refused input (bad file, missing tags...): the client's problem.
                    let status = match rejected {
                        Rejected::TrackNotFound(_) => StatusCode::NOT_FOUND,
                        Rejected::AlreadyStored { .. } => StatusCode::CONFLICT,
                        _ => StatusCode::UNPROCESSABLE_ENTITY,
                    };
                    (status, rejected.to_string())
                } else if let Some(rejected) = err.downcast_ref::<EditRejected>() {
                    let status = match rejected {
                        EditRejected::Stale { .. } => StatusCode::CONFLICT,
                        EditRejected::Invalid(_) => StatusCode::BAD_REQUEST,
                        EditRejected::NotFound(_) => StatusCode::NOT_FOUND,
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
