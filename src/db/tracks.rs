//! Read queries for tracks.

use serde::Serialize;
use sqlx::PgConnection;
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
    /// The file it plays (`GET /api/files/{id}/stream`).
    pub file_id: Uuid,
}

/// One page of tracks, newest first.
///
/// Uses keyset pagination: pass the last id of the previous page as `before`.
/// Unlike OFFSET, this stays fast at any depth because it's a single index range scan.
/// (UUIDv7 ids are time-ordered, so ordering by id means ordering by creation time.)
pub async fn list_tracks(
    conn: &mut PgConnection,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<TrackSummary>> {
    sqlx::query_as!(
        TrackSummary,
        r#"
        SELECT t.id, t.title, t.artist_display, t.status, t.release_date,
               f.duration_ms, f.codec, f.id AS file_id
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.merged_into IS NULL  -- merged tracks only redirect
          AND t.id < COALESCE($1, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY t.id DESC
        LIMIT $2
        "#,
        before,
        limit,
    )
    .fetch_all(&mut *conn)
    .await
}

/// Like `list_tracks`, filtered to tracks whose title or artist contains `query`
/// (case-insensitive). The trigram indexes make `ILIKE '%...%'` fast.
pub async fn search_tracks(
    conn: &mut PgConnection,
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
               f.duration_ms, f.codec, f.id AS file_id
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE (t.title ILIKE $1 OR t.artist_display ILIKE $1)
          AND t.merged_into IS NULL
          AND t.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY t.id DESC
        LIMIT $3
        "#,
        pattern,
        before,
        limit,
    )
    .fetch_all(&mut *conn)
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
    /// Other tracks playing the same file (see `track_files::use_file_of`), if visible.
    pub file_shared_with: Vec<super::track_files::SharedWith>,
    /// Previous files. Only included for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_history: Option<Vec<FileChange>>,
    /// Changes of the current file's origin, newest first (see
    /// `sources::set_file_origin`). Only included for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_changes: Option<Vec<super::sources::OriginChange>>,
    /// Changes of whether the current file is an original file (and its name), newest
    /// first (see `og_files`). Only for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_filename_changes: Option<Vec<super::og_files::Change>>,
    pub sources: Vec<super::sources::Source>,
    pub credits: Vec<Credit>,
    pub albums: Vec<AlbumEntry>,
    /// Seen from this track.
    pub relations: Vec<RelationEntry>,
    pub variants: Vec<Variant>,
    /// The latest external revision (status, albums, relations): the base for editing them.
    pub external_revision_id: Option<Uuid>,
    /// Only included for users allowed to view history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<Revision>>,
    pub lyrics: Vec<Lyrics>,
    /// Tracks merged into this one (filled in by the API, see `revisions::merges_into`).
    pub merges: Vec<super::revisions::MergeInfo>,
    /// The current user's plays, rating and star.
    pub annotation: super::annotations::Annotation,
    pub covers: Vec<Cover>,
}

#[derive(Debug, Serialize)]
pub struct FileInfo {
    pub id: Uuid,
    pub sha256: String,
    /// Set only if this is an original file: its name (see migration 0013).
    pub original_filename: Option<String>,
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
    pub old_original_filename: Option<String>,
    pub old_uploaded_filename: String,
    pub old_codec: Option<String>,
    pub old_bitrate: Option<i32>,
    pub old_origin: Option<super::sources::Origin>,
    pub old_size_bytes: i64,
    /// The old file is in the trash (and can be purged after the grace period).
    pub old_trashed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The old file's bytes were deleted; only its record is left.
    pub old_purged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub old_purged_by: Option<String>,
    pub new_file_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct Credit {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub role: String,
    /// The alias it's credited under (None = the artist's name).
    pub alias_id: Option<Uuid>,
    pub alias: Option<String>,
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
pub struct RelationEntry {
    pub kind: String,
    pub track_id: Uuid,
    pub title: String,
    pub artist_display: String,
    /// How to say it, e.g. "%A is a remix of %B": %A is this track, %B the other one.
    pub template: String,
}

#[derive(Debug, Serialize)]
pub struct Variant {
    pub id: Uuid,
    pub label: String,
    /// The variant's latest internal revision: the base for editing it.
    pub revision_id: Uuid,
    /// Its current metadata (`model::TrackMetadata`).
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct Revision {
    pub id: Uuid,
    /// "internal" (a variant's metadata) or "external" (status, albums, relations).
    pub kind: String,
    /// Set for internal revisions.
    pub variant_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub author_id: Option<Uuid>,
    pub author: Option<String>,
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
    pub language: Option<String>,
    /// Where the entry came from: "embedded" (in the file), "manual".
    pub source: String,
    pub body: String,
    /// The latest revision: the base for editing.
    pub revision_id: Uuid,
    /// With `view_history` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisions: Option<Vec<super::catalog::EntityRevision>>,
}

/// A cover of a track or album (not in the trash). The first one is the main cover.
#[derive(Debug, Serialize)]
pub struct Cover {
    pub id: Uuid,
    pub blob_sha256: String,
    pub mime: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub position: i16,
    /// "embedded" (in the file), "upload", "copied".
    pub source: String,
}

/// If the track was merged into another: the one it redirects to.
pub async fn merged_into(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar!("SELECT merged_into FROM tracks WHERE id = $1", id)
        .fetch_optional(conn)
        .await?
        .flatten())
}

/// Everything about one track. Returns None if it doesn't exist. Revisions and previous
/// files are only loaded if `with_history`.
pub async fn get_track(
    conn: &mut PgConnection,
    id: Uuid,
    with_history: bool,
) -> sqlx::Result<Option<TrackDetail>> {
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
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };

    let history_rows = if !with_history {
        Vec::new()
    } else {
        sqlx::query!(
            r#"
            SELECT h.replaced_at, h.reason, h.old_file_id, f.original_filename AS old_original_filename,
                   f.uploaded_filename AS old_uploaded_filename,
                   f.codec AS old_codec, f.bitrate AS old_bitrate, f.size_bytes AS old_size_bytes,
                   f.trashed_at AS old_trashed_at, f.purged_at AS old_purged_at,
                   pu.username AS "old_purged_by?", h.new_file_id
            FROM track_file_history h
            JOIN files f ON f.id = h.old_file_id
            LEFT JOIN users pu ON pu.id = f.purged_by
            WHERE h.track_id = $1
            ORDER BY h.id DESC
            "#,
            id
        )
        .fetch_all(&mut *conn)
        .await?
    };

    // Origins of the current file and all previous files, in one query.
    let mut file_ids: Vec<Uuid> = history_rows.iter().map(|h| h.old_file_id).collect();
    file_ids.push(row.file_id);
    let mut origins = super::sources::origins_of_files(conn, &file_ids).await?;

    let file_history = history_rows
        .into_iter()
        .map(|h| FileChange {
            old_origin: origins.remove(&h.old_file_id),
            replaced_at: h.replaced_at,
            reason: h.reason,
            old_file_id: h.old_file_id,
            old_original_filename: h.old_original_filename,
            old_uploaded_filename: h.old_uploaded_filename,
            old_codec: h.old_codec,
            old_bitrate: h.old_bitrate,
            old_size_bytes: h.old_size_bytes,
            old_trashed_at: h.old_trashed_at,
            old_purged_at: h.old_purged_at,
            old_purged_by: h.old_purged_by,
            new_file_id: h.new_file_id,
        })
        .collect();
    let file_history = with_history.then_some(file_history);
    let origin_changes = if with_history { Some(super::sources::origin_changes(conn, row.file_id).await?) } else { None };
    let original_filename_changes = if with_history { Some(super::og_files::changes(conn, row.file_id).await?) } else { None };

    let sources = super::sources::list_for(conn, id).await?;
    let file_shared_with = super::track_files::shared_with(conn, row.file_id, id).await?;

    let credits = sqlx::query_as!(
        Credit,
        r#"
        SELECT c.artist_id, a.name AS artist_name, c.role, c.alias_id, al.alias AS "alias?"
        FROM track_credits c
        JOIN artists a ON a.id = c.artist_id
        LEFT JOIN artist_aliases al ON al.id = c.alias_id
        WHERE c.track_id = $1
        ORDER BY c.position, a.name
        "#,
        id
    )
    .fetch_all(&mut *conn)
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
    .fetch_all(&mut *conn)
    .await?;

    let relations = sqlx::query_as!(
        RelationEntry,
        r#"
        SELECT r.kind, r.to_track_id AS track_id, t.title, t.artist_display, k.template
        FROM track_relations r
        JOIN tracks t ON t.id = r.to_track_id
        JOIN relation_kinds k ON k.kind = r.kind
        WHERE r.from_track_id = $1
        ORDER BY r.kind, t.title
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    let variants = sqlx::query_as!(
        Variant,
        r#"
        SELECT v.id, v.label, latest.id AS "revision_id!", latest.data AS "data!"
        FROM track_variants v
        CROSS JOIN LATERAL (
            SELECT r.id, r.data FROM track_revisions r
            WHERE r.variant_id = v.id ORDER BY r.id DESC LIMIT 1
        ) latest
        WHERE v.track_id = $1
        ORDER BY v.id
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut variants = variants;
    strip_hidden(conn, variants.iter_mut().map(|v| &mut v.data)).await?;

    let external_revision_id = sqlx::query_scalar!(
        r#"
        SELECT id FROM track_revisions
        WHERE track_id = $1 AND kind = 'external'
        ORDER BY id DESC LIMIT 1
        "#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;

    let revisions = if !with_history {
        None
    } else {
        let revisions = sqlx::query_as!(
            Revision,
            r#"
            SELECT r.id, r.kind, r.variant_id, r.created_at, r.author_id,
                   u.username AS "author?", r.source, r.reason, r.is_canon, r.data
            FROM track_revisions r
            LEFT JOIN users u ON u.id = r.author_id
            WHERE r.track_id = $1
            ORDER BY r.id DESC
            "#,
            id
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut revisions = revisions;
        strip_hidden(conn, revisions.iter_mut().map(|r| &mut r.data)).await?;
        Some(revisions)
    };

    let lyrics = lyrics_of(conn, id, with_history).await?;
    let covers = covers_of(conn, id).await?;

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
        file_shared_with,
        file_history,
        origin_changes,
        original_filename_changes,
        sources,
        credits,
        albums,
        relations,
        variants,
        external_revision_id,
        revisions,
        lyrics,
        covers,
        merges: Vec::new(),
        annotation: super::annotations::get(conn, super::annotations::Item::Track(id)).await?,
    }))
}

/// Removes from revision data (as JSON) what the transaction's user can't see: relations
/// with hidden tracks, hidden albums, and credits of hidden artists.
///
/// Revisions store the whole picture, including things only some users may see (e.g. a
/// relation with a restricted track). Row-level security filters rows, not what's inside
/// a JSON column, so that's done here, before revision data is shown to anyone.
async fn strip_hidden<'a>(
    conn: &mut PgConnection,
    datas: impl Iterator<Item = &'a mut serde_json::Value>,
) -> sqlx::Result<()> {
    // `impl Iterator<Item = &mut Value>`: any sequence of mutable references to the
    // data, e.g. `revisions.iter_mut().map(|r| &mut r.data)`. The `'a` just says those
    // references live as long as the iterator hands them out.
    let datas: Vec<&mut serde_json::Value> = datas.collect();

    // Every id mentioned, then one query per kind for the visible ones.
    let ids_at = |data: &serde_json::Value, list: &str, field: &str| -> Vec<Uuid> {
        data.get(list)
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get(field)?.as_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut track_ids = Vec::new();
    let mut album_ids = Vec::new();
    let mut artist_ids = Vec::new();
    for data in &datas {
        track_ids.extend(ids_at(data, "relations", "track_id"));
        album_ids.extend(ids_at(data, "albums", "album_id"));
        artist_ids.extend(ids_at(data, "credits", "artist_id"));
    }
    if track_ids.is_empty() && album_ids.is_empty() && artist_ids.is_empty() {
        return Ok(());
    }
    let tracks = sqlx::query_scalar!("SELECT id FROM tracks WHERE id = ANY($1)", &track_ids)
        .fetch_all(&mut *conn)
        .await?;
    let albums = sqlx::query_scalar!("SELECT id FROM albums WHERE id = ANY($1)", &album_ids)
        .fetch_all(&mut *conn)
        .await?;
    let artists = sqlx::query_scalar!("SELECT id FROM artists WHERE id = ANY($1)", &artist_ids)
        .fetch_all(&mut *conn)
        .await?;

    let keep_visible = |data: &mut serde_json::Value, list: &str, field: &str, visible: &[Uuid]| {
        if let Some(items) = data.get_mut(list).and_then(|v| v.as_array_mut()) {
            items.retain(|item| {
                item.get(field)
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok())
                    .is_some_and(|id| visible.contains(&id))
            });
        }
    };
    for data in datas {
        keep_visible(data, "relations", "track_id", &tracks);
        keep_visible(data, "albums", "album_id", &albums);
        keep_visible(data, "credits", "artist_id", &artists);
    }
    Ok(())
}

/// A track's lyrics entries, official ones first; with their history if `with_history`.
async fn lyrics_of(conn: &mut PgConnection, track_id: Uuid, with_history: bool) -> sqlx::Result<Vec<Lyrics>> {
    let rows = sqlx::query!(
        r#"
        SELECT l.id, l.is_official, l.is_synced, l.language, l.source, l.body,
               (SELECT r.id FROM lyrics_revisions r
                WHERE r.lyrics_id = l.id ORDER BY r.id DESC LIMIT 1) AS "revision_id!"
        FROM lyrics l WHERE l.track_id = $1
        ORDER BY l.is_official DESC, l.id
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut lyrics: Vec<Lyrics> = rows
        .into_iter()
        .map(|r| Lyrics {
            id: r.id,
            is_official: r.is_official,
            is_synced: r.is_synced,
            language: r.language,
            source: r.source,
            body: r.body,
            revision_id: r.revision_id,
            revisions: None, // filled in below, if asked for
        })
        .collect();
    if with_history {
        for entry in &mut lyrics {
            let revisions = sqlx::query_as!(
                super::catalog::EntityRevision,
                r#"
                SELECT r.id, r.created_at, u.username AS "author?", r.source, r.reason, r.data
                FROM lyrics_revisions r
                LEFT JOIN users u ON u.id = r.author_id
                WHERE r.lyrics_id = $1
                ORDER BY r.id DESC
                "#,
                entry.id
            )
            .fetch_all(&mut *conn)
            .await?;
            entry.revisions = Some(revisions);
        }
    }
    Ok(lyrics)
}

/// The covers of a track or an album (by the owner's id), main cover first, without
/// the ones in the trash.
pub async fn covers_of(conn: &mut PgConnection, owner_id: Uuid) -> sqlx::Result<Vec<Cover>> {
    sqlx::query_as!(
        Cover,
        r#"
        SELECT id, blob_sha256, mime, width, height, position, source
        FROM covers
        WHERE (track_id = $1 OR album_id = $1) AND trashed_at IS NULL
        ORDER BY position, id
        "#,
        owner_id
    )
    .fetch_all(conn)
    .await
}
