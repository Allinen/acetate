//! The whole library as one user sees it, in one go (`GET /api/catalog`): what a client
//! needs to browse, sort and search locally without asking the server for every list.
//!
//! Each part is built by Postgres itself as JSON (`json_agg`), a row as an array in the
//! order of its `*_COLUMNS` (keys would be repeated 185k times otherwise). The queries are
//! set-based: each related table is grouped once and joined, rather than a subquery per
//! row. Row-level security decides what's in it, as everywhere; merged things are left
//! out (they only redirect).

use sqlx::PgConnection;
use uuid::Uuid;

/// A track's columns, in the order of its row arrays.
/// `credits`: `[[artist_id, role, alias], …]` in credit order; `alias` is the name it's
/// credited under when that's an alias, else null.
/// `albums`: `[[album_id, disc, track], …]`, every album it's on, its main one first (a real
/// album before a single, then the oldest). `cover_id`: its own first cover, else its main
/// album's. `cover_key`: the start of that image's SHA-256, the same for the same image
/// under different covers (every track of an album often has its own copy): clients can
/// keep one copy per image.
pub const TRACK_COLUMNS: &[&str] = &[
    "id", "title", "artist_display", "credits", "albums", "duration_ms",
    "file_id", "cover_id", "cover_key", "release_date", "year", "status", "genres", "created_at",
    "is_restricted", "codec",
];

/// `credits` as for tracks; `artist_display` with aliases as credited.
/// `cover_id`: its own first cover, else its first track's; `cover_key` as for tracks.
pub const ALBUM_COLUMNS: &[&str] = &[
    "id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key",
    "created_at", "track_count", "duration_ms",
];

pub const ARTIST_COLUMNS: &[&str] = &["id", "name", "disambiguation", "aliases", "track_count", "album_count"];

/// Each track's open review flags: `[[flag id, kind], ...]` (kinds in `review_kinds`
/// order). The whole catalog has a row per track with any; a delta has one per changed
/// track, `[]` when it has none anymore.
pub const ISSUE_COLUMNS: &[&str] = &["track_id", "flags"];

/// The current user's annotations. `kind`: "track", "album" or "artist".
pub const ANNOTATION_COLUMNS: &[&str] = &["kind", "id", "play_count", "last_played_at", "rating", "starred_at"];

/// The open flags of the tracks (all with any, or these ones), see `ISSUE_COLUMNS`.
pub async fn issues_json(conn: &mut PgConnection, ids: Option<&[Uuid]>) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH open AS (
            SELECT f.track_id, json_agg(json_build_array(f.id, f.kind) ORDER BY k.position, f.id) AS flags
            FROM review_flags f
            JOIN review_kinds k ON k.kind = f.kind
            WHERE f.resolved_at IS NULL AND ($1::uuid[] IS NULL OR f.track_id = ANY($1))
            GROUP BY f.track_id
        )
        SELECT COALESCE(json_agg(json_build_array(t.id, COALESCE(o.flags, '[]'::json)) ORDER BY t.id), '[]'::json)::text AS "rows!"
        FROM tracks t
        LEFT JOIN open o ON o.track_id = t.id
        WHERE t.merged_into IS NULL
          AND (($1::uuid[] IS NULL AND o.track_id IS NOT NULL) OR t.id = ANY($1))
        "#,
        ids as Option<&[Uuid]>
    )
    .fetch_one(conn)
    .await
}

pub async fn tracks_json(conn: &mut PgConnection, ids: Option<&[Uuid]>) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH credits AS (
            SELECT c.track_id,
                   json_agg(json_build_array(c.artist_id, c.role, al.alias)
                            ORDER BY c.position, c.role) AS credits
            FROM track_credits c
            LEFT JOIN artist_aliases al ON al.id = c.alias_id
            WHERE $1::uuid[] IS NULL OR c.track_id = ANY($1)
            GROUP BY c.track_id
        ), memberships AS (
            SELECT at.track_id,
                   json_agg(json_build_array(at.album_id, at.disc_number, at.track_number)
                            ORDER BY a.kind = 'single', a.id) AS albums,
                   (array_agg(at.album_id ORDER BY a.kind = 'single', a.id))[1] AS main_album
            FROM album_tracks at
            JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
            WHERE $1::uuid[] IS NULL OR at.track_id = ANY($1)
            GROUP BY at.track_id
        ), track_cover AS (
            SELECT DISTINCT ON (c.track_id) c.track_id, c.id, left(c.blob_sha256, 16) AS key
            FROM covers c
            WHERE c.track_id IS NOT NULL AND c.trashed_at IS NULL
              AND ($1::uuid[] IS NULL OR c.track_id = ANY($1))
            ORDER BY c.track_id, c.position, c.id
        ), album_cover AS (
            SELECT DISTINCT ON (c.album_id) c.album_id, c.id, left(c.blob_sha256, 16) AS key
            FROM covers c
            WHERE c.album_id IS NOT NULL AND c.trashed_at IS NULL
            ORDER BY c.album_id, c.position, c.id
        )
        SELECT COALESCE(json_agg(json_build_array(
                   t.id, t.title, t.artist_display, COALESCE(cr.credits, '[]'::json),
                   COALESCE(m.albums, '[]'::json), f.duration_ms,
                   t.file_id, COALESCE(tc.id, ac.id), CASE WHEN tc.id IS NOT NULL THEN tc.key ELSE ac.key END,
                   t.release_date, t.release_year, t.status,
                   t.genres, t.created_at, t.is_restricted, f.codec
               ) ORDER BY t.id), '[]'::json)::text AS "rows!"
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        LEFT JOIN credits cr ON cr.track_id = t.id
        LEFT JOIN memberships m ON m.track_id = t.id
        LEFT JOIN track_cover tc ON tc.track_id = t.id
        LEFT JOIN album_cover ac ON ac.album_id = m.main_album
        WHERE t.merged_into IS NULL AND ($1::uuid[] IS NULL OR t.id = ANY($1))
        "#,
        ids as Option<&[Uuid]>
    )
    .fetch_one(conn)
    .await
}

pub async fn albums_json(conn: &mut PgConnection, ids: Option<&[Uuid]>) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH credits AS (
            SELECT c.album_id,
                   json_agg(json_build_array(c.artist_id, c.role, al.alias)
                            ORDER BY c.position, c.role) AS credits,
                   string_agg(COALESCE(al.alias, ar.name), ', ' ORDER BY c.position, c.role) AS display
            FROM album_credits c
            JOIN artists ar ON ar.id = c.artist_id
            LEFT JOIN artist_aliases al ON al.id = c.alias_id
            WHERE $1::uuid[] IS NULL OR c.album_id = ANY($1)
            GROUP BY c.album_id
        ), own_cover AS (
            SELECT DISTINCT ON (c.album_id) c.album_id, c.id, left(c.blob_sha256, 16) AS key
            FROM covers c
            WHERE c.album_id IS NOT NULL AND c.trashed_at IS NULL
              AND ($1::uuid[] IS NULL OR c.album_id = ANY($1))
            ORDER BY c.album_id, c.position, c.id
        ), track_cover AS (
            SELECT DISTINCT ON (at.album_id) at.album_id, c.id, left(c.blob_sha256, 16) AS key
            FROM album_tracks at
            JOIN covers c ON c.track_id = at.track_id AND c.trashed_at IS NULL
            WHERE $1::uuid[] IS NULL OR at.album_id = ANY($1)
            ORDER BY at.album_id, at.disc_number NULLS FIRST, at.track_number NULLS LAST, c.position, c.id
        ), counts AS (
            SELECT at.album_id, count(*) AS tracks, COALESCE(sum(f.duration_ms), 0) AS duration_ms
            FROM album_tracks at
            JOIN tracks t ON t.id = at.track_id AND t.merged_into IS NULL
            JOIN files f ON f.id = t.file_id
            WHERE $1::uuid[] IS NULL OR at.album_id = ANY($1)
            GROUP BY at.album_id
        )
        SELECT COALESCE(json_agg(json_build_array(
                   a.id, a.title, a.kind, COALESCE(cr.display, ''), COALESCE(cr.credits, '[]'::json),
                   a.release_date,
                   CASE WHEN a.release_date ~ '^\d{4}' THEN substr(a.release_date, 1, 4)::int END,
                   COALESCE(oc.id, tc.id), CASE WHEN oc.id IS NOT NULL THEN oc.key ELSE tc.key END,
                   a.created_at,
                   COALESCE(n.tracks, 0), COALESCE(n.duration_ms, 0)
               ) ORDER BY a.id), '[]'::json)::text AS "rows!"
        FROM albums a
        LEFT JOIN credits cr ON cr.album_id = a.id
        LEFT JOIN own_cover oc ON oc.album_id = a.id
        LEFT JOIN track_cover tc ON tc.album_id = a.id
        LEFT JOIN counts n ON n.album_id = a.id
        WHERE a.merged_into IS NULL AND ($1::uuid[] IS NULL OR a.id = ANY($1))
        "#,
        ids as Option<&[Uuid]>
    )
    .fetch_one(conn)
    .await
}

pub async fn artists_json(conn: &mut PgConnection, ids: Option<&[Uuid]>) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH aliases AS (
            SELECT artist_id, json_agg(alias ORDER BY lower(alias), alias) AS aliases
            FROM artist_aliases
            WHERE merged_into IS NULL AND ($1::uuid[] IS NULL OR artist_id = ANY($1))
            GROUP BY artist_id
        ), track_counts AS (
            SELECT c.artist_id, count(DISTINCT c.track_id) AS n
            FROM track_credits c
            JOIN tracks t ON t.id = c.track_id AND t.merged_into IS NULL
            WHERE $1::uuid[] IS NULL OR c.artist_id = ANY($1)
            GROUP BY c.artist_id
        ), album_counts AS (
            SELECT c.artist_id, count(DISTINCT c.album_id) AS n
            FROM album_credits c
            JOIN albums a ON a.id = c.album_id AND a.merged_into IS NULL
            WHERE $1::uuid[] IS NULL OR c.artist_id = ANY($1)
            GROUP BY c.artist_id
        )
        SELECT COALESCE(json_agg(json_build_array(
                   a.id, a.name, a.disambiguation, COALESCE(al.aliases, '[]'::json),
                   COALESCE(tn.n, 0), COALESCE(an.n, 0)
               ) ORDER BY a.id), '[]'::json)::text AS "rows!"
        FROM artists a
        LEFT JOIN aliases al ON al.artist_id = a.id
        LEFT JOIN track_counts tn ON tn.artist_id = a.id
        LEFT JOIN album_counts an ON an.artist_id = a.id
        WHERE a.merged_into IS NULL AND ($1::uuid[] IS NULL OR a.id = ANY($1))
        "#,
        ids as Option<&[Uuid]>
    )
    .fetch_one(conn)
    .await
}

/// Only annotations that say something (played, rated or starred).
pub async fn annotations_json(conn: &mut PgConnection, ids: Option<&[Uuid]>) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        SELECT COALESCE(json_agg(json_build_array(
                   CASE WHEN track_id IS NOT NULL THEN 'track'
                        WHEN album_id IS NOT NULL THEN 'album' ELSE 'artist' END,
                   COALESCE(track_id, album_id, artist_id),
                   play_count, last_played_at, rating,
                   CASE WHEN starred THEN starred_at END
               ) ORDER BY id), '[]'::json)::text AS "rows!"
        FROM annotations
        WHERE user_id = current_app_user()
          AND (play_count > 0 OR rating > 0 OR starred)
          AND ($1::uuid[] IS NULL OR COALESCE(track_id, album_id, artist_id) = ANY($1))
        "#,
        ids as Option<&[Uuid]>
    )
    .fetch_one(conn)
    .await
}

// ---- Changes since a client's copy (`?since=`) ----

/// Where the library is now, as a version for clients: a Postgres snapshot ("which
/// transactions had committed", as `xmin:xmax:running,…`). A later delta is exactly the
/// changes that weren't visible in it: none missed (a transaction still running at the
/// time is in it as running), none sent twice, whatever else runs on the server. Taken
/// before reading the catalog.
pub async fn version(conn: &mut PgConnection) -> sqlx::Result<String> {
    sqlx::query_scalar!(r#"SELECT pg_current_snapshot()::text AS "version!""#).fetch_one(conn).await
}

/// What changed since a version.
#[derive(Debug, Default)]
pub struct Changes {
    /// Send the whole catalog: who may see what changed, the log was trimmed past
    /// `since`, or it's too much for a delta to be worth it.
    pub everything: bool,
    pub tracks: Vec<Uuid>,
    pub albums: Vec<Uuid>,
    pub artists: Vec<Uuid>,
    /// Things the user's own plays, stars or ratings changed on.
    pub annotations: Vec<Uuid>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        !self.everything && self.tracks.is_empty() && self.albums.is_empty() && self.artists.is_empty() && self.annotations.is_empty()
    }
}

/// Above this many changed things, the whole catalog is sent instead (it's about as
/// quick to build, and a delta that size is no smaller).
const DELTA_LIMIT: i64 = 50_000;

/// What changed since `since` (a `version`). Errors if it isn't one (a 400).
pub async fn changes_since(conn: &mut PgConnection, since: &str) -> anyhow::Result<Changes> {
    if !since.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b',') {
        return Err(super::revisions::EditRejected::Invalid("`since` isn't a catalog version".into()).into());
    }
    // Older than what the log keeps: the whole catalog.
    let trimmed = sqlx::query_scalar!(
        r#"SELECT pg_snapshot_xmin($1::text::pg_snapshot) < before AS "older!" FROM library_changes_trimmed"#,
        since
    )
    .fetch_optional(&mut *conn)
    .await?;
    if trimmed == Some(true) {
        return Ok(Changes { everything: true, ..Changes::default() });
    }
    let rows = sqlx::query!(
        r#"
        SELECT kind AS "kind!", entity_id
        FROM library_changes
        WHERE xid >= pg_snapshot_xmin($1::text::pg_snapshot)
          AND NOT pg_visible_in_snapshot(xid, $1::text::pg_snapshot)
        GROUP BY kind, entity_id
        LIMIT $2
        "#,
        since,
        DELTA_LIMIT + 1
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() as i64 > DELTA_LIMIT || rows.iter().any(|r| r.kind == "everything") {
        return Ok(Changes { everything: true, ..Changes::default() });
    }
    let mut changes = Changes::default();
    for row in rows {
        let Some(id) = row.entity_id else { continue };
        match row.kind.as_str() {
            "track" => changes.tracks.push(id),
            "album" => changes.albums.push(id),
            "artist" => changes.artists.push(id),
            "annotation" => changes.annotations.push(id),
            _ => {}
        }
    }
    Ok(changes)
}

/// Of these, the ones merged into another (visible to the user): the client drops them.
/// (Something that became hidden isn't here: that logs `everything`, and the client gets
/// the whole catalog, so a delta never names it.)
pub async fn merged(conn: &mut PgConnection, tracks: &[Uuid], albums: &[Uuid], artists: &[Uuid]) -> sqlx::Result<(Vec<Uuid>, Vec<Uuid>, Vec<Uuid>)> {
    let tracks = sqlx::query_scalar!("SELECT id FROM tracks WHERE id = ANY($1) AND merged_into IS NOT NULL", tracks)
        .fetch_all(&mut *conn)
        .await?;
    let albums = sqlx::query_scalar!("SELECT id FROM albums WHERE id = ANY($1) AND merged_into IS NOT NULL", albums)
        .fetch_all(&mut *conn)
        .await?;
    let artists = sqlx::query_scalar!("SELECT id FROM artists WHERE id = ANY($1) AND merged_into IS NOT NULL", artists)
        .fetch_all(&mut *conn)
        .await?;
    Ok((tracks, albums, artists))
}

/// Deletes changes older than `days` days, noting how far (clients with an older copy
/// then get the whole catalog). Run as the owner, at startup.
pub async fn trim_changes(conn: &mut PgConnection, days: i32) -> sqlx::Result<u64> {
    let before = sqlx::query_scalar!(
        r#"SELECT max(xid)::text::bigint AS "before" FROM library_changes WHERE at < now() - make_interval(days => $1)"#,
        days
    )
    .fetch_one(&mut *conn)
    .await?;
    let Some(before) = before else { return Ok(0) };
    let deleted = sqlx::query!("DELETE FROM library_changes WHERE xid <= $1::text::xid8", before.to_string())
        .execute(&mut *conn)
        .await?
        .rows_affected();
    sqlx::query!(
        "INSERT INTO library_changes_trimmed (before) VALUES (($1::bigint + 1)::text::xid8)
         ON CONFLICT (one) DO UPDATE SET before = EXCLUDED.before",
        before
    )
    .execute(&mut *conn)
    .await?;
    Ok(deleted)
}

