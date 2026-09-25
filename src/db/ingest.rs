//! Database writes used by the ingest pipeline.
//!
//! Every function takes `&mut PgConnection` so it can run inside a transaction:
//! call them with `&mut *tx` (the `*` turns the `Transaction` into the connection
//! it wraps, and `&mut` borrows that).

use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::{self, Source};
use crate::model::sources::{Kind, SourceOwner};
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
    /// Only for an original file: its name. None for everything else (rips, downloads).
    pub original_filename: Option<&'a str>,
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
#[derive(Debug, Clone)]
pub struct FoundArtist {
    pub id: Uuid,
    /// The alias the name is (None = the artist's own name).
    pub alias_id: Option<Uuid>,
    /// All artists the name matched, if more than one (then the best match was used, and
    /// ingest flags it for review).
    pub ambiguous: Vec<Uuid>,
    /// True if no artist matched and this one was just created.
    pub created: bool,
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
            created: true,
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
        created: false,
    })
}

/// Finds the album a file's tags describe: the same title (ignoring case), exactly the
/// same album artists (as a set, whatever their role: none missing, none extra) and the same release
/// date (verbatim; no date matches no date). Anything different is a different album,
/// even with the same title: splitting too eagerly is fixed by merging, while wrongly
/// joined albums are much harder to notice.
pub async fn find_album(
    conn: &mut PgConnection,
    title: &str,
    album_artist_ids: &[Uuid],
    release_date: Option<&str>,
) -> sqlx::Result<Option<Uuid>> {
    let mut artists = album_artist_ids.to_vec();
    artists.sort();
    artists.dedup();
    sqlx::query_scalar!(
        r#"
        SELECT a.id FROM albums a
        WHERE lower(a.title) = lower($1)
          AND a.release_date IS NOT DISTINCT FROM $3
          AND COALESCE((SELECT array_agg(DISTINCT c.artist_id ORDER BY c.artist_id)
                        FROM album_credits c
                        WHERE c.album_id = a.id), '{}') = $2::uuid[]
        ORDER BY a.id
        LIMIT 1
        "#,
        title,
        &artists,
        release_date,
    )
    .fetch_optional(conn)
    .await
}

/// Creates an album from a file's tags (with album revision 0). Every album artist is
/// primary, except on a `single`: there only the first is, and the others are featured
/// (NetEase lists everyone on a single as album artists).
pub async fn create_album(
    conn: &mut PgConnection,
    title: &str,
    album_artists: &[FoundArtist],
    single: bool,
    release_date: Option<&str>,
) -> anyhow::Result<Uuid> {
    let mut credits: Vec<Credit> = Vec::new();
    for artist in album_artists {
        // The same artist twice in the tag would be a duplicate credit.
        if !credits.iter().any(|c| c.artist_id == artist.id) {
            let role = if single && !credits.is_empty() { "featured" } else { "primary" };
            credits.push(Credit {
                artist_id: artist.id,
                role: role.to_string(),
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

/// The artists, albums or tracks that have this platform id among their sources (not
/// counting trashed ones: those were wrong). Artists that were merged lead to the artist
/// they were merged into; merged tracks aren't included.
pub async fn owners_with_platform_id(
    conn: &mut PgConnection,
    kind: Kind,
    platform: &str,
    external_id: &str,
) -> sqlx::Result<Vec<Uuid>> {
    match kind {
        Kind::Artist => {
            sqlx::query_scalar!(
                r#"
                SELECT DISTINCT COALESCE(a.merged_into, a.id) AS "id!"
                FROM sources s JOIN artists a ON a.id = s.artist_id
                WHERE s.platform = $1 AND s.external_id = $2 AND s.trashed_at IS NULL
                ORDER BY 1
                "#,
                platform,
                external_id
            )
            .fetch_all(conn)
            .await
        }
        Kind::Album => {
            sqlx::query_scalar!(
                r#"
                SELECT DISTINCT s.album_id AS "id!"
                FROM sources s
                WHERE s.platform = $1 AND s.external_id = $2 AND s.trashed_at IS NULL
                  AND s.album_id IS NOT NULL
                ORDER BY 1
                "#,
                platform,
                external_id
            )
            .fetch_all(conn)
            .await
        }
        Kind::Track => {
            sqlx::query_scalar!(
                r#"
                SELECT DISTINCT t.id
                FROM sources s JOIN tracks t ON t.id = s.track_id
                WHERE s.platform = $1 AND s.external_id = $2 AND s.trashed_at IS NULL
                  AND t.merged_into IS NULL
                ORDER BY 1
                "#,
                platform,
                external_id
            )
            .fetch_all(conn)
            .await
        }
    }
}

/// An owner's ids on a platform, e.g. an artist's NetEase artist ids. With `trashed`,
/// only the ones in the trash (marked wrong), else only the ones that aren't.
pub async fn platform_ids_of(
    conn: &mut PgConnection,
    owner: SourceOwner,
    platform: &str,
    trashed: bool,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        r#"
        SELECT external_id AS "external_id!" FROM sources
        WHERE COALESCE(track_id, album_id, artist_id) = $1
          AND platform = $2 AND external_id IS NOT NULL
          AND (trashed_at IS NOT NULL) = $3
        ORDER BY external_id
        "#,
        owner.id(),
        platform,
        trashed
    )
    .fetch_all(conn)
    .await
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
        // A merged track has no file of its own anymore: as good as not there.
        r#"SELECT file_id AS "file_id!" FROM tracks WHERE id = $1 AND merged_into IS NULL FOR UPDATE"#,
        track_id
    )
    .fetch_optional(conn)
    .await
}

/// Adds a file to a track as a previous file: recorded in the file history as if the
/// current file had replaced it, so it's listed, streamable and cleanable (trash, purge)
/// like any previous file. The track keeps playing its current file.
pub async fn add_previous_file(
    conn: &mut PgConnection,
    track_id: Uuid,
    file_id: Uuid,
    current_file_id: Uuid,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO track_file_history (track_id, old_file_id, new_file_id, reason)
        VALUES ($1, $2, $3, $4)
        "#,
        track_id,
        file_id,
        current_file_id,
        reason,
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("UPDATE tracks SET updated_at = now() WHERE id = $1", track_id)
        .execute(conn)
        .await?;
    Ok(())
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

