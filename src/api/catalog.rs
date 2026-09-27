//! Artists, albums and search: viewing, creating and editing (through revisions).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::catalog::{AlbumDetail, AlbumHit, ArtistDetail, ArtistHit, TrackHit};
use crate::db::revisions::{self, Source, Written};
use crate::db::tracks::TrackSummary;
use crate::model::{AlbumData, Alias, ArtistData, Credit};

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    q: String,
    limit: Option<i64>,
}

fn search_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(20).clamp(1, 100)
}

fn search_query(q: &str) -> Result<&str, AppError> {
    let q = q.trim();
    if q.is_empty() {
        return Err(AppError::BadRequest("`q` can't be empty".to_string()));
    }
    Ok(q)
}

#[derive(Debug, Serialize)]
pub struct SearchResults {
    tracks: Vec<TrackHit>,
    artists: Vec<ArtistHit>,
    albums: Vec<AlbumHit>,
}

/// `GET /api/search?q=...`: tracks, artists (incl. aliases) and albums, fuzzy (typos are
/// fine) and ranked, up to `limit` (default 20) of each.
pub async fn search(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResults>, AppError> {
    let q = search_query(&params.q)?;
    let limit = search_limit(params.limit);
    let mut tx = state.db(&user).await?;
    Ok(Json(SearchResults {
        tracks: db::catalog::search_tracks(&mut tx, q, limit).await?,
        artists: db::catalog::search_artists(&mut tx, q, limit).await?,
        albums: db::catalog::search_albums(&mut tx, q, limit).await?,
    }))
}

/// `GET /api/vocab`: allowed statuses, credit roles, album kinds and relation kinds.
pub async fn vocab(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<db::catalog::Vocabularies>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::catalog::vocabularies(&mut tx).await?))
}

// ---- Artists ----

/// `GET /api/artists?q=...`: artists by name or alias (e.g. for picking one to credit).
pub async fn search_artists(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<ArtistHit>>, AppError> {
    let q = search_query(&params.q)?;
    let mut tx = state.db(&user).await?;
    let hits = db::catalog::search_artists(&mut tx, q, search_limit(params.limit)).await?;
    Ok(Json(hits))
}

/// `GET /api/artists/{id}`: with the first page of tracks, and history if allowed.
pub async fn get_artist(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<ArtistDetail>, AppError> {
    let mut tx = state.db(&user).await?;
    db::catalog::get_artist(&mut tx, id, user.has(Permission::ViewHistory))
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

#[derive(Debug, Deserialize)]
pub struct PageParams {
    /// Id of the last track on the previous page.
    before: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /api/artists/{id}/tracks?before=...`: more of an artist's tracks.
pub async fn artist_tracks(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(params): Query<PageParams>,
) -> Result<Json<Vec<TrackSummary>>, AppError> {
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let mut tx = state.db(&user).await?;
    let tracks = db::catalog::artist_tracks(&mut tx, id, params.before, limit).await?;
    Ok(Json(tracks))
}

#[derive(Debug, Serialize)]
pub struct Created {
    id: Uuid,
}

/// `POST /api/artists`, JSON `{"name": "...", "disambiguation": "...", "aliases": [...]}`.
/// Also for a second artist with an existing name (tell them apart with `disambiguation`).
pub async fn create_artist(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(data): Json<ArtistData>,
) -> Result<Json<Created>, AppError> {
    let mut tx = state.db(&user).await?;
    let id = revisions::create_artist(&mut tx, &data, Source::Edit, "created").await?;
    tx.commit().await?;
    Ok(Json(Created { id }))
}

#[derive(Debug, Deserialize)]
pub struct EditArtist {
    base_revision_id: Uuid,
    name: String,
    #[serde(default)]
    disambiguation: String,
    /// The full list: `{"id": ..., "name": ...}` for existing ones (renaming keeps the
    /// id), `{"name": ...}` for new ones. Left out = hidden.
    #[serde(default)]
    aliases: Vec<Alias>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/artists/{id}`: new name, disambiguation and aliases (the full list).
pub async fn edit_artist(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<EditArtist>,
) -> Result<Json<Written>, AppError> {
    let data = ArtistData {
        name: body.name,
        disambiguation: body.disambiguation,
        aliases: body.aliases,
    };
    let mut tx = state.db(&user).await?;
    let written = revisions::write_artist(
        &mut tx,
        id,
        Some(body.base_revision_id),
        &data,
        Source::Edit,
        &body.reason,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Restore {
    reason: String,
}

/// `POST /api/artist-revisions/{id}/restore`
pub async fn restore_artist(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(revision_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<Written>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let written = revisions::restore_artist_revision(&mut tx, revision_id, &reason).await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Deserialize)]
pub struct MergeArtist {
    /// The artist to merge this one into (the one that stays).
    into: Uuid,
    /// Keep this artist's name as an alias of the other, so its tracks keep showing it.
    #[serde(default)]
    keep_name_as_alias: bool,
    /// Flag every track whose credits changed for review. On unless turned off: a safety
    /// net, e.g. for "A feat. B" merged into A (B may need crediting again).
    #[serde(default = "default_true")]
    flag_for_review: bool,
    #[serde(default)]
    reason: String,
}

fn default_true() -> bool {
    true
}

/// `GET /api/artists/{id}/aliases/{alias_id}/uses`: the tracks and albums crediting the
/// alias (see `revisions::AliasUses`). An alias still in use can't be removed; this says
/// which credits to change first.
pub async fn alias_uses(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((artist_id, alias_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<revisions::AliasUses>, AppError> {
    let mut tx = state.db(&user).await?;
    let of_artist = db::catalog::is_alias_of(&mut tx, artist_id, alias_id).await?;
    if !of_artist {
        return Err(AppError::NotFound);
    }
    Ok(Json(revisions::alias_uses_seen(&mut tx, alias_id).await?))
}

#[derive(Debug, Deserialize)]
pub struct BrowseParams {
    before: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /api/library/albums?before=&limit=`: albums, newest first (up to 500 a page), with
/// their album artists and the cover to show. Only albums the user can see.
pub async fn list_albums(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<BrowseParams>,
) -> Result<Json<Vec<db::catalog::AlbumListItem>>, AppError> {
    let mut tx = state.db(&user).await?;
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(db::catalog::list_albums(&mut tx, params.before, limit).await?))
}

/// `POST /api/artists/{id}/merge`, JSON `{"into": "...", "keep_name_as_alias": true}`:
/// this artist is the same as `into`. Its credits, aliases and sources move there (each
/// changed track/album gets a revision); this artist then redirects to `into`.
pub async fn merge_artist(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MergeArtist>,
) -> Result<Json<revisions::Merged>, AppError> {
    let mut tx = state.db(&user).await?;
    let merged = revisions::merge_artist(
        &mut tx,
        id,
        body.into,
        body.keep_name_as_alias,
        body.flag_for_review,
        &body.reason,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(merged))
}

// ---- Albums ----

/// `GET /api/albums?q=...`
pub async fn search_albums(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<AlbumHit>>, AppError> {
    let q = search_query(&params.q)?;
    let mut tx = state.db(&user).await?;
    let hits = db::catalog::search_albums(&mut tx, q, search_limit(params.limit)).await?;
    Ok(Json(hits))
}

/// `GET /api/albums/{id}`: with credits, tracks, and history if allowed. A merged album
/// redirects (308) to the album it was merged into.
pub async fn get_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let mut tx = state.db(&user).await?;
    if let Some(target) = db::catalog::album_merged_into(&mut tx, id).await? {
        return Ok(Redirect::permanent(&format!("/api/albums/{target}")).into_response());
    }
    let album: AlbumDetail = db::catalog::get_album(&mut tx, id, user.has(Permission::ViewHistory))
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(album).into_response())
}

/// `POST /api/albums`, JSON `{"title": "...", "kind": "ep", "release_date": "...",
/// "credits": [{"artist_id": "...", "role": "primary"}]}`.
pub async fn create_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(data): Json<AlbumData>,
) -> Result<Json<Created>, AppError> {
    let mut tx = state.db(&user).await?;
    let id = revisions::create_album(&mut tx, &data, Source::Edit, "created").await?;
    tx.commit().await?;
    Ok(Json(Created { id }))
}

#[derive(Debug, Deserialize)]
pub struct EditAlbum {
    base_revision_id: Uuid,
    title: String,
    kind: String,
    release_date: Option<String>,
    #[serde(default)]
    credits: Vec<Credit>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/albums/{id}`: new title, kind, release date and credits.
pub async fn edit_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<EditAlbum>,
) -> Result<Json<Written>, AppError> {
    let data = AlbumData {
        title: body.title,
        kind: body.kind,
        release_date: body.release_date,
        credits: body.credits,
    };
    let mut tx = state.db(&user).await?;
    let written = revisions::write_album(
        &mut tx,
        id,
        Some(body.base_revision_id),
        &data,
        Source::Edit,
        &body.reason,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(written))
}

#[derive(Debug, Deserialize)]
pub struct MergeAlbum {
    /// The album to merge this one into (the one that stays).
    into: Uuid,
    #[serde(default)]
    reason: String,
}

/// `POST /api/albums/{id}/merge`, JSON `{"into": "..."}`: this album is the same as
/// `into`. Its tracks (each with a revision), covers, sources and shares move there; this
/// album then redirects to `into`.
pub async fn merge_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MergeAlbum>,
) -> Result<Json<db::album_merges::MergedAlbum>, AppError> {
    let mut tx = state.db(&user).await?;
    let merged = db::album_merges::merge_album(&mut tx, id, body.into, &body.reason).await?;
    tx.commit().await?;
    Ok(Json(merged))
}

/// `GET /api/album-merges/{id}/undo`: what undoing the merge would do, item by item
/// (nothing is changed).
pub async fn preview_album_merge_undo(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(merge_id): Path<Uuid>,
) -> Result<Json<db::album_merges::UndoReport>, AppError> {
    let mut tx = state.db(&user).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, merge_id, "", false).await?;
    // Dropped without commit: rolls back (the preview only read).
    Ok(Json(report))
}

/// `POST /api/album-merges/{id}/undo`, JSON `{"reason": "..."}` (optional): undoes the
/// merge, exactly as the preview said. Returns the same report, with `done: true`.
pub async fn undo_album_merge(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(merge_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<db::album_merges::UndoReport>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, merge_id, &reason, true).await?;
    tx.commit().await?;
    Ok(Json(report))
}

/// `POST /api/album-revisions/{id}/restore`
pub async fn restore_album(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(revision_id): Path<Uuid>,
    body: Option<Json<Restore>>,
) -> Result<Json<Written>, AppError> {
    let reason = body.map(|Json(b)| b.reason).unwrap_or_default();
    let mut tx = state.db(&user).await?;
    let written = revisions::restore_album_revision(&mut tx, revision_id, &reason).await?;
    tx.commit().await?;
    Ok(Json(written))
}
