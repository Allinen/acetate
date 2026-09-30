//! Which volume each blob is on (migration 0018). Only the system uses this, so every
//! function here switches to it (see `db::as_system`).

use serde::Serialize;
use sqlx::PgConnection;

/// Records that a blob is stored on `volume`. If it's recorded already, that stays (a blob
/// is only ever stored once, on the volume it went to first).
pub async fn register(conn: &mut PgConnection, sha256: &str, volume: &str, size_bytes: Option<i64>) -> sqlx::Result<()> {
    let system = super::as_system(conn).await?;
    sqlx::query!(
        "INSERT INTO blobs (sha256, volume, size_bytes) VALUES ($1, $2, $3) ON CONFLICT (sha256) DO NOTHING",
        sha256,
        volume,
        size_bytes
    )
    .execute(&mut *conn)
    .await?;
    system.end(conn).await
}

/// A blob was deleted (purged): forget it.
pub async fn forget(conn: &mut PgConnection, sha256: &str) -> sqlx::Result<()> {
    let system = super::as_system(conn).await?;
    sqlx::query!("DELETE FROM blobs WHERE sha256 = $1", sha256)
        .execute(&mut *conn)
        .await?;
    system.end(conn).await
}

/// The volumes blobs are recorded on (to check they're all configured).
pub async fn volumes_in_use(conn: &mut PgConnection) -> sqlx::Result<Vec<(String, i64, i64)>> {
    let system = super::as_system(conn).await?;
    let rows = sqlx::query!(
        r#"SELECT volume, count(*) AS "blobs!", COALESCE(sum(size_bytes), 0)::bigint AS "bytes!"
           FROM blobs GROUP BY volume ORDER BY volume"#
    )
    .fetch_all(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(rows.into_iter().map(|r| (r.volume, r.blobs, r.bytes)).collect())
}

/// What's stored, per volume and kind: "audio" (tracks' files, current or previous, not
/// purged), "covers", "extras" (their files and thumbnails), "not imported" (files that
/// didn't import, kept), else "other" (nothing refers to it anymore).
#[derive(Debug, Serialize)]
pub struct Total {
    pub volume: String,
    pub kind: String,
    pub blobs: i64,
    /// Of those with a known size.
    pub bytes: i64,
    /// Blobs without a known size (`acetate measure-covers` measures them).
    pub unmeasured: i64,
}

pub async fn totals(conn: &mut PgConnection) -> sqlx::Result<Vec<Total>> {
    let system = super::as_system(conn).await?;
    let totals = sqlx::query_as!(
        Total,
        r#"
        SELECT b.volume, k.kind AS "kind!", count(*) AS "blobs!", COALESCE(sum(b.size_bytes), 0)::bigint AS "bytes!",
               count(*) FILTER (WHERE b.size_bytes IS NULL) AS "unmeasured!"
        FROM blobs b
        CROSS JOIN LATERAL (SELECT CASE
            WHEN EXISTS (SELECT 1 FROM files f WHERE f.sha256 = b.sha256 AND f.purged_at IS NULL) THEN 'audio'
            WHEN EXISTS (SELECT 1 FROM covers c WHERE c.blob_sha256 = b.sha256) THEN 'covers'
            WHEN EXISTS (SELECT 1 FROM extra_files e WHERE e.sha256 = b.sha256)
              OR EXISTS (SELECT 1 FROM extra_files e WHERE e.thumb_sha256 = b.sha256) THEN 'extras'
            WHEN EXISTS (SELECT 1 FROM ingest_problems p WHERE (p.sha256 = b.sha256 OR b.sha256 = ANY (p.cover_blobs))
                                                          AND p.audio_purged_at IS NULL) THEN 'not imported'
            ELSE 'other'
        END AS kind) k
        GROUP BY b.volume, k.kind
        ORDER BY b.volume, k.kind
        "#
    )
    .fetch_all(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(totals)
}
