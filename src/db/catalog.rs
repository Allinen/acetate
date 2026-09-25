//! Read queries for artists and albums, and search across the library.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::sources::Source;
use super::tracks::TrackSummary;

/// A revision of an artist or album.
#[derive(Debug, Serialize)]
pub struct EntityRevision {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub author: Option<String>,
    pub source: String,
    pub reason: String,
    pub data: serde_json::Value,
}

// ---- Artists ----

#[derive(Debug, Serialize)]
pub struct AliasInfo {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct ArtistDetail {
    pub id: Uuid,
    /// The current user's plays, rating and star.
    pub annotation: super::annotations::Annotation,
    pub name: String,
    pub disambiguation: String,
    /// Listed aliases.
    pub aliases: Vec<AliasInfo>,
    /// If this artist was merged into another: that one. (The UI goes there instead.)
    pub merged_into: Option<Uuid>,
    /// The latest artist revision: the base for editing.
    pub revision_id: Option<Uuid>,
    pub sources: Vec<Source>,
    /// The first page of the artist's tracks (more via `artist_tracks`).
    pub tracks: Vec<TrackSummary>,
    /// Only included for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<EntityRevision>>,
}

pub async fn get_artist(
    conn: &mut PgConnection,
    id: Uuid,
    with_history: bool,
) -> sqlx::Result<Option<ArtistDetail>> {
    let Some(row) = sqlx::query!(
        r#"
        SELECT a.id, a.name, a.disambiguation, a.merged_into,
               (SELECT r.id FROM artist_revisions r
                WHERE r.artist_id = a.id ORDER BY r.id DESC LIMIT 1) AS revision_id
        FROM artists a
        WHERE a.id = $1
        "#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };

    let revisions = if with_history {
        Some(
            sqlx::query_as!(
                EntityRevision,
                r#"
                SELECT r.id, r.created_at, u.username AS "author?", r.source, r.reason, r.data
                FROM artist_revisions r
                LEFT JOIN users u ON u.id = r.author_id
                WHERE r.artist_id = $1
                ORDER BY r.id DESC
                "#,
                id
            )
            .fetch_all(&mut *conn)
            .await?,
        )
    } else {
        None
    };

    let aliases = sqlx::query_as!(
        AliasInfo,
        r#"
        SELECT id, alias AS name FROM artist_aliases
        WHERE artist_id = $1 AND listed
        ORDER BY lower(alias), alias
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(Some(ArtistDetail {
        annotation: super::annotations::get(conn, super::annotations::Item::Artist(row.id)).await?,
        id: row.id,
        name: row.name,
        disambiguation: row.disambiguation,
        aliases,
        merged_into: row.merged_into,
        revision_id: row.revision_id,
        sources: super::sources::list_for(conn, id).await?,
        tracks: artist_tracks(conn, id, None, 100).await?,
        revisions,
    }))
}

/// Tracks crediting an artist (in the primary variant), newest first, a page at a time.
pub async fn artist_tracks(
    conn: &mut PgConnection,
    artist_id: Uuid,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<TrackSummary>> {
    sqlx::query_as!(
        TrackSummary,
        r#"
        SELECT t.id, t.title, t.artist_display, t.status, t.release_date,
               f.duration_ms, f.codec
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id IN (SELECT track_id FROM track_credits WHERE artist_id = $1)
          AND t.merged_into IS NULL
          AND t.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY t.id DESC
        LIMIT $3
        "#,
        artist_id,
        before,
        limit
    )
    .fetch_all(conn)
    .await
}

// ---- Albums ----

#[derive(Debug, Serialize)]
pub struct AlbumDetail {
    pub id: Uuid,
    /// The current user's plays, rating and star.
    pub annotation: super::annotations::Annotation,
    pub title: String,
    pub kind: String,
    pub release_date: Option<String>,
    pub credits: Vec<AlbumCredit>,
    /// The latest album revision: the base for editing.
    pub revision_id: Option<Uuid>,
    /// The album's own covers, main one first.
    pub covers: Vec<super::tracks::Cover>,
    /// The cover to show for the album: its own main cover, else the main cover of the
    /// first track (in track order) that has one.
    pub display_cover_id: Option<Uuid>,
    pub sources: Vec<Source>,
    pub tracks: Vec<AlbumTrack>,
    /// Only included for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<EntityRevision>>,
}

#[derive(Debug, Serialize)]
pub struct AlbumCredit {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub role: String,
    pub alias_id: Option<Uuid>,
    pub alias: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AlbumTrack {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub disc_number: Option<i16>,
    pub track_number: Option<i16>,
    pub duration_ms: Option<i64>,
    /// The track's main cover, if it has one.
    pub cover_id: Option<Uuid>,
}

pub async fn get_album(
    conn: &mut PgConnection,
    id: Uuid,
    with_history: bool,
) -> sqlx::Result<Option<AlbumDetail>> {
    let Some(row) = sqlx::query!(
        r#"
        SELECT a.id, a.title, a.kind, a.release_date,
               (SELECT r.id FROM album_revisions r
                WHERE r.album_id = a.id ORDER BY r.id DESC LIMIT 1) AS revision_id
        FROM albums a
        WHERE a.id = $1
        "#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };

    let credits = sqlx::query_as!(
        AlbumCredit,
        r#"
        SELECT c.artist_id, a.name AS artist_name, c.role, c.alias_id, al.alias AS "alias?"
        FROM album_credits c
        JOIN artists a ON a.id = c.artist_id
        LEFT JOIN artist_aliases al ON al.id = c.alias_id
        WHERE c.album_id = $1
        ORDER BY c.position, a.name
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    let tracks = sqlx::query_as!(
        AlbumTrack,
        r#"
        SELECT t.id, t.title, t.artist_display, at.disc_number, at.track_number, f.duration_ms,
               (SELECT c.id FROM covers c WHERE c.track_id = t.id AND c.trashed_at IS NULL
                ORDER BY c.position, c.id LIMIT 1) AS cover_id
        FROM album_tracks at
        JOIN tracks t ON t.id = at.track_id
        JOIN files f ON f.id = t.file_id
        WHERE at.album_id = $1
        ORDER BY at.disc_number NULLS FIRST, at.track_number NULLS LAST, t.title
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    let revisions = if with_history {
        Some(
            sqlx::query_as!(
                EntityRevision,
                r#"
                SELECT r.id, r.created_at, u.username AS "author?", r.source, r.reason, r.data
                FROM album_revisions r
                LEFT JOIN users u ON u.id = r.author_id
                WHERE r.album_id = $1
                ORDER BY r.id DESC
                "#,
                id
            )
            .fetch_all(&mut *conn)
            .await?,
        )
    } else {
        None
    };

    let covers = super::tracks::covers_of(conn, id).await?;
    Ok(Some(AlbumDetail {
        annotation: super::annotations::get(conn, super::annotations::Item::Album(row.id)).await?,
        id: row.id,
        title: row.title,
        kind: row.kind,
        release_date: row.release_date,
        credits,
        revision_id: row.revision_id,
        display_cover_id: covers
            .first()
            .map(|c| c.id)
            .or_else(|| tracks.iter().find_map(|t| t.cover_id)),
        covers,
        sources: super::sources::list_for(conn, id).await?,
        tracks,
        revisions,
    }))
}

// ---- Search ----
//
// Fuzzy: `word_similarity` finds the query as (part of) a title or name even when
// misspelled ("nortbound" -> "Northbound"). `$1 <% column` is its indexed form (the
// trigram GIN indexes), true above a similarity threshold (0.6 by default). Substring
// matches (`ILIKE`, also indexed) are included too, so short or partial queries like
// "mir" work for pickers. Results are ranked by similarity, substring matches first.

#[derive(Debug, Serialize)]
pub struct TrackHit {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct ArtistHit {
    pub id: Uuid,
    pub name: String,
    pub disambiguation: String,
    /// The alias that matched, if it was an alias rather than the name.
    pub matched_alias: Option<String>,
    pub score: f32,
}

#[derive(Debug, Serialize)]
pub struct AlbumHit {
    pub id: Uuid,
    pub title: String,
    pub kind: String,
    pub score: f32,
}

/// `%query%` for ILIKE, with LIKE's special characters escaped so they match literally.
fn like_pattern(query: &str) -> String {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

pub async fn search_tracks(conn: &mut PgConnection, query: &str, limit: i64) -> sqlx::Result<Vec<TrackHit>> {
    sqlx::query_as!(
        TrackHit,
        r#"
        SELECT t.id, t.title, t.artist_display,
               GREATEST(
                   CASE WHEN t.title ILIKE $2 OR t.artist_display ILIKE $2 THEN 1 ELSE 0 END,
                   word_similarity($1, t.title),
                   word_similarity($1, t.artist_display)
               )::real AS "score!"
        FROM tracks t
        WHERE ($1 <% t.title OR $1 <% t.artist_display
               OR t.title ILIKE $2 OR t.artist_display ILIKE $2)
          AND t.merged_into IS NULL
        ORDER BY 4 DESC, t.id DESC
        LIMIT $3
        "#,
        query,
        like_pattern(query),
        limit
    )
    .fetch_all(conn)
    .await
}

pub async fn search_artists(conn: &mut PgConnection, query: &str, limit: i64) -> sqlx::Result<Vec<ArtistHit>> {
    sqlx::query_as!(
        ArtistHit,
        r#"
        SELECT a.id, a.name, a.disambiguation, best.alias AS matched_alias, best.score AS "score!"
        FROM (
            SELECT DISTINCT ON (id) id, alias, score
            FROM (
                SELECT a.id, NULL::text AS alias,
                       GREATEST(CASE WHEN a.name ILIKE $2 THEN 1 ELSE 0 END,
                                word_similarity($1, a.name))::real AS score
                FROM artists a
                WHERE ($1 <% a.name OR a.name ILIKE $2) AND a.merged_into IS NULL
                UNION ALL
                SELECT al.artist_id, al.alias,
                       GREATEST(CASE WHEN al.alias ILIKE $2 THEN 1 ELSE 0 END,
                                word_similarity($1, al.alias))::real
                FROM artist_aliases al
                WHERE ($1 <% al.alias OR al.alias ILIKE $2) AND al.listed
            ) matches
            -- Per artist, its best match; on a tie the name beats an alias.
            ORDER BY id, score DESC, alias NULLS FIRST
        ) best
        JOIN artists a ON a.id = best.id
        ORDER BY best.score DESC, a.name
        LIMIT $3
        "#,
        query,
        like_pattern(query),
        limit
    )
    .fetch_all(conn)
    .await
}

pub async fn search_albums(conn: &mut PgConnection, query: &str, limit: i64) -> sqlx::Result<Vec<AlbumHit>> {
    sqlx::query_as!(
        AlbumHit,
        r#"
        SELECT al.id, al.title, al.kind,
               GREATEST(CASE WHEN al.title ILIKE $2 THEN 1 ELSE 0 END,
                        word_similarity($1, al.title))::real AS "score!"
        FROM albums al
        WHERE $1 <% al.title OR al.title ILIKE $2
        ORDER BY 4 DESC, al.title
        LIMIT $3
        "#,
        query,
        like_pattern(query),
        limit
    )
    .fetch_all(conn)
    .await
}

// ---- Vocabularies ----

#[derive(Debug, Serialize)]
pub struct RelationKind {
    pub kind: String,
    pub inverse_kind: String,
    pub template: String,
}

#[derive(Debug, Serialize)]
pub struct Vocabularies {
    pub statuses: Vec<String>,
    pub credit_roles: Vec<String>,
    pub album_kinds: Vec<String>,
    pub relation_kinds: Vec<RelationKind>,
    pub permissions: Vec<PermissionInfo>,
}

#[derive(Debug, Serialize)]
pub struct PermissionInfo {
    pub permission: String,
    pub description: String,
}

/// The allowed values of the lookup tables, for clients' forms.
pub async fn vocabularies(conn: &mut PgConnection) -> sqlx::Result<Vocabularies> {
    Ok(Vocabularies {
        statuses: sqlx::query_scalar!("SELECT status FROM track_statuses ORDER BY status")
            .fetch_all(&mut *conn)
            .await?,
        credit_roles: sqlx::query_scalar!("SELECT role FROM credit_roles ORDER BY role")
            .fetch_all(&mut *conn)
            .await?,
        album_kinds: sqlx::query_scalar!("SELECT kind FROM album_kinds ORDER BY kind")
            .fetch_all(&mut *conn)
            .await?,
        relation_kinds: sqlx::query_as!(
            RelationKind,
            "SELECT kind, inverse_kind, template FROM relation_kinds ORDER BY kind"
        )
        .fetch_all(&mut *conn)
        .await?,
        permissions: sqlx::query_as!(
            PermissionInfo,
            "SELECT permission, description FROM permissions ORDER BY permission"
        )
        .fetch_all(&mut *conn)
        .await?,
    })
}
