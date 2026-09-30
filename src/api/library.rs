//! `GET /api/catalog`: the whole library as the current user sees it, for clients that
//! browse, sort and search locally (see `db::library`).
//!
//! The body can be large (tens of MB for a big library), so it's compressed (gzip or
//! zstd, as the client asks) and has an ETag: a client sends it back as `If-None-Match`
//! and gets `304 Not Modified` with no body when nothing changed.
//!
//! Every catalog has a `version`. With `?since=<version>`, only what changed since that
//! copy comes back (a delta, `"delta": true`): the changed tracks, albums and artists as
//! rows (those the user can see), the user's changed annotations, all playlists, and
//! `removed` (merged ones: they only redirect now; annotations that say nothing anymore).
//! Nothing changed: 304. When a delta can't be given (who may see what changed, or the
//! copy is older than the change log keeps), the whole catalog comes back instead
//! (`"delta": false`).
//!
//! Both have `issues`: each track's open review flags (see `db::library::ISSUE_COLUMNS`),
//! so a client can list what needs a look without asking for every flag. (A part of its
//! own rather than a track column: clients that don't know it just skip it.)

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::library::{ALBUM_COLUMNS, ANNOTATION_COLUMNS, ARTIST_COLUMNS, EXTRA_COLUMNS, GROUPING_COLUMNS, ISSUE_COLUMNS, TRACK_COLUMNS};

/// Bumped when the format changes in a way old clients can't read.
const FORMAT: u32 = 7;

#[derive(Debug, Deserialize)]
pub struct CatalogParams {
    /// The version of the client's copy (a catalog's `version`).
    since: Option<String>,
}

pub async fn catalog(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<CatalogParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    // Before reading anything: whatever changes after this is in the next delta.
    let version = db::library::version(&mut tx).await?;

    // A delta, when there's a copy to bring up to date and it can be done that way.
    if let Some(since) = params.since {
        let changes = db::library::changes_since(&mut tx, &since).await?;
        if changes.is_empty() {
            return Ok((StatusCode::NOT_MODIFIED, [no_cache()]).into_response());
        }
        if !changes.everything {
            let tracks = db::library::tracks_json(&mut tx, Some(&changes.tracks)).await?;
            let albums = db::library::albums_json(&mut tx, Some(&changes.albums)).await?;
            let artists = db::library::artists_json(&mut tx, Some(&changes.artists)).await?;
            let annotations = db::library::annotations_json(&mut tx, Some(&changes.annotations)).await?;
            let issues = db::library::issues_json(&mut tx, Some(&changes.tracks)).await?;
            let (mut gone_tracks, mut gone_albums, gone_artists) =
                db::library::merged(&mut tx, &changes.tracks, &changes.albums, &changes.artists).await?;
            // Turned into extras (and albums they were the last track of): gone for all.
            gone_tracks.extend(&changes.gone_tracks);
            gone_albums.extend(&changes.gone_albums);
            let playlists = serde_json::to_string(&db::playlists::list(&mut tx, false).await?)?;
            let groupings = db::library::groupings_json(&mut tx).await?;
            let extras = db::library::extras_json(&mut tx, Some(&changes.extras)).await?;
            drop(tx);
            // The user's changed annotations that say nothing anymore (unstarred, unrated,
            // never played) aren't rows: the client drops them.
            let kept: Vec<Vec<serde_json::Value>> = serde_json::from_str(&annotations)?;
            let kept: std::collections::HashSet<String> = kept.iter().filter_map(|r| r.get(1)?.as_str().map(str::to_string)).collect();
            let cleared: Vec<uuid::Uuid> = changes.annotations.iter().filter(|id| !kept.contains(&id.to_string())).copied().collect();
            let removed = serde_json::json!({
                "tracks": gone_tracks, "albums": gone_albums, "artists": gone_artists, "annotations": cleared,
            });
            let body = format!(
                r#"{{"format":{FORMAT},"delta":true,"version":"{version}","tracks":{},"albums":{},"artists":{},"annotations":{},"issues":{},"groupings":{},"extras":{},"playlists":{playlists},"removed":{removed}}}"#,
                part(TRACK_COLUMNS, &tracks)?,
                part(ALBUM_COLUMNS, &albums)?,
                part(ARTIST_COLUMNS, &artists)?,
                part(ANNOTATION_COLUMNS, &annotations)?,
                part(ISSUE_COLUMNS, &issues)?,
                part(GROUPING_COLUMNS, &groupings)?,
                part(EXTRA_COLUMNS, &extras)?,
            );
            return Ok(([(header::CONTENT_TYPE, HeaderValue::from_static("application/json")), no_cache()], body).into_response());
        }
    }

    let tracks = db::library::tracks_json(&mut tx, None).await?;
    let albums = db::library::albums_json(&mut tx, None).await?;
    let artists = db::library::artists_json(&mut tx, None).await?;
    let annotations = db::library::annotations_json(&mut tx, None).await?;
    let issues = db::library::issues_json(&mut tx, None).await?;
    let playlists = serde_json::to_string(&db::playlists::list(&mut tx, false).await?)?;
    let groupings = db::library::groupings_json(&mut tx).await?;
    let extras = db::library::extras_json(&mut tx, None).await?;
    drop(tx);

    // The parts are JSON already (built by Postgres), so they're put together as text
    // instead of being parsed and serialized again.
    let body = format!(
        r#"{{"format":{FORMAT},"delta":false,"version":"{version}","tracks":{},"albums":{},"artists":{},"annotations":{},"issues":{},"groupings":{},"extras":{},"playlists":{playlists}}}"#,
        part(TRACK_COLUMNS, &tracks)?,
        part(ALBUM_COLUMNS, &albums)?,
        part(ARTIST_COLUMNS, &artists)?,
        part(ANNOTATION_COLUMNS, &annotations)?,
        part(ISSUE_COLUMNS, &issues)?,
        part(GROUPING_COLUMNS, &groupings)?,
        part(EXTRA_COLUMNS, &extras)?,
    );

    // The ETag leaves out the version (it moves with any change anywhere, even ones this
    // user can't see): the same content is the same tag.
    let content = body.replacen(&format!(r#""version":"{version}","#), "", 1);
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(content.as_bytes()))[..32]);
    let etag = HeaderValue::from_str(&etag).map_err(anyhow::Error::from)?;
    let unchanged = headers.get(header::IF_NONE_MATCH).is_some_and(|sent| sent == etag);
    if unchanged {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag), no_cache()]).into_response());
    }
    Ok(([(header::CONTENT_TYPE, HeaderValue::from_static("application/json")), (header::ETAG, etag), no_cache()], body).into_response())
}

/// A part: its columns, and its rows (JSON already).
fn part(columns: &[&str], rows: &str) -> Result<String, AppError> {
    Ok(format!(r#"{{"columns":{},"rows":{rows}}}"#, serde_json::to_string(columns)?))
}

/// `private, no-cache`: a client may keep it, but asks again each time (cheap with the
/// ETag or `since`); shared caches (proxies) must not, it's one user's view.
fn no_cache() -> (header::HeaderName, HeaderValue) {
    (header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"))
}
