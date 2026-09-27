//! `GET /api/catalog`: the whole library as the current user sees it, for clients that
//! browse, sort and search locally (see `db::library`).
//!
//! The body can be large (tens of MB for a big library), so it's compressed (gzip or
//! zstd, as the client asks) and has an ETag: a client sends it back as `If-None-Match`
//! and gets `304 Not Modified` with no body when nothing changed.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::library::{ALBUM_COLUMNS, ANNOTATION_COLUMNS, ARTIST_COLUMNS, TRACK_COLUMNS};

/// Bumped when the format changes in a way old clients can't read.
const FORMAT: u32 = 1;

pub async fn catalog(State(state): State<AppState>, user: CurrentUser, headers: HeaderMap) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    let tracks = db::library::tracks_json(&mut tx).await?;
    let albums = db::library::albums_json(&mut tx).await?;
    let artists = db::library::artists_json(&mut tx).await?;
    let annotations = db::library::annotations_json(&mut tx).await?;
    let playlists = serde_json::to_string(&db::playlists::list(&mut tx, false).await?)?;
    drop(tx);

    // The parts are JSON already (built by Postgres), so they're put together as text
    // instead of being parsed and serialized again.
    let part = |columns: &[&str], rows: &str| -> Result<String, AppError> {
        Ok(format!(r#"{{"columns":{},"rows":{rows}}}"#, serde_json::to_string(columns)?))
    };
    let body = format!(
        r#"{{"format":{FORMAT},"tracks":{},"albums":{},"artists":{},"annotations":{},"playlists":{playlists}}}"#,
        part(TRACK_COLUMNS, &tracks)?,
        part(ALBUM_COLUMNS, &albums)?,
        part(ARTIST_COLUMNS, &artists)?,
        part(ANNOTATION_COLUMNS, &annotations)?,
    );

    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(body.as_bytes()))[..32]);
    let etag = HeaderValue::from_str(&etag).map_err(anyhow::Error::from)?;
    let unchanged = headers.get(header::IF_NONE_MATCH).is_some_and(|sent| sent == etag);
    // `private, no-cache`: a client may keep it, but asks again each time (cheap with the
    // ETag); shared caches (proxies) must not, it's one user's view.
    let cache = (header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    if unchanged {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag), cache]).into_response());
    }
    Ok((
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json")), (header::ETAG, etag), cache],
        body,
    )
        .into_response())
}
