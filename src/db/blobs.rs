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

// ---- Mirrors (`GET /api/mirror`, migration 0059) ----

/// A blob as a mirror sees it: where it is and what it's for now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MirrorEntry {
    pub sha256: String,
    /// The volume it's on (a mirror may leave a volume out); None when it's gone.
    pub volume: Option<String>,
    /// None when not measured yet (some thumbnails), or gone.
    pub size: Option<i64>,
    /// What it's for, the first that applies: "audio" (a track plays it), "extra" (an
    /// extra's file or thumbnail, also hidden ones), "cover", "not_imported" (a file that
    /// didn't import, or a cover sent with one, still open), "previous" (a track's earlier
    /// file), "trashed" (a file, cover or resolved not-imported file in the trash),
    /// "other" (stored, nothing refers to it), "gone" (deleted from the store: purged).
    pub state: &'static str,
}

const STATES: [&str; 7] = ["audio", "extra", "cover", "not_imported", "previous", "trashed", "other"];

/// Every stored blob (`only` None), or those of `only` (then also ones gone), with their
/// states. Ranks each thing referring to a blob (1 = "audio" … 6 = "trashed") and takes
/// the best; nothing referring to it: "other". One pass over each table, grouped by hash
/// (not a lookup per blob: the whole list is hundreds of thousands).
async fn mirror_entries(conn: &mut PgConnection, only: Option<&[String]>) -> sqlx::Result<Vec<MirrorEntry>> {
    let system = super::as_system(conn).await?;
    let rows = sqlx::query!(
        r#"
        WITH refs (sha256, rank) AS (
            SELECT f.sha256,
                   CASE WHEN f.trashed_at IS NOT NULL THEN 6
                        WHEN EXISTS (SELECT 1 FROM tracks t WHERE t.file_id = f.id) THEN 1
                        ELSE 5 END
            FROM files f
            WHERE f.purged_at IS NULL AND ($1::text[] IS NULL OR f.sha256 = ANY ($1))
            UNION ALL
            SELECT sha256, 2 FROM extra_files WHERE $1::text[] IS NULL OR sha256 = ANY ($1)
            UNION ALL
            SELECT thumb_sha256, 2 FROM extra_files
            WHERE thumb_sha256 IS NOT NULL AND ($1::text[] IS NULL OR thumb_sha256 = ANY ($1))
            UNION ALL
            SELECT c.blob_sha256, CASE WHEN c.trashed_at IS NULL THEN 3 ELSE 6 END
            FROM covers c
            WHERE $1::text[] IS NULL OR c.blob_sha256 = ANY ($1)
            UNION ALL
            -- Open, or resolved by hand and in the trash (one that imported later has its
            -- file and covers elsewhere).
            SELECT s, CASE WHEN p.resolved_at IS NULL THEN 4 ELSE 6 END
            FROM ingest_problems p, unnest(p.cover_blobs || p.sha256) s
            WHERE p.audio_purged_at IS NULL AND (p.resolved_at IS NULL OR p.track_id IS NULL)
              AND ($1::text[] IS NULL OR s = ANY ($1))
        ),
        wanted (sha256) AS (
            SELECT sha256 FROM blobs WHERE $1::text[] IS NULL
            UNION ALL
            SELECT DISTINCT unnest($1::text[])
        )
        SELECT w.sha256 AS "sha256!", b.volume AS "volume?", b.size_bytes AS "size?", b.sha256 IS NOT NULL AS "stored!",
               r.rank AS "rank?"
        FROM wanted w
        LEFT JOIN blobs b ON b.sha256 = w.sha256
        LEFT JOIN (SELECT sha256, min(rank) AS rank FROM refs GROUP BY sha256) r ON r.sha256 = w.sha256
        ORDER BY w.sha256
        "#,
        only as Option<&[String]>
    )
    .fetch_all(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(rows
        .into_iter()
        .map(|r| MirrorEntry {
            sha256: r.sha256,
            volume: r.volume,
            size: r.size,
            state: match (r.stored, r.rank) {
                (false, _) => "gone",
                (true, None) => "other",
                (true, Some(rank)) => STATES[(rank.clamp(1, 6) - 1) as usize],
            },
        })
        .collect())
}

/// Every stored blob, with its state.
pub async fn mirror_all(conn: &mut PgConnection) -> sqlx::Result<Vec<MirrorEntry>> {
    mirror_entries(conn, None).await
}

/// The blobs that changed since `since` (a version, see `db::library::version`), with
/// their states now; None if the log was trimmed past it (then the whole list is needed).
/// Errors if `since` isn't a version (a 400).
pub async fn mirror_changed_since(conn: &mut PgConnection, since: &str) -> anyhow::Result<Option<Vec<MirrorEntry>>> {
    if !since.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b',') || since.is_empty() {
        return Err(super::revisions::EditRejected::Invalid("`since` isn't a mirror version".into()).into());
    }
    let system = super::as_system(conn).await?;
    let trimmed = sqlx::query_scalar!(
        r#"SELECT pg_snapshot_xmin($1::text::pg_snapshot) < before AS "older!" FROM blob_changes_trimmed"#,
        since
    )
    .fetch_optional(&mut *conn)
    .await?;
    if trimmed == Some(true) {
        system.end(conn).await?;
        return Ok(None);
    }
    let changed = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT sha256 AS "sha256!"
        FROM blob_changes
        WHERE xid >= pg_snapshot_xmin($1::text::pg_snapshot)
          AND NOT pg_visible_in_snapshot(xid, $1::text::pg_snapshot)
        "#,
        since
    )
    .fetch_all(&mut *conn)
    .await?;
    system.end(conn).await?;
    if changed.is_empty() {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(mirror_entries(conn, Some(&changed)).await?))
}

/// Deletes blob changes older than `days` days, noting how far (mirrors with an older
/// version then get the whole list). Run as the owner, at startup.
pub async fn trim_changes(conn: &mut PgConnection, days: i32) -> sqlx::Result<u64> {
    let before = sqlx::query_scalar!(
        r#"SELECT max(xid)::text::bigint AS "before" FROM blob_changes WHERE at < now() - make_interval(days => $1)"#,
        days
    )
    .fetch_one(&mut *conn)
    .await?;
    let Some(before) = before else { return Ok(0) };
    let deleted = sqlx::query!("DELETE FROM blob_changes WHERE xid <= $1::text::xid8", before.to_string())
        .execute(&mut *conn)
        .await?
        .rows_affected();
    sqlx::query!(
        "INSERT INTO blob_changes_trimmed (before) VALUES (($1::bigint + 1)::text::xid8)
         ON CONFLICT (one) DO UPDATE SET before = EXCLUDED.before",
        before
    )
    .execute(&mut *conn)
    .await?;
    Ok(deleted)
}
