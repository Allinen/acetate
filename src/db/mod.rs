//! Database access. Queries are checked against the real schema at compile time
//! by the `sqlx::query!` macros (they read `DATABASE_URL` from `.env`).
//!
//! Functions take `&mut PgConnection`, so they run inside whatever transaction the caller
//! started with `begin`. Call them with `&mut tx`: a `Transaction` derefs to the
//! connection it wraps.

pub mod catalog;
pub mod ingest;
pub mod shares;
pub mod sources;
pub mod tracks;
pub mod users;

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// Starts a transaction acting as `user` (None = the system, e.g. the CLI).
///
/// All database work on behalf of a user goes through this. It tells Postgres who the
/// user is (`app.user_id`, see migration 0002), which fills in "created by" columns and
/// is what row-level security (Phase 3) will filter on. So code using the transaction
/// never has to pass the user along itself.
///
/// Commit to keep writes. Read-only callers can just drop it, which rolls back.
///
/// (`'static`: a transaction from a pool isn't borrowing anything, it owns its connection.)
pub async fn begin(pool: &PgPool, user: Option<Uuid>) -> sqlx::Result<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await?;
    if let Some(user_id) = user {
        // `true`: the setting only lasts until the end of this transaction.
        // (It's a SELECT, so it returns the new value, which we don't need.)
        sqlx::query_scalar!(
            "SELECT set_config('app.user_id', $1, true)",
            user_id.to_string()
        )
        .fetch_one(&mut *tx)
        .await?;
    }
    Ok(tx)
}

/// Starts a transaction serving a share link (for whoever opened it, usually a guest).
///
/// Like `begin` for users, it tells Postgres which share this is (`app.share_id`, see
/// migration 0003). Row-level security (Phase 3) will use it to open up exactly the
/// share's track or album, even when restricted, and nothing else.
pub async fn begin_share(pool: &PgPool, share_id: Uuid) -> sqlx::Result<Transaction<'static, Postgres>> {
    let mut tx = pool.begin().await?;
    sqlx::query_scalar!(
        "SELECT set_config('app.share_id', $1, true)",
        share_id.to_string()
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok(tx)
}
