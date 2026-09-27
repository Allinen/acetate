//! The whole library as one user sees it, in one go (`GET /api/catalog`): what a client
//! needs to browse, sort and search locally without asking the server for every list.
//!
//! Each part is built by Postgres itself as JSON (`json_agg`), a row as an array in the
//! order of its `*_COLUMNS` (keys would be repeated 185k times otherwise). The queries are
//! set-based: each related table is grouped once and joined, rather than a subquery per
//! row. Row-level security decides what's in it, as everywhere; merged things are left
//! out (they only redirect).

use sqlx::PgConnection;

/// A track's columns, in the order of its row arrays.
/// `credits`: `[[artist_id, role, alias], …]` in credit order; `alias` is the name it's
/// credited under when that's an alias, else null.
/// `albums`: `[[album_id, disc, track], …]`, every album it's on, its main one first (a real
/// album before a single, then the oldest). `cover_id`: its own first cover, else its main
/// album's.
pub const TRACK_COLUMNS: &[&str] = &[
    "id", "title", "artist_display", "credits", "albums", "duration_ms",
    "file_id", "cover_id", "release_date", "year", "status", "genres", "created_at",
    "is_restricted", "codec",
];

/// `credits` as for tracks; `artist_display` with aliases as credited.
/// `cover_id`: its own first cover, else its first track's.
pub const ALBUM_COLUMNS: &[&str] = &[
    "id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id",
    "created_at", "track_count", "duration_ms",
];

pub const ARTIST_COLUMNS: &[&str] = &["id", "name", "disambiguation", "aliases", "track_count", "album_count"];

/// The current user's annotations. `kind`: "track", "album" or "artist".
pub const ANNOTATION_COLUMNS: &[&str] = &["kind", "id", "play_count", "last_played_at", "rating", "starred_at"];

pub async fn tracks_json(conn: &mut PgConnection) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH credits AS (
            SELECT c.track_id,
                   json_agg(json_build_array(c.artist_id, c.role, al.alias)
                            ORDER BY c.position, c.role) AS credits
            FROM track_credits c
            LEFT JOIN artist_aliases al ON al.id = c.alias_id
            GROUP BY c.track_id
        ), memberships AS (
            SELECT at.track_id,
                   json_agg(json_build_array(at.album_id, at.disc_number, at.track_number)
                            ORDER BY a.kind = 'single', a.id) AS albums,
                   (array_agg(at.album_id ORDER BY a.kind = 'single', a.id))[1] AS main_album
            FROM album_tracks at
            JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
            GROUP BY at.track_id
        ), track_cover AS (
            SELECT DISTINCT ON (c.track_id) c.track_id, c.id
            FROM covers c
            WHERE c.track_id IS NOT NULL AND c.trashed_at IS NULL
            ORDER BY c.track_id, c.position, c.id
        ), album_cover AS (
            SELECT DISTINCT ON (c.album_id) c.album_id, c.id
            FROM covers c
            WHERE c.album_id IS NOT NULL AND c.trashed_at IS NULL
            ORDER BY c.album_id, c.position, c.id
        )
        SELECT COALESCE(json_agg(json_build_array(
                   t.id, t.title, t.artist_display, COALESCE(cr.credits, '[]'::json),
                   COALESCE(m.albums, '[]'::json), f.duration_ms,
                   t.file_id, COALESCE(tc.id, ac.id), t.release_date, t.release_year, t.status,
                   t.genres, t.created_at, t.is_restricted, f.codec
               ) ORDER BY t.id), '[]'::json)::text AS "rows!"
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        LEFT JOIN credits cr ON cr.track_id = t.id
        LEFT JOIN memberships m ON m.track_id = t.id
        LEFT JOIN track_cover tc ON tc.track_id = t.id
        LEFT JOIN album_cover ac ON ac.album_id = m.main_album
        WHERE t.merged_into IS NULL
        "#
    )
    .fetch_one(conn)
    .await
}

pub async fn albums_json(conn: &mut PgConnection) -> sqlx::Result<String> {
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
            GROUP BY c.album_id
        ), own_cover AS (
            SELECT DISTINCT ON (c.album_id) c.album_id, c.id
            FROM covers c
            WHERE c.album_id IS NOT NULL AND c.trashed_at IS NULL
            ORDER BY c.album_id, c.position, c.id
        ), track_cover AS (
            SELECT DISTINCT ON (at.album_id) at.album_id, c.id
            FROM album_tracks at
            JOIN covers c ON c.track_id = at.track_id AND c.trashed_at IS NULL
            ORDER BY at.album_id, at.disc_number NULLS FIRST, at.track_number NULLS LAST, c.position, c.id
        ), counts AS (
            SELECT at.album_id, count(*) AS tracks, COALESCE(sum(f.duration_ms), 0) AS duration_ms
            FROM album_tracks at
            JOIN tracks t ON t.id = at.track_id AND t.merged_into IS NULL
            JOIN files f ON f.id = t.file_id
            GROUP BY at.album_id
        )
        SELECT COALESCE(json_agg(json_build_array(
                   a.id, a.title, a.kind, COALESCE(cr.display, ''), COALESCE(cr.credits, '[]'::json),
                   a.release_date,
                   CASE WHEN a.release_date ~ '^\d{4}' THEN substr(a.release_date, 1, 4)::int END,
                   COALESCE(oc.id, tc.id), a.created_at,
                   COALESCE(n.tracks, 0), COALESCE(n.duration_ms, 0)
               ) ORDER BY a.id), '[]'::json)::text AS "rows!"
        FROM albums a
        LEFT JOIN credits cr ON cr.album_id = a.id
        LEFT JOIN own_cover oc ON oc.album_id = a.id
        LEFT JOIN track_cover tc ON tc.album_id = a.id
        LEFT JOIN counts n ON n.album_id = a.id
        WHERE a.merged_into IS NULL
        "#
    )
    .fetch_one(conn)
    .await
}

pub async fn artists_json(conn: &mut PgConnection) -> sqlx::Result<String> {
    sqlx::query_scalar!(
        r#"
        WITH aliases AS (
            SELECT artist_id, json_agg(alias ORDER BY lower(alias), alias) AS aliases
            FROM artist_aliases
            WHERE merged_into IS NULL
            GROUP BY artist_id
        ), track_counts AS (
            SELECT c.artist_id, count(DISTINCT c.track_id) AS n
            FROM track_credits c
            JOIN tracks t ON t.id = c.track_id AND t.merged_into IS NULL
            GROUP BY c.artist_id
        ), album_counts AS (
            SELECT c.artist_id, count(DISTINCT c.album_id) AS n
            FROM album_credits c
            JOIN albums a ON a.id = c.album_id AND a.merged_into IS NULL
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
        WHERE a.merged_into IS NULL
        "#
    )
    .fetch_one(conn)
    .await
}

/// Only annotations that say something (played, rated or starred).
pub async fn annotations_json(conn: &mut PgConnection) -> sqlx::Result<String> {
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
        "#
    )
    .fetch_one(conn)
    .await
}
