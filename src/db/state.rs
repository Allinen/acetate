//! What the server keeps between runs (the `server_state` table, migration 0033): a value
//! under a name, as JSON, for anything that should survive a restart. Read and written
//! whole; entries are never removed.
//!
//! Names in use:
//! * `uploads`: today's upload count (`api::upload_limits`);
//! * `rate_limits`: recent failed logins and share lookups (`api::rate_limit`);
//! * `instance_id`: this server's identity, a uuid made by migration 0061 and never changed.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::PgConnection;

/// The value kept under `key`, if there is one (and it still reads as a `T`).
pub async fn get<T: DeserializeOwned>(conn: &mut PgConnection, key: &str) -> sqlx::Result<Option<T>> {
    let value = sqlx::query_scalar!("SELECT value FROM server_state WHERE key = $1", key).fetch_optional(conn).await?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

/// Keeps `value` under `key` (replacing what was there).
pub async fn set<T: Serialize>(conn: &mut PgConnection, key: &str, value: &T) -> sqlx::Result<()> {
    let value = serde_json::to_value(value).map_err(|err| sqlx::Error::Encode(Box::new(err)))?;
    sqlx::query!(
        "INSERT INTO server_state (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        key,
        value
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// This server's identity (see migration 0061): the same across restarts, restores and
/// address changes.
pub async fn instance_id(pool: &sqlx::PgPool) -> anyhow::Result<uuid::Uuid> {
    let mut conn = pool.acquire().await?;
    let id: String = get(&mut conn, "instance_id").await?.ok_or_else(|| anyhow::anyhow!("the server has no instance id (migration 0061)"))?;
    Ok(id.parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Count {
        day: String,
        bytes: u64,
    }

    /// As the app role (what requests use), so it checks the grants too.
    #[sqlx::test]
    async fn a_value_kept_read_and_replaced(_: sqlx::postgres::PgPoolOptions, connect: sqlx::postgres::PgConnectOptions) -> anyhow::Result<()> {
        let lib = crate::db::rls_tests::setup(connect).await?;
        let mut tx = crate::db::begin(&lib.app, None).await?;
        assert_eq!(get::<Count>(&mut tx, "test").await?, None);
        set(&mut tx, "test", &Count { day: "2026-09-29".into(), bytes: 5 }).await?;
        set(&mut tx, "test", &Count { day: "2026-09-29".into(), bytes: 7 }).await?;
        assert_eq!(get::<Count>(&mut tx, "test").await?, Some(Count { day: "2026-09-29".into(), bytes: 7 }));
        // Something else under the name: not a `Count`, so none.
        set(&mut tx, "test", &"text").await?;
        assert_eq!(get::<Count>(&mut tx, "test").await?, None);
        // Kept, not removed.
        assert!(sqlx::query("DELETE FROM server_state").execute(&mut *tx).await.is_err());
        Ok(())
    }

    /// The migration makes it; the app role reads it, and it stays the same.
    #[sqlx::test]
    async fn the_server_has_an_identity(_: sqlx::postgres::PgPoolOptions, connect: sqlx::postgres::PgConnectOptions) -> anyhow::Result<()> {
        let lib = crate::db::rls_tests::setup(connect).await?;
        let id = instance_id(&lib.app).await?;
        assert_eq!(instance_id(&lib.app).await?, id);
        assert!(!id.is_nil());
        Ok(())
    }
}
