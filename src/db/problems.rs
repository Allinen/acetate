//! Files that didn't import (see migration 0014): refused or failed, with their audio kept.
//! Listed on the Review page; resolved by hand, or by themselves when the same file
//! imports fine later.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

pub struct NewProblem<'a> {
    /// "skipped" (the file or its description was refused) or "failed" (our error).
    pub kind: &'a str,
    pub sha256: &'a str,
    pub size_bytes: i64,
    pub uploaded_filename: &'a str,
    pub path: Option<&'a str>,
    pub reason: &'a str,
    /// Everything it was imported with (see migration 0015).
    pub description: serde_json::Value,
    pub embedded_tags: Option<serde_json::Value>,
    /// What the tags did have, to fill in the form for the rest.
    pub tag_title: Option<&'a str>,
    pub tag_artists: &'a [String],
}

/// Records a file that didn't import, or, if it's already listed (the same file tried
/// again), updates its entry. As the system: the entry may be someone else's.
pub async fn record(conn: &mut PgConnection, problem: &NewProblem<'_>) -> sqlx::Result<Uuid> {
    let system = super::as_system(conn).await?;
    let id = sqlx::query_scalar!(
        r#"
        INSERT INTO ingest_problems (kind, sha256, size_bytes, uploaded_filename, path, reason,
                                     description, embedded_tags, tag_title, tag_artists)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (sha256) WHERE resolved_at IS NULL DO UPDATE SET
            kind = EXCLUDED.kind,
            reason = EXCLUDED.reason,
            uploaded_filename = EXCLUDED.uploaded_filename,
            path = EXCLUDED.path,
            description = EXCLUDED.description,
            embedded_tags = COALESCE(EXCLUDED.embedded_tags, ingest_problems.embedded_tags),
            tag_title = EXCLUDED.tag_title,
            tag_artists = EXCLUDED.tag_artists,
            attempts = ingest_problems.attempts + 1,
            last_attempt_at = now()
        RETURNING id
        "#,
        problem.kind,
        problem.sha256,
        problem.size_bytes,
        problem.uploaded_filename,
        problem.path,
        problem.reason,
        problem.description,
        problem.embedded_tags,
        problem.tag_title,
        problem.tag_artists,
    )
    .fetch_one(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(id)
}

/// The file with this hash is in the library now (imported, or found to be there
/// already): resolves its open entry, if any. Called by ingest, as the system.
pub async fn resolve_imported(
    conn: &mut PgConnection,
    sha256: &str,
    track_id: Uuid,
    resolution: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        UPDATE ingest_problems
        SET resolved_at = now(), resolved_by = current_app_user(), resolution = $3, track_id = $2
        WHERE sha256 = $1 AND resolved_at IS NULL
        "#,
        sha256,
        track_id,
        resolution
    )
    .execute(conn)
    .await?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Problem {
    pub id: Uuid,
    pub kind: String,
    pub sha256: String,
    pub size_bytes: i64,
    pub uploaded_filename: String,
    pub path: Option<String>,
    pub reason: String,
    pub description: Option<serde_json::Value>,
    pub embedded_tags: Option<serde_json::Value>,
    pub tag_title: Option<String>,
    pub tag_artists: Vec<String>,
    pub attempts: i32,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub last_attempt_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by: Option<String>,
    pub resolution: String,
    /// The track the file became, if it imported later.
    pub track_id: Option<Uuid>,
}

/// Open (or resolved) entries the user can see, newest first, a page at a time
/// (`before` = the last id seen).
pub async fn list(
    conn: &mut PgConnection,
    resolved: bool,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<Problem>> {
    sqlx::query_as!(
        Problem,
        r#"
        SELECT p.id, p.kind, p.sha256, p.size_bytes, p.uploaded_filename, p.path, p.reason,
               p.description, p.embedded_tags, p.tag_title, p.tag_artists, p.attempts, p.created_at, cu.username AS "created_by?",
               p.last_attempt_at, p.resolved_at, ru.username AS "resolved_by?", p.resolution,
               p.track_id
        FROM ingest_problems p
        LEFT JOIN users cu ON cu.id = p.created_by
        LEFT JOIN users ru ON ru.id = p.resolved_by
        WHERE (p.resolved_at IS NOT NULL) = $1
          AND p.id < COALESCE($2, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY p.id DESC
        LIMIT $3
        "#,
        resolved,
        before,
        limit
    )
    .fetch_all(conn)
    .await
}

/// Marks an entry done by hand ("fixed the tags and imported it again", "junk"...).
/// False if there's no such open entry the user can see.
pub async fn resolve(conn: &mut PgConnection, id: Uuid, resolution: &str) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE ingest_problems
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

/// An open entry's file and what it was imported with, for importing it again.
pub struct OpenProblem {
    pub sha256: String,
    pub size_bytes: i64,
    pub uploaded_filename: String,
    pub path: Option<String>,
    pub description: Option<serde_json::Value>,
}

/// An open entry the user can see.
pub async fn get_open(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<OpenProblem>> {
    sqlx::query_as!(
        OpenProblem,
        r#"
        SELECT sha256, size_bytes, uploaded_filename, path, description
        FROM ingest_problems
        WHERE id = $1 AND resolved_at IS NULL
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

/// The kept audio of an entry the user can see: (sha256, uploaded filename).
pub async fn audio(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<(String, String)>> {
    let row = sqlx::query!(
        "SELECT sha256, uploaded_filename FROM ingest_problems WHERE id = $1",
        id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| (r.sha256, r.uploaded_filename)))
}
