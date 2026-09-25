//! Which volume each blob is on (migration 0018). Only the system uses this, so every
//! function here switches to it (see `db::as_system`).

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
