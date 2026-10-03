//! HTTP API. One module per resource.
//!
//! Every `/api` route declares the permission it needs, in `router` below, so it's the one
//! place to check who can do what. The exceptions are logging in/out and opening share
//! links, which anyone can do. Requests without a login are made by a guest, who has no
//! permissions.

mod access;
mod annotations;
mod auth;
mod covers;
mod lyrics;
mod notes;
mod extras;
mod catalog;
mod groupings;
mod groups;
mod media;
mod metadata;
mod playlists;
mod rate_limit;
mod problems;
#[cfg(test)]
mod problems_tests;
mod review;
mod shares;
mod storage;
mod sources;
mod tracks;
mod trash;
mod upload;
pub mod uploads;
pub mod upload_limits;
mod ingest;
mod library;
mod users;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use std::sync::Arc;

use tower_http::compression::CompressionLayer;
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
    /// Smaller copies of cover images (`GET /api/images/{hash}?size=`).
    pub images: Arc<crate::storage::images::ImageCache>,
    /// How much may be uploaded per request and per day (see `upload_limits`).
    pub upload_limits: Arc<upload_limits::UploadLimits>,
    /// Chunked uploads under way (see `uploads`).
    pub uploads: Arc<uploads::Uploads>,
}

impl AppState {
    /// A transaction acting as `user`. All database work for a request goes through
    /// this (see `db::begin`).
    pub async fn db(&self, user: &CurrentUser) -> sqlx::Result<Transaction<'static, Postgres>> {
        db::begin(&self.pool, user.id()).await
    }
}

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
        // Listing groups is also for choosing who may see a restricted track.
        .route(
            "/api/groups",
            require(&[ManageUsers, ManageAccess], get(groups::list))
                .merge(require(&[ManageUsers], post(groups::create))),
        )
        .route(
            "/api/groups/{id}",
            require(&[ManageUsers], put(groups::update).delete(groups::delete)),
        )
        .route("/api/shares", require(&[Share], get(shares::list).post(shares::create)))
        .route("/api/shares/{id}", require(&[Share], delete(shares::revoke)))
        .route("/api/search", require(&[Browse], get(catalog::search)))
        .route("/api/vocab", require(&[Browse], get(catalog::vocab)))
        .route("/api/tracks", require(&[Browse], get(tracks::list)))
        .route("/api/tracks/{id}", require(&[Browse], get(tracks::get)))
        .route("/api/tracks/{id}/access", require(&[ManageAccess], get(access::get).put(access::set)))
        .route("/api/albums/{id}/access", require(&[ManageAccess], put(access::set_album)))
        // Editing. Changing relations also needs `edit_relations` (checked in the handler).
        .route("/api/tracks/{id}/facts", require(&[EditMetadata], put(tracks::edit_facts)))
        // Merging a duplicate into a track (also needs replace_file, checked in the handler).
        .route("/api/tracks/{id}/merge", require(&[EditMetadata], post(tracks::merge)))
        .route("/api/track-merges/{id}/undo", require(&[EditMetadata], post(tracks::undo_merge)))
        // Which file a track plays: another track's (shared), or one of its previous files.
        .route("/api/tracks/{id}/use-file", require(&[ReplaceFile], post(tracks::use_file)))
        .route("/api/tracks/{id}/duplicate", require(&[EditMetadata], post(tracks::duplicate)))
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
        // A user's own plays, stars and ratings.
        .route("/api/tracks/{id}/star", require(&[Browse], post(annotations::star_track).delete(annotations::unstar_track)))
        .route("/api/albums/{id}/star", require(&[Browse], post(annotations::star_album).delete(annotations::unstar_album)))
        .route("/api/artists/{id}/star", require(&[Browse], post(annotations::star_artist).delete(annotations::unstar_artist)))
        .route("/api/tracks/{id}/rating", require(&[Browse], put(annotations::rate_track)))
        .route("/api/albums/{id}/rating", require(&[Browse], put(annotations::rate_album)))
        .route("/api/artists/{id}/rating", require(&[Browse], put(annotations::rate_artist)))
        .route("/api/tracks/{id}/played", require(&[Browse], post(annotations::played)))
        .route("/api/review", require(&[EditMetadata], get(review::list)))
        .route("/api/review/{id}/resolve", require(&[EditMetadata], post(review::resolve)))
        .route("/api/review/resolve", require(&[EditMetadata], post(review::resolve_many)))
        .route("/api/review/ambiguous", require(&[EditMetadata], post(review::settle_ambiguous)))
        .route("/api/review/merge-duplicates", require(&[EditMetadata], post(review::merge_duplicates)))
        .route("/api/tracks/singles", require(&[EditMetadata], post(review::make_singles)))
        .route("/api/tracks/edit", require(&[EditMetadata], post(tracks::edit_many)))
        // Files that didn't import: whoever uploaded them (admins: all).
        .route("/api/ingest-problems", require(&[Upload], get(problems::list)))
        .route("/api/ingest-problems/{id}/audio", require(&[Upload], get(problems::audio)))
        .route("/api/ingest-problems/{id}/import", require(&[Upload], post(problems::import)))
        .route(
            "/api/ingest-problems/{id}/add-to-track",
            require(&[ReplaceFile], post(problems::add_to_track)),
        )
        .route("/api/tracks/{id}/sources", require(&[EditSources], post(sources::add_track_source)))
        .route("/api/sources/{id}", require(&[EditSources], delete(sources::trash)))
        .route("/api/sources/accounts", require(&[Browse], post(sources::accounts)))
        .route("/api/metadata/youtube", require(&[EditMetadata], post(metadata::youtube)))
        // Lyrics (entries are edited, never deleted).
        .route("/api/tracks/{id}/lyrics", require(&[EditMetadata], post(lyrics::create)))
        .route("/api/lyrics/{id}", require(&[EditMetadata], put(lyrics::edit).delete(lyrics::remove)))
        // Notes on tracks, albums and artists (listed with their details).
        .route("/api/notes", require(&[EditMetadata], post(notes::create)))
        .route("/api/notes/{id}", require(&[EditMetadata], put(notes::edit)))
        .route("/api/notes/{id}/hidden", require(&[EditMetadata], post(notes::set_hidden)))
        .route("/api/notes/{id}/revisions", require(&[ViewHistory], get(notes::revisions)))
        // Extras: vocals, stems, samples… (listed with what they're linked to).
        .route("/api/extras", require(&[EditMetadata], post(extras::create)))
        .route("/api/extras/{id}", require(&[Browse], get(extras::get)).merge(require(&[EditMetadata], put(extras::edit))))
        .route("/api/extras/{id}/history", require(&[ViewHistory], get(extras::history)))
        .route("/api/extras/{id}/access", require(&[ManageAccess], put(extras::set_access)))
        .route("/api/extras/{id}/hidden", require(&[EditMetadata], post(extras::set_hidden)))
        .route("/api/extras/{id}/links", require(&[EditMetadata], post(extras::link)))
        .route("/api/extras/{id}/back-to-track", require(&[EditMetadata], post(extras::back_to_track)))
        .route("/api/tracks/{id}/to-extra", require(&[EditMetadata], post(extras::track_to_extra)))
        .route(
            "/api/extras/{id}/files",
            require(&[Upload], post(extras::add_files).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit))),
        )
        .route("/api/extra-files/{id}/hidden", require(&[EditMetadata], post(extras::set_file_hidden)))
        .route("/api/extra-files/{id}/stream", require(&[Stream], get(extras::stream)))
        .route("/api/ingest-problems/{id}/extra", require(&[EditMetadata], post(extras::from_problem)))
        .route("/api/lyrics-revisions/{id}/restore", require(&[RestoreRevision], post(lyrics::restore)))
        // Covers. Uploads need a bigger body limit than the default 2 MB.
        .route(
            "/api/tracks/{id}/covers",
            require(
                &[EditMetadata],
                post(covers::upload_to_track).layer(DefaultBodyLimit::max(covers::MAX_COVER_BYTES + 64 * 1024)).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit)),
            ),
        )
        .route(
            "/api/albums/{id}/covers",
            require(
                &[EditMetadata],
                post(covers::upload_to_album).layer(DefaultBodyLimit::max(covers::MAX_COVER_BYTES + 64 * 1024)).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit)),
            ),
        )
        // An image to make covers of in a track edit (`/api/tracks/edit`).
        .route(
            "/api/covers/staged",
            require(
                &[EditMetadata],
                post(covers::stage).layer(DefaultBodyLimit::max(covers::MAX_COVER_BYTES + 64 * 1024)).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit)),
            ),
        )
        .route("/api/tracks/{id}/covers/copy", require(&[EditMetadata], post(covers::copy_to_track)))
        .route("/api/albums/{id}/covers/copy", require(&[EditMetadata], post(covers::copy_to_album)))
        .route(
            "/api/albums/{id}/covers/apply-to-tracks",
            require(&[EditMetadata], post(covers::apply_to_album_tracks)),
        )
        .route("/api/tracks/{id}/covers/order", require(&[EditMetadata], put(covers::order_track)))
        .route("/api/albums/{id}/covers/order", require(&[EditMetadata], put(covers::order_album)))
        // Trash. Restoring checks per kind (covers: edit_metadata, sources: edit_sources,
        // files: admin); purging is for admins only.
        .route("/api/trash", require(&[EditMetadata, EditSources], get(trash::list)))
        .route("/api/trash/restore", require(&[EditMetadata, EditSources], post(trash::restore)))
        .route("/api/trash/purge", require(&[Admin], post(trash::purge)))
        .route("/api/storage", require(&[Admin], get(storage::totals)))
        .route("/api/files/replaced", require(&[Admin], get(trash::replaced_files)))
        .route("/api/files/trash", require(&[Admin], post(trash::trash_files)))
        .route("/api/source-shapes", require(&[Browse], get(sources::shapes)))
        // Previous (replaced) files additionally need `view_history`, checked in the handler.
        .route("/api/files/{id}/stream", require(&[Stream], get(media::stream_file)))
        .route("/api/files/{id}/origin", require(&[EditSources], put(sources::set_origin)))
        .route("/api/files/{id}/original-filename", require(&[EditFiles], put(sources::set_original_filename)))
        .route("/api/tracks/mark-original", require(&[EditFiles], post(sources::mark_original)))
        .route("/api/images/{hash}", require(&[Browse], get(media::image)))
        .route(
            "/api/covers/{id}",
            require(&[Browse], get(media::cover)).merge(require(&[EditMetadata], delete(covers::trash))),
        )
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
        .route("/api/albums/{id}/merge", require(&[EditMetadata], post(catalog::merge_album)))
        .route(
            "/api/album-merges/{id}/undo",
            require(&[EditMetadata], get(catalog::preview_album_merge_undo).post(catalog::undo_album_merge)),
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
        .route("/api/library/albums", require(&[Browse], get(catalog::list_albums)))
        // The whole library for local browsing: big, so compressed (only this route; audio
        // and covers are compressed already).
        .route("/api/catalog", require(&[Browse], get(library::catalog)).layer(CompressionLayer::new()))
        // Playlists: everyone logged in has their own (see `db::playlists`).
        .route("/api/playlists", require(&[Browse], get(playlists::list).post(playlists::create)))
        .route(
            "/api/playlists/{id}",
            require(&[Browse], get(playlists::get).patch(playlists::update).delete(playlists::trash)),
        )
        .route("/api/playlists/{id}/restore", require(&[Browse], post(playlists::restore)))
        .route("/api/playlists/{id}/entries", require(&[Browse], post(playlists::add).delete(playlists::remove)))
        .route("/api/playlists/{id}/order", require(&[Browse], put(playlists::reorder)))
        .route("/api/playlists/{id}/log", require(&[Browse], get(playlists::log)))
        .route("/api/playlists/{id}/log/{change}/undo", require(&[Browse], post(playlists::undo)))
        .route("/api/artists/{id}/aliases/{alias_id}/uses", require(&[Browse], get(catalog::alias_uses)))
        .route("/api/artists/{id}/merge", require(&[EditMetadata], post(catalog::merge_artist)))
        .route("/api/artists/{id}/credited-as", require(&[EditMetadata], post(catalog::credit_as)))
        .route("/api/artists/{id}/sources", require(&[EditSources], post(sources::add_artist_source)))
        // Groupings (collectives, eras, projects).
        .route("/api/groupings", require(&[EditMetadata], post(groupings::create)))
        .route("/api/groupings/{id}", require(&[Browse], get(groupings::get)).merge(require(&[EditMetadata], put(groupings::edit))))
        .route("/api/groupings/{id}/add", require(&[EditMetadata], post(groupings::add)))
        .route("/api/groupings/{id}/remove", require(&[EditMetadata], post(groupings::remove)))
        .route("/api/groupings/{id}/members/add", require(&[EditMetadata], post(groupings::add_members)))
        .route("/api/groupings/{id}/members/remove", require(&[EditMetadata], post(groupings::remove_members)))
        .route("/api/groupings/{id}/sources", require(&[EditSources], post(sources::add_grouping_source)))
        .route(
            "/api/artists/{id}/collective",
            require(&[EditMetadata], get(groupings::collective_preview)).merge(require(&[EditMetadata, EditSources], post(groupings::turn_into_collective))),
        )
        // Uploads are streamed to disk, so a large body doesn't mean large memory use. The
        // limits (per request, per day) are `upload_limits`'s, not axum's.
        .route(
            "/api/upload",
            require(&[Upload], post(upload::upload).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit))),
        )
        .route(
            "/api/ingest",
            require(&[Upload], post(ingest::ingest).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit))),
        )
        // Chunked uploads: a file over several requests (see `uploads`). Each chunk goes
        // through the upload limits.
        .route("/api/uploads", require(&[Upload], post(uploads::start)))
        .route(
            "/api/uploads/{id}",
            require(&[Upload], get(uploads::progress).delete(uploads::drop_upload)).merge(require(
                &[Upload],
                put(uploads::append).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit)),
            )),
        )
        .route(
            "/api/tracks/{id}/file",
            require(
                &[ReplaceFile],
                post(upload::replace_file).layer(DefaultBodyLimit::disable()).layer(middleware::from_fn_with_state(state.clone(), upload_limits::limit)),
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
        .route_service("/s/{token}", ServeFile::new(state.web_dir.join("index.html")))
        // A link to a track, album or artist, for someone with an account: the page hands
        // it to the desktop app (or the web UI) itself.
        .route_service("/open/{kind}/{id}", ServeFile::new(state.web_dir.join("open.html")));

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
    /// A login with a wrong username or password: 401 too, but not "log in again".
    WrongLogin,
    /// Logged in, but not allowed: 403.
    Forbidden(String),
    /// Rate limited: 429, try again after this long.
    TooManyRequests(std::time::Duration),
    /// An upload over the size allowed per request: 413 (see `upload_limits`).
    TooLarge(String),
    /// The day's uploads are used up: 429, until midnight UTC (see `upload_limits`).
    UploadQuota { message: String, wait: std::time::Duration },
    Conflict(String),
    /// It existed, but is gone for good (e.g. a purged file): 410.
    Gone(String),
    /// A tool this needs isn't on the server (e.g. yt-dlp): 503.
    Unavailable(String),
    /// A track that was made into this extra: 404, with the extra's id (`extra_id`) to go
    /// to instead.
    BecameExtra(uuid::Uuid),
    Internal(anyhow::Error),
}

impl IntoResponse for AppError {
    /// The body: `{"code", "message"}`. `message` is for a person (what happened, and
    /// what to do), so clients show it as it is; `code` is for code (a stable name to act
    /// on: log in again on "unauthorized", offer a reload on "stale").
    fn into_response(self) -> Response {
        let (status, code, detail) = match self {
            AppError::TooManyRequests(wait) => {
                // Round up, so "try again in 0 seconds" never happens.
                let secs = wait.as_secs() + 1;
                let body = json!({
                    "code": "too_many_requests",
                    "message": format!("Too many tries. Wait {secs} seconds, then try again."),
                });
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    [(axum::http::header::RETRY_AFTER, secs.to_string())],
                    Json(body),
                )
                    .into_response();
            }
            AppError::BecameExtra(extra) => {
                let body = json!({
                    "code": "became_extra",
                    "message": "This track was turned into an extra; it's there now.",
                    "extra_id": extra,
                });
                return (StatusCode::NOT_FOUND, Json(body)).into_response();
            }
            AppError::UploadQuota { message, wait } => {
                let secs = wait.as_secs() + 1;
                let body = json!({ "code": "upload_quota", "message": message });
                return (StatusCode::TOO_MANY_REQUESTS, [(axum::http::header::RETRY_AFTER, secs.to_string())], Json(body)).into_response();
            }
            AppError::TooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, "too_large", msg),
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".to_string()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "invalid", msg),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "unauthorized", msg.to_string()),
            AppError::WrongLogin => (StatusCode::UNAUTHORIZED, "wrong_login", "wrong username or password".to_string()),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, "forbidden", msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, "conflict", msg),
            AppError::Gone(msg) => (StatusCode::GONE, "purged", msg),
            AppError::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, "unavailable", msg),
            AppError::Internal(err) => internal_parts(&err),
        };
        let message = human_message(code, &detail);
        (status, Json(json!({ "code": code, "message": message }))).into_response()
    }
}

/// What a track that couldn't be written in a many-track edit says: the same `code` and
/// `message` as the error would have as a response.
pub(crate) fn failure(err: anyhow::Error) -> db::track_edits::Failure {
    failure_of(&err)
}

/// `failure`, keeping the error.
pub(crate) fn failure_of(err: &anyhow::Error) -> db::track_edits::Failure {
    let (_, code, detail) = internal_parts(err);
    db::track_edits::Failure { code: code.to_string(), message: human_message(code, &detail) }
}

/// An error from below the API (the database, a refused edit…): its status, code and
/// detail.
fn internal_parts(err: &anyhow::Error) -> (StatusCode, &'static str, String) {
    if let Some(rejected) = err.downcast_ref::<Rejected>() {
        // Refused input (bad file, missing tags...): the client's problem.
        let (status, code) = match rejected {
            Rejected::TrackNotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            Rejected::AlreadyStored { .. } => (StatusCode::CONFLICT, "already_stored"),
            _ => (StatusCode::UNPROCESSABLE_ENTITY, "rejected"),
        };
        (status, code, rejected.to_string())
    } else if let Some(rejected) = err.downcast_ref::<EditRejected>() {
        let (status, code) = match rejected {
            EditRejected::Stale { .. } => (StatusCode::CONFLICT, "stale"),
            EditRejected::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid"),
            EditRejected::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        };
        (status, code, rejected.to_string())
    } else if let Some(invalid) = err.downcast_ref::<InvalidSource>() {
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_source", invalid.to_string())
    } else if let Some(msg) = foreign_key_violation(err) {
        (StatusCode::BAD_REQUEST, "invalid_reference", msg)
    } else if is_row_security_violation(err) {
        // A write that row-level security refused: normally the code checks
        // first and says why, so this is the safety net.
        (StatusCode::FORBIDDEN, "forbidden", "not allowed".to_string())
    } else {
        tracing::error!("internal error: {err:#}");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error".to_string())
    }
}

/// The sentence a person sees for an error: what happened and, where there's something to
/// do, what. Where the detail already says it in words (why an edit was refused, which
/// field is wrong), it's part of the sentence.
fn human_message(code: &str, detail: &str) -> String {
    let detail = sentence(detail);
    match code {
        "not_found" => "This isn't there (anymore), or you can't see it.".into(),
        "unauthorized" => "You're not logged in, or your session ended. Log in again.".into(),
        "wrong_login" => "Wrong username or password.".into(),
        "forbidden" if detail == "Not allowed." => "You're not allowed to do this.".into(),
        "forbidden" => format!("You're not allowed to do this: {detail}"),
        "stale" => format!("Someone else changed this while you were editing it. Reload it, then make your change again. ({detail})"),
        "purged" => format!("This file was purged: its audio is gone for good, only its record is left. ({detail})"),
        "already_stored" => format!("This file is already in the library. ({detail})"),
        "invalid_reference" => "Something this refers to doesn't exist (an unknown platform, status or role?).".into(),
        "internal" => "Something went wrong on the server. Try again; if it keeps happening, the server's log says what.".into(),
        // Invalid input, refused files, conflicts: the detail is the explanation.
        _ => detail,
    }
}

/// "only its owner can change it" → "Only its owner can change it."
fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?', ')']) {
        out.push('.');
    }
    out
}

/// A storage volume picked in a request: only admins choose (it's about the server's
/// disks), and it must be one that's configured.
pub(crate) fn check_volume(state: &AppState, user: &CurrentUser, volume: &str) -> Result<(), AppError> {
    if !user.has(crate::auth::Permission::Admin) {
        return Err(AppError::Forbidden("choosing a storage volume is for admins".to_string()));
    }
    if !state.store.has_volume(volume) {
        return Err(AppError::BadRequest(format!("no storage volume called \"{volume}\"")));
    }
    Ok(())
}

// Lets handlers use `?` on anything that converts into anyhow::Error (sqlx errors, io errors...).
impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        AppError::Internal(err.into())
    }
}

/// Postgres refused a write because of a row-level security policy (SQLSTATE 42501,
/// insufficient_privilege).
fn is_row_security_violation(err: &anyhow::Error) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|e| e.code().as_deref() == Some("42501"))
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

#[cfg(test)]
mod error_tests {
    use super::*;

    async fn body(err: AppError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 10_000).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn every_error_has_a_code_and_a_sentence() {
        let (status, b) = body(AppError::NotFound).await;
        assert_eq!((status, b["code"].as_str()), (StatusCode::NOT_FOUND, Some("not_found")));
        assert_eq!(b["message"], "This isn't there (anymore), or you can't see it.");
        assert!(b.get("error").is_none(), "only code and message");

        let (_, b) = body(AppError::Forbidden("only its owner can change a playlist".into())).await;
        assert_eq!(b["message"], "You're not allowed to do this: Only its owner can change a playlist.");

        let stale = EditRejected::Stale { latest: None };
        let (status, b) = body(AppError::Internal(stale.into())).await;
        assert_eq!((status, b["code"].as_str()), (StatusCode::CONFLICT, Some("stale")));
        assert!(b["message"].as_str().unwrap().starts_with("Someone else changed this"), "{b}");

        let (_, b) = body(AppError::Internal(anyhow::anyhow!("disk on fire"))).await;
        assert_eq!(b["code"], "internal");
        assert!(!b.to_string().contains("disk on fire"), "internal details stay in the log");

        let (_, b) = body(AppError::BadRequest("a rating is 0 to 10".into())).await;
        assert_eq!(b["message"], "A rating is 0 to 10.");
    }
}
