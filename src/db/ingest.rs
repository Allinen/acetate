//! Database writes used by the ingest pipeline.
//!
//! Every function takes `&mut PgConnection` so it can run inside a transaction:
//! call them with `&mut *tx` (the `*` turns the `Transaction` into the connection
//! it wraps, and `&mut` borrows that).

use sqlx::PgConnection;
use uuid::Uuid;

use crate::model::TrackMetadata;

pub struct StoredFile {
    /// The track currently using this file, or the track it used to belong to
    /// before being replaced by another file.
    pub track_id: Option<Uuid>,
}

/// Looks up a file by hash, including files that were replaced (e.g. by an upgrade).
pub async fn find_file_by_sha256(
    conn: &mut PgConnection,
    sha256: &str,
) -> sqlx::Result<Option<StoredFile>> {
    let row = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(t.id, h.track_id) AS track_id
        FROM files f
        LEFT JOIN tracks t ON t.file_id = f.id
        LEFT JOIN LATERAL (
            SELECT track_id FROM track_file_history WHERE old_file_id = f.id LIMIT 1
        ) h ON true
        WHERE f.sha256 = $1
        "#,
        sha256
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|track_id| StoredFile { track_id }))
}

pub struct NewFile<'a> {
    pub id: Uuid,
    /// Where the file came from (required).
    pub origin_snapshot_id: Uuid,
    pub sha256: &'a str,
    pub original_filename: &'a str,
    pub uploaded_filename: &'a str,
    pub size_bytes: i64,
    pub container: Option<&'a str>,
    pub codec: Option<&'a str>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub bit_depth: Option<i16>,
    pub duration_ms: Option<i64>,
    pub embedded_tags: &'a serde_json::Value,
}

pub async fn insert_file(conn: &mut PgConnection, f: &NewFile<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO files (id, origin_snapshot_id, sha256, original_filename, uploaded_filename,
                           size_bytes, container, codec, bitrate, sample_rate, channels,
                           bit_depth, duration_ms, embedded_tags)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
        f.id,
        f.origin_snapshot_id,
        f.sha256,
        f.original_filename,
        f.uploaded_filename,
        f.size_bytes,
        f.container,
        f.codec,
        f.bitrate,
        f.sample_rate,
        f.channels,
        f.bit_depth,
        f.duration_ms,
        f.embedded_tags,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Creates a track together with its first variant and revision 0.
///
/// This (and later the metadata edit code) is the only place that writes the
/// denormalized metadata columns on `tracks`.
pub async fn create_track(
    conn: &mut PgConnection,
    file_id: Uuid,
    status: &str,
    meta: &TrackMetadata,
    variant_label: &str,
    revision_reason: &str,
) -> anyhow::Result<Uuid> {
    // Generate the ids up front: the track points at its variant and the variant points
    // at the track. The FK from tracks to track_variants is DEFERRABLE, so it's only
    // checked at COMMIT, by which point both rows exist. (Same for `file_id`: the file
    // row is inserted later in the transaction.)
    let track_id = Uuid::now_v7();
    let variant_id = Uuid::now_v7();
    let data = serde_json::to_value(meta)?;

    sqlx::query!(
        r#"
        INSERT INTO tracks (id, file_id, status, primary_variant_id, title, release_date, genres)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
        track_id,
        file_id,
        status,
        variant_id,
        meta.title,
        meta.release_date,
        &meta.genres,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "INSERT INTO track_variants (id, track_id, label) VALUES ($1, $2, $3)",
        variant_id,
        track_id,
        variant_label,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        r#"
        INSERT INTO track_revisions (track_id, variant_id, source, reason, data)
        VALUES ($1, $2, 'import', $3, $4)
        "#,
        track_id,
        variant_id,
        revision_reason,
        data,
    )
    .execute(&mut *conn)
    .await?;

    Ok(track_id)
}

/// Finds an artist by exact (case-insensitive) name or alias, or creates a new one.
/// Returns the artist id and its canonical name.
///
/// TODO(Phase 2): when several artists share a name, flag the credit for review
/// instead of silently picking the oldest one.
pub async fn find_or_create_artist(
    conn: &mut PgConnection,
    name: &str,
) -> sqlx::Result<(Uuid, String)> {
    let existing = sqlx::query!(
        r#"
        SELECT id AS "id!", name AS "name!" FROM (
            SELECT a.id, a.name, 0 AS rank FROM artists a WHERE lower(a.name) = lower($1)
            UNION ALL
            SELECT a.id, a.name, 1 AS rank FROM artist_aliases al
            JOIN artists a ON a.id = al.artist_id
            WHERE lower(al.alias) = lower($1)
        ) matches
        ORDER BY rank, id
        LIMIT 1
        "#,
        name
    )
    .fetch_optional(&mut *conn)
    .await?;

    if let Some(row) = existing {
        return Ok((row.id, row.name));
    }

    let id = sqlx::query_scalar!("INSERT INTO artists (name) VALUES ($1) RETURNING id", name)
        .fetch_one(&mut *conn)
        .await?;
    Ok((id, name.to_string()))
}

pub async fn insert_track_credit(
    conn: &mut PgConnection,
    track_id: Uuid,
    artist_id: Uuid,
    role: &str,
    credited_as: Option<&str>,
    position: i16,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO track_credits (track_id, artist_id, role, credited_as, position)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT DO NOTHING
        "#,
        track_id,
        artist_id,
        role,
        credited_as,
        position,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn set_artist_display(
    conn: &mut PgConnection,
    track_id: Uuid,
    artist_display: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE tracks SET artist_display = $2, updated_at = now() WHERE id = $1",
        track_id,
        artist_display
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Finds an album with the same title (case-insensitive) credited to the same primary
/// artist, or to no artist if `album_artists` is empty. Creates it if there's none.
pub async fn find_or_create_album(
    conn: &mut PgConnection,
    title: &str,
    album_artists: &[Uuid],
    release_date: Option<&str>,
) -> sqlx::Result<Uuid> {
    let primary_artist = album_artists.first().copied();

    let existing = sqlx::query_scalar!(
        r#"
        SELECT a.id FROM albums a
        WHERE lower(a.title) = lower($1)
          AND CASE WHEN $2::uuid IS NULL
                   THEN NOT EXISTS (SELECT 1 FROM album_credits c WHERE c.album_id = a.id)
                   ELSE EXISTS (SELECT 1 FROM album_credits c
                                WHERE c.album_id = a.id AND c.artist_id = $2 AND c.role = 'primary')
              END
        ORDER BY a.id
        LIMIT 1
        "#,
        title,
        primary_artist as Option<Uuid>,
    )
    .fetch_optional(&mut *conn)
    .await?;

    if let Some(id) = existing {
        return Ok(id);
    }

    let album_id = sqlx::query_scalar!(
        "INSERT INTO albums (title, release_date) VALUES ($1, $2) RETURNING id",
        title,
        release_date,
    )
    .fetch_one(&mut *conn)
    .await?;

    for (position, artist_id) in album_artists.iter().enumerate() {
        sqlx::query!(
            r#"
            INSERT INTO album_credits (album_id, artist_id, role, position)
            VALUES ($1, $2, 'primary', $3)
            ON CONFLICT DO NOTHING
            "#,
            album_id,
            artist_id,
            position as i16,
        )
        .execute(&mut *conn)
        .await?;
    }

    Ok(album_id)
}

pub async fn add_album_track(
    conn: &mut PgConnection,
    album_id: Uuid,
    track_id: Uuid,
    disc_number: Option<i16>,
    track_number: Option<i16>,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO album_tracks (album_id, track_id, disc_number, track_number)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT DO NOTHING
        "#,
        album_id,
        track_id,
        disc_number,
        track_number,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_track_cover(
    conn: &mut PgConnection,
    track_id: Uuid,
    blob_sha256: &str,
    mime: Option<&str>,
    position: i16,
    source: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO covers (track_id, blob_sha256, mime, position, source)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        track_id,
        blob_sha256,
        mime,
        position,
        source,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn insert_lyrics(
    conn: &mut PgConnection,
    track_id: Uuid,
    is_official: bool,
    is_synced: bool,
    source: &str,
    body: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO lyrics (track_id, is_official, is_synced, source, body)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        track_id,
        is_official,
        is_synced,
        source,
        body,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Locks the track row for the rest of the transaction and returns its current file.
/// Returns None if the track doesn't exist.
pub async fn lock_track_file(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar!(
        "SELECT file_id FROM tracks WHERE id = $1 FOR UPDATE",
        track_id
    )
    .fetch_optional(conn)
    .await
}

/// Points a track at a new file and records the change. The old file is kept.
pub async fn replace_track_file(
    conn: &mut PgConnection,
    track_id: Uuid,
    old_file_id: Uuid,
    new_file_id: Uuid,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO track_file_history (track_id, old_file_id, new_file_id, reason)
        VALUES ($1, $2, $3, $4)
        "#,
        track_id,
        old_file_id,
        new_file_id,
        reason,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "UPDATE tracks SET file_id = $2, updated_at = now() WHERE id = $1",
        track_id,
        new_file_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

