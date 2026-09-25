//! Database writes used by the ingest pipeline.
//!
//! Every function takes `&mut PgConnection` so it can run inside a transaction:
//! call them with `&mut *tx` (the `*` turns the `Transaction` into the connection
//! it wraps, and `&mut` borrows that).

use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::{self, Source};
use crate::model::{AlbumData, ArtistData, Credit};

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
        -- A purged file's bytes are gone: storing them again makes a new file.
        WHERE f.sha256 = $1 AND f.purged_at IS NULL
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

/// An artist found (or created) for a name from the tags, and how to credit it.
pub struct FoundArtist {
    pub id: Uuid,
    /// The alias the name is (None = the artist's own name).
    pub alias_id: Option<Uuid>,
    /// All artists the name matched, if more than one (then the best match was used, and
    /// ingest flags it for review).
    pub ambiguous: Vec<Uuid>,
}

/// Finds the artist a name from the tags refers to, creating what's missing:
/// * exactly an artist's name or listed alias: that (a merged artist's name leads to the
///   artist it was merged into);
/// * the same apart from case ("mira vale" for "Mira Vale"): that artist, with the
///   spelling added as a new alias, so the tags' exact spelling is kept;
/// * otherwise a new artist.
///
/// If several artists match, exact spellings win, then names over aliases, then the oldest.
pub async fn find_or_create_artist(
    conn: &mut PgConnection,
    name: &str,
) -> anyhow::Result<FoundArtist> {
    let matches = sqlx::query!(
        r#"
        SELECT artist_id AS "artist_id!", alias_id, exact AS "exact!" FROM (
            SELECT COALESCE(a.merged_into, a.id) AS artist_id,
                   CASE WHEN a.merged_into IS NOT NULL THEN a.merged_alias_id END AS alias_id,
                   a.name = $1 AS exact, 0 AS rank, a.id AS ord
            FROM artists a
            WHERE lower(a.name) = lower($1)
            UNION ALL
            SELECT al.artist_id, al.id, al.alias = $1, 1, al.id
            FROM artist_aliases al
            WHERE lower(al.alias) = lower($1) AND al.listed
        ) matches
        ORDER BY exact DESC, rank, ord
        "#,
        name
    )
    .fetch_all(&mut *conn)
    .await?;

    let Some(best) = matches.first() else {
        let data = ArtistData {
            name: name.to_string(),
            ..Default::default()
        };
        let id =
            revisions::create_artist(conn, &data, Source::Import, "imported_from_file").await?;
        return Ok(FoundArtist {
            id,
            alias_id: None,
            ambiguous: Vec::new(),
        });
    };

    let mut ids: Vec<Uuid> = matches.iter().map(|m| m.artist_id).collect();
    ids.sort();
    ids.dedup(); // an artist can match by name and by alias
    let ambiguous = if ids.len() > 1 { ids } else { Vec::new() };

    let alias_id = if best.exact {
        best.alias_id
    } else {
        let alias = revisions::ensure_alias(
            conn,
            best.artist_id,
            name,
            Source::Import,
            "spelling from the tags",
        )
        .await?;
        Some(alias)
    };
    Ok(FoundArtist {
        id: best.artist_id,
        alias_id,
        ambiguous,
    })
}

/// Finds an album with the same title (case-insensitive) credited to the same primary
/// artist, or to no artist if `album_artists` is empty. Creates it (with album revision 0)
/// if there's none.
pub async fn find_or_create_album(
    conn: &mut PgConnection,
    title: &str,
    album_artists: &[FoundArtist],
    release_date: Option<&str>,
) -> anyhow::Result<Uuid> {
    let primary_artist = album_artists.first().map(|a| a.id);

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

    let mut credits: Vec<Credit> = Vec::new();
    for artist in album_artists {
        // The same artist twice in the tag would be a duplicate credit.
        if !credits.iter().any(|c| c.artist_id == artist.id) {
            credits.push(Credit {
                artist_id: artist.id,
                role: "primary".to_string(),
                alias_id: artist.alias_id,
            });
        }
    }
    let data = AlbumData {
        title: title.to_string(),
        release_date: release_date.map(str::to_string),
        credits,
        ..Default::default()
    };
    revisions::create_album(conn, &data, Source::Import, "imported_from_file").await
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

