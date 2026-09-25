//! Review flags: things a person should look at, noticed by ingest (possible duplicates,
//! ambiguous artist names) or left by a merge (tracks whose credits it moved). Nothing is
//! changed automatically; resolving a flag just records that someone looked (and what
//! they did).

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

/// How similar a title must be (pg_trgm `similarity`, 0 to 1) to count as a possible
/// duplicate. "Northbound" vs "Northbound (Live)" is about 0.6.
const DUPLICATE_SIMILARITY: f32 = 0.5;

/// Flags a track whose artist name from the tags matched several artists; `chosen` (the
/// oldest) was credited.
pub async fn flag_ambiguous_artist(
    conn: &mut PgConnection,
    track_id: Uuid,
    name: &str,
    chosen: Uuid,
    artists: &[Uuid],
) -> sqlx::Result<()> {
    let related = json!({ "name": name, "chosen": chosen, "artists": artists });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('ambiguous_artist', $1, $2)",
        track_id,
        related
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Flags a track whose credits a merge moved from `from` to `into`, so someone can check
/// the result (e.g. "A feat. B" merged into A: B may need crediting again).
pub async fn flag_merged_artist(
    conn: &mut PgConnection,
    track_id: Uuid,
    from: Uuid,
    from_name: &str,
    into: Uuid,
) -> sqlx::Result<()> {
    let related = json!({ "from": from, "from_name": from_name, "into": into, "artists": [from, into] });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('merged_artist', $1, $2)",
        track_id,
        related
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Flags a new track if tracks by the same artists have a similar title.
///
/// Starts from the artists' tracks rather than from all similar titles: an artist has at
/// most thousands of tracks, while a short title like "Intro" is similar to far more.
pub async fn flag_possible_duplicates(
    conn: &mut PgConnection,
    track_id: Uuid,
    title: &str,
    artist_ids: &[Uuid],
) -> sqlx::Result<()> {
    let similar = sqlx::query_scalar!(
        r#"
        SELECT t.id FROM tracks t
        WHERE t.id IN (SELECT track_id FROM track_credits WHERE artist_id = ANY($3))
          AND t.id <> $1
          AND t.merged_into IS NULL
          AND similarity(t.title, $2) >= $4
        ORDER BY similarity(t.title, $2) DESC, t.id
        LIMIT 10
        "#,
        track_id,
        title,
        artist_ids,
        DUPLICATE_SIMILARITY,
    )
    .fetch_all(&mut *conn)
    .await?;
    if similar.is_empty() {
        return Ok(());
    }
    let related = json!({ "tracks": similar });
    sqlx::query!(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2)",
        track_id,
        related
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Flag {
    pub id: Uuid,
    pub kind: String,
    pub track_id: Uuid,
    pub track_title: String,
    pub track_artist: String,
    /// As stored (see the migration), minus the `tracks` / `artists` id lists: those are
    /// expanded below instead, which leaves out the ones the user can't see.
    pub related: serde_json::Value,
    /// For possible duplicates: the similar tracks, `[{id, title, artist_display}]`.
    pub related_tracks: serde_json::Value,
    /// For ambiguous artists: the candidates; for merges: both artists.
    /// `[{id, name, disambiguation}]`.
    pub related_artists: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub resolution: String,
}

/// Open (or resolved) flags, newest first, a page at a time (`before` = last id seen).
pub async fn list(
    conn: &mut PgConnection,
    resolved: bool,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<Flag>> {
    sqlx::query_as!(
        Flag,
        r#"
        SELECT f.id, f.kind, f.track_id, t.title AS track_title, t.artist_display AS track_artist,
               f.related - 'tracks' - 'artists' AS "related!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'id', rt.id, 'title', rt.title, 'artist_display', rt.artist_display))
                   FROM tracks rt
                   WHERE rt.id IN (SELECT (jsonb_array_elements_text(f.related -> 'tracks'))::uuid)
               ), '[]') AS "related_tracks!",
               COALESCE((
                   SELECT jsonb_agg(jsonb_build_object(
                       'id', a.id, 'name', a.name, 'disambiguation', a.disambiguation))
                   FROM artists a
                   WHERE a.id IN (SELECT (jsonb_array_elements_text(f.related -> 'artists'))::uuid)
               ), '[]') AS "related_artists!",
               f.created_at, f.resolved_at, u.username AS "resolved_by?", f.resolution
        FROM review_flags f
        JOIN tracks t ON t.id = f.track_id
        LEFT JOIN users u ON u.id = f.resolved_by
        WHERE (f.resolved_at IS NOT NULL) = $1
          AND f.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY f.id DESC
        LIMIT $3
        "#,
        resolved,
        before,
        limit
    )
    .fetch_all(conn)
    .await
}

/// Marks a flag as resolved by the transaction's user. False if there's no such open flag.
pub async fn resolve(conn: &mut PgConnection, id: Uuid, resolution: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE review_flags
        SET resolved_at = now(), resolved_by = current_app_user(), resolution = $2
        WHERE id = $1 AND resolved_at IS NULL
        "#,
        id,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}
