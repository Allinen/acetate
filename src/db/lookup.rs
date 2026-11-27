//! "Is this already in the library?" for tools outside Acetate (`POST /api/lookup/…`,
//! `GET /api/tracks/search`) and for `acetate verify`: by file content, by a platform's
//! id, by title. Read-only; as the user asking, so only what they can see is found.

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

/// What has a file with these bytes. Each list is `[]` when nothing does.
#[derive(Debug, Serialize)]
pub struct FileHit {
    pub sha256: String,
    /// Tracks whose file it is now (`current`) or was before it was replaced:
    /// `[{track_id, file_id, current, trashed, purged}]` (a merged track: the one it went into).
    pub tracks: serde_json::Value,
    /// `[{extra_id, file_id, extra_name, file_name, hidden}]`.
    pub extras: serde_json::Value,
    /// Covers with these bytes, not in the trash: `[{cover_id, track_id, album_id}]`.
    pub covers: serde_json::Value,
    /// Uploads that didn't import and are still open on the Review page: `[{id, kind, uploaded_filename, reason}]`.
    pub problems: serde_json::Value,
}

/// For each hash (lowercase hex), in the order given.
pub async fn files(conn: &mut PgConnection, shas: &[String]) -> sqlx::Result<Vec<FileHit>> {
    sqlx::query_as!(
        FileHit,
        r#"
        SELECT h.sha256 AS "sha256!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'track_id', final_track(o.track_id), 'file_id', f.id, 'current', o.current,
                       'trashed', f.trashed_at IS NOT NULL, 'purged', f.purged_at IS NOT NULL) ORDER BY f.id, o.current DESC)
                   FROM files f
                   CROSS JOIN LATERAL (
                       SELECT t.id AS track_id, true AS current FROM tracks t WHERE t.file_id = f.id
                       UNION
                       SELECT fh.track_id, false FROM track_file_history fh WHERE fh.old_file_id = f.id
                   ) o
                   WHERE f.sha256 = h.sha256
               ), '[]') AS "tracks!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'extra_id', e.id, 'file_id', x.id, 'extra_name', e.name, 'file_name', x.name,
                       'hidden', e.hidden_at IS NOT NULL OR x.hidden_at IS NOT NULL) ORDER BY x.id)
                   FROM extra_files x JOIN extras e ON e.id = x.extra_id
                   WHERE x.sha256 = h.sha256
               ), '[]') AS "extras!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object('cover_id', c.id, 'track_id', c.track_id, 'album_id', c.album_id) ORDER BY c.id)
                   FROM covers c
                   WHERE c.blob_sha256 = h.sha256 AND c.trashed_at IS NULL
               ), '[]') AS "covers!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object('id', p.id, 'kind', p.kind, 'uploaded_filename', p.uploaded_filename, 'reason', p.reason))
                   FROM ingest_problems p
                   WHERE p.sha256 = h.sha256 AND p.resolved_at IS NULL
               ), '[]') AS "problems!"
        FROM unnest($1::text[]) WITH ORDINALITY AS h(sha256, n)
        ORDER BY h.n
        "#,
        shas
    )
    .fetch_all(conn)
    .await
}

/// For `acetate verify`, per hash: (its bytes are kept as a track's file, an extra's file
/// or a cover; kept for the Review page because it didn't import). A purged file's bytes
/// are gone, so it doesn't count.
pub async fn stored(conn: &mut PgConnection, shas: &[String]) -> sqlx::Result<Vec<(bool, bool)>> {
    let rows = sqlx::query!(
        r#"
        SELECT h.n AS "n!",
               (EXISTS (SELECT 1 FROM files f WHERE f.sha256 = h.sha256 AND f.purged_at IS NULL)
                OR EXISTS (SELECT 1 FROM extra_files x WHERE x.sha256 = h.sha256)
                OR EXISTS (SELECT 1 FROM covers c WHERE c.blob_sha256 = h.sha256 AND c.trashed_at IS NULL)) AS "in_library!",
               EXISTS (SELECT 1 FROM ingest_problems p
                       WHERE p.sha256 = h.sha256 AND p.resolved_at IS NULL) AS "for_review!"
        FROM unnest($1::text[]) WITH ORDINALITY AS h(sha256, n)
        ORDER BY h.n
        "#,
        shas
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.in_library, r.for_review)).collect())
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformId {
    pub platform: String,
    pub external_id: String,
}

/// What has a source with this platform id (sources in the trash don't count).
#[derive(Debug, Serialize)]
pub struct SourceHit {
    pub platform: String,
    pub external_id: String,
    /// `[{kind: "track" | "album" | "artist" | "grouping", id}]`, merged ones as the one
    /// they went into, each once.
    pub owners: serde_json::Value,
}

/// For each platform id, in the order given.
pub async fn sources(conn: &mut PgConnection, ids: &[PlatformId]) -> sqlx::Result<Vec<SourceHit>> {
    let platforms: Vec<String> = ids.iter().map(|i| i.platform.clone()).collect();
    let external_ids: Vec<String> = ids.iter().map(|i| i.external_id.clone()).collect();
    sqlx::query_as!(
        SourceHit,
        r#"
        SELECT q.platform AS "platform!", q.external_id AS "external_id!",
               COALESCE((
                   SELECT jsonb_agg(DISTINCT jsonb_build_object('kind', o.kind, 'id', o.id))
                   FROM sources s
                   CROSS JOIN LATERAL (
                       SELECT CASE WHEN s.track_id IS NOT NULL THEN 'track' WHEN s.album_id IS NOT NULL THEN 'album'
                                   WHEN s.artist_id IS NOT NULL THEN 'artist' ELSE 'grouping' END AS kind,
                              COALESCE(final_track(s.track_id), final_album(s.album_id), final_artist(s.artist_id), s.grouping_id) AS id
                   ) o
                   WHERE s.platform = q.platform AND s.external_id = q.external_id AND s.trashed_at IS NULL
               ), '[]') AS "owners!"
        FROM unnest($1::text[], $2::text[]) WITH ORDINALITY AS q(platform, external_id, n)
        ORDER BY q.n
        "#,
        &platforms,
        &external_ids
    )
    .fetch_all(conn)
    .await
}

#[derive(Debug, Serialize)]
pub struct TrackHit {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub release_date: Option<String>,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub sha256: String,
    /// How well the title matched, 0–1 (1: it contains the words as typed).
    pub score: f32,
}

/// Tracks by title (and, if given, by the artist as displayed), best first. Merged
/// tracks aren't listed (the one they went into is).
pub async fn search_tracks(conn: &mut PgConnection, query: &str, artist: Option<&str>, limit: i64) -> sqlx::Result<Vec<TrackHit>> {
    let artist = artist.map(str::trim).filter(|a| !a.is_empty());
    sqlx::query_as!(
        TrackHit,
        r#"
        SELECT t.id, t.title, t.artist_display, t.release_date, t.status, f.duration_ms, f.sha256,
               GREATEST(CASE WHEN t.title ILIKE $2 THEN 1 ELSE 0 END, word_similarity($1, t.title))::real AS "score!"
        FROM tracks t JOIN files f ON f.id = t.file_id
        WHERE ($1 <% t.title OR t.title ILIKE $2) AND t.merged_into IS NULL
          AND ($3::text IS NULL OR $3 <% t.artist_display OR t.artist_display ILIKE $4)
        ORDER BY 8 DESC, t.title, t.id
        LIMIT $5
        "#,
        query,
        super::catalog::like_pattern(query),
        artist,
        artist.map(super::catalog::like_pattern),
        limit
    )
    .fetch_all(conn)
    .await
}
