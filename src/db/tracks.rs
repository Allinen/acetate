//! Read queries for tracks.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct TrackSummary {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub status: String,
    pub release_date: Option<String>,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
}

/// One page of tracks, newest first.
///
/// Uses keyset pagination: pass the last id of the previous page as `before`.
/// Unlike OFFSET, this stays fast at any depth because it's a single index range scan.
/// (UUIDv7 ids are time-ordered, so ordering by id means ordering by creation time.)
pub async fn list_tracks(
    pool: &PgPool,
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
        WHERE t.id < COALESCE($1, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY t.id DESC
        LIMIT $2
        "#,
        before,
        limit,
    )
    .fetch_all(pool)
    .await
}

/// Like `list_tracks`, filtered to tracks whose title or artist contains `query`
/// (case-insensitive). The trigram indexes make `ILIKE '%...%'` fast.
pub async fn search_tracks(
    pool: &PgPool,
    query: &str,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<TrackSummary>> {
    // Escape LIKE's special characters so they match literally.
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    sqlx::query_as!(
        TrackSummary,
        r#"
        SELECT t.id, t.title, t.artist_display, t.status, t.release_date,
               f.duration_ms, f.codec
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE (t.title ILIKE $1 OR t.artist_display ILIKE $1)
          AND t.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY t.id DESC
        LIMIT $3
        "#,
        pattern,
        before,
        limit,
    )
    .fetch_all(pool)
    .await
}

#[derive(Debug, Serialize)]
pub struct TrackDetail {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub status: String,
    pub is_restricted: bool,
    pub release_date: Option<String>,
    pub release_year: Option<i16>,
    pub genres: Vec<String>,
    pub primary_variant_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub file: FileInfo,
    pub file_history: Vec<FileChange>,
    pub sources: Vec<super::sources::Source>,
    pub credits: Vec<Credit>,
    pub albums: Vec<AlbumEntry>,
    pub variants: Vec<Variant>,
    pub revisions: Vec<Revision>,
    pub lyrics: Vec<Lyrics>,
    pub covers: Vec<Cover>,
}

#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub id: Uuid,
    pub sha256: String,
    pub original_filename: String,
    pub uploaded_filename: String,
    pub size_bytes: i64,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub bit_depth: Option<i16>,
    pub duration_ms: Option<i64>,
    /// Where this file came from, if known.
    pub origin: Option<super::sources::Origin>,
    pub uploaded_at: chrono::DateTime<chrono::Utc>,
    pub embedded_tags: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct FileChange {
    pub replaced_at: chrono::DateTime<chrono::Utc>,
    pub reason: String,
    pub old_file_id: Uuid,
    pub old_original_filename: String,
    pub old_codec: Option<String>,
    pub old_bitrate: Option<i32>,
    pub old_origin: Option<super::sources::Origin>,
    pub new_file_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct Credit {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub role: String,
    pub credited_as: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AlbumEntry {
    pub album_id: Uuid,
    pub title: String,
    pub kind: String,
    pub disc_number: Option<i16>,
    pub track_number: Option<i16>,
}

#[derive(Debug, Serialize)]
pub struct Variant {
    pub id: Uuid,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct Revision {
    pub id: Uuid,
    pub variant_id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub author_id: Option<Uuid>,
    pub source: String,
    pub reason: String,
    pub is_canon: bool,
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct Lyrics {
    pub id: Uuid,
    pub is_official: bool,
    pub is_synced: bool,
    pub source: String,
    pub body: String,
}

#[derive(Debug, Serialize)]
pub struct Cover {
    pub id: Uuid,
    pub blob_sha256: String,
    pub mime: Option<String>,
    pub position: i16,
    pub source: String,
}

/// Everything about one track. Returns None if it doesn't exist.
pub async fn get_track(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<TrackDetail>> {
    let Some(row) = sqlx::query!(
        r#"
        SELECT t.id, t.title, t.artist_display, t.status, t.is_restricted, t.release_date,
               t.release_year, t.genres, t.primary_variant_id, t.created_at,
               f.id AS file_id, f.sha256, f.original_filename, f.uploaded_filename, f.size_bytes, f.container,
               f.codec, f.bitrate, f.sample_rate, f.channels, f.bit_depth, f.duration_ms,
               f.uploaded_at, f.embedded_tags
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id = $1
        "#,
        id
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let history_rows = sqlx::query!(
        r#"
        SELECT h.replaced_at, h.reason, h.old_file_id, f.original_filename AS old_original_filename,
               f.codec AS old_codec, f.bitrate AS old_bitrate, h.new_file_id
        FROM track_file_history h
        JOIN files f ON f.id = h.old_file_id
        WHERE h.track_id = $1
        ORDER BY h.id DESC
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    // Origins of the current file and all previous files, in one query.
    let mut file_ids: Vec<Uuid> = history_rows.iter().map(|h| h.old_file_id).collect();
    file_ids.push(row.file_id);
    let mut origins = super::sources::origins_of_files(pool, &file_ids).await?;

    let file_history = history_rows
        .into_iter()
        .map(|h| FileChange {
            old_origin: origins.remove(&h.old_file_id),
            replaced_at: h.replaced_at,
            reason: h.reason,
            old_file_id: h.old_file_id,
            old_original_filename: h.old_original_filename,
            old_codec: h.old_codec,
            old_bitrate: h.old_bitrate,
            new_file_id: h.new_file_id,
        })
        .collect();

    let sources = super::sources::list_for(pool, id).await?;

    let credits = sqlx::query_as!(
        Credit,
        r#"
        SELECT c.artist_id, a.name AS artist_name, c.role, c.credited_as
        FROM track_credits c
        JOIN artists a ON a.id = c.artist_id
        WHERE c.track_id = $1
        ORDER BY c.position, a.name
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    let albums = sqlx::query_as!(
        AlbumEntry,
        r#"
        SELECT al.id AS album_id, al.title, al.kind, at.disc_number, at.track_number
        FROM album_tracks at
        JOIN albums al ON al.id = at.album_id
        WHERE at.track_id = $1
        ORDER BY al.release_date NULLS LAST, al.title
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    let variants = sqlx::query_as!(
        Variant,
        "SELECT id, label FROM track_variants WHERE track_id = $1 ORDER BY id",
        id
    )
    .fetch_all(pool)
    .await?;

    let revisions = sqlx::query_as!(
        Revision,
        r#"
        SELECT id, variant_id, created_at, author_id, source, reason, is_canon, data
        FROM track_revisions
        WHERE track_id = $1
        ORDER BY id DESC
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    let lyrics = sqlx::query_as!(
        Lyrics,
        r#"
        SELECT id, is_official, is_synced, source, body
        FROM lyrics WHERE track_id = $1
        ORDER BY is_official DESC, id
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    let covers = sqlx::query_as!(
        Cover,
        r#"
        SELECT id, blob_sha256, mime, position, source
        FROM covers WHERE track_id = $1
        ORDER BY position, id
        "#,
        id
    )
    .fetch_all(pool)
    .await?;

    Ok(Some(TrackDetail {
        id: row.id,
        title: row.title,
        artist_display: row.artist_display,
        status: row.status,
        is_restricted: row.is_restricted,
        release_date: row.release_date,
        release_year: row.release_year,
        genres: row.genres,
        primary_variant_id: row.primary_variant_id,
        created_at: row.created_at,
        file: FileInfo {
            id: row.file_id,
            sha256: row.sha256,
            original_filename: row.original_filename,
            uploaded_filename: row.uploaded_filename,
            size_bytes: row.size_bytes,
            container: row.container,
            codec: row.codec,
            bitrate: row.bitrate,
            sample_rate: row.sample_rate,
            channels: row.channels,
            bit_depth: row.bit_depth,
            duration_ms: row.duration_ms,
            origin: origins.remove(&row.file_id),
            uploaded_at: row.uploaded_at,
            embedded_tags: row.embedded_tags,
        },
        file_history,
        sources,
        credits,
        albums,
        variants,
        revisions,
        lyrics,
        covers,
    }))
}
