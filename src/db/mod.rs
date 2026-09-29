//! Database access. Queries are checked against the real schema at compile time
//! by the `sqlx::query!` macros (they read `DATABASE_URL` from `.env`).
//!
//! Functions take `&mut PgConnection`, so they run inside whatever transaction the caller
//! started with `begin`. Call them with `&mut tx`: a `Transaction` derefs to the
//! connection it wraps.

pub mod access;
pub mod album_merges;
pub mod annotations;
pub mod blobs;
pub mod catalog;
pub mod covers;
pub mod groupings;
pub mod groups;
pub mod ingest;
pub mod library;
pub mod playlists;
pub mod review;
pub mod review_bulk;
pub mod credited_as;
pub mod og_files;
pub mod revisions;
pub mod shares;
pub mod sources;
pub mod state;
pub mod track_files;
pub mod tracks;
pub mod problems;
pub mod trash;
pub mod users;

#[cfg(test)]
mod rls_tests;
#[cfg(test)]
mod trash_tests;
#[cfg(test)]
mod merge_tests;
#[cfg(test)]
mod ingest_tests;
#[cfg(test)]
mod annotation_tests;
#[cfg(test)]
mod navidrome_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod album_merge_tests;
#[cfg(test)]
mod playlist_tests;
#[cfg(test)]
mod library_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod artist_merge_tests;
#[cfg(test)]
mod origin_tests;
#[cfg(test)]
mod groupings_tests;
#[cfg(test)]
mod review_bulk_tests;
#[cfg(test)]
mod credited_as_tests;
#[cfg(test)]
mod og_files_tests;

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

// ---- Row-level security ----
//
// Restricted tracks are hidden by Postgres itself (migration 0008), but Postgres doesn't
// apply that to the owner of the tables, which is who we connect as. So the server's
// connections switch to the role `acetate_app` right after connecting, and everything the
// server does is filtered. Migrations and the CLI keep the owner's role: they're the
// system, which sees everything.

/// The role the server works as.
pub const APP_ROLE: &str = "acetate_app";

/// Tables without row-level security. They hold nothing about the library that could be
/// hidden (users, groups, sessions, shares, fixed vocabularies), and the code that uses
/// them decides who sees what. Every other table must have RLS; see `check_row_level_security`.
const TABLES_WITHOUT_RLS: &[&str] = &[
    "_sqlx_migrations",
    // One row: how far the change log was trimmed (see migration 0029).
    "library_changes_trimmed",
    // What the server keeps between runs (see `state`): nothing about what users see.
    "server_state",
    "users",
    "groups",
    "user_groups",
    "permissions",
    "group_permissions",
    "sessions",
    "shares",
    "track_statuses",
    "credit_roles",
    "album_kinds",
    "relation_kinds",
    "review_kinds",
];

/// Connects as the owner: for migrations and the CLI.
pub async fn connect_owner(url: &str) -> sqlx::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        // Without a limit, an unreachable database means half a minute of silence.
        .acquire_timeout(Duration::from_secs(5))
        .connect(url)
        .await
}

/// Connects the server's pool. Every connection switches to `APP_ROLE` as soon as it's
/// opened, so there's no request that could forget to.
pub async fn connect_app(url: &str) -> sqlx::Result<PgPool> {
    let pool = app_pool_options().connect(url).await?;

    // Belt and braces: make sure it worked.
    let role = sqlx::query_scalar!(r#"SELECT current_user::text AS "role!""#)
        .fetch_one(&pool)
        .await?;
    assert_eq!(role, APP_ROLE, "server connections must run as {APP_ROLE}");
    Ok(pool)
}

/// The server's pool settings (also used by the tests, to connect like the server does).
pub fn app_pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        // Runs for each new connection. It has to return a boxed future (the pool stores
        // the callback without knowing its exact type), hence `Box::pin(async move { .. })`.
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SET ROLE acetate_app").execute(&mut *conn).await?;
                // No JIT compiling: the app's queries are short lookups, and a plan that looks
                // expensive (RLS subqueries inflate the estimate) spent seconds compiling
                // for a query that then ran in milliseconds.
                sqlx::query("SET jit = off").execute(&mut *conn).await?;
                Ok(())
            })
        })
}

/// Checked when the server starts (with the owner's pool), so a mistake shows up at once
/// instead of as a leak:
/// * every table has row-level security, except the ones in `TABLES_WITHOUT_RLS`;
/// * the app role can't skip it (not a superuser, no BYPASSRLS).
///
/// Also warns if the search operators aren't LEAKPROOF (migration 0008): search then
/// still works but reads every row.
pub async fn check_row_level_security(pool: &PgPool) -> anyhow::Result<()> {
    let without_rls: Vec<&str> = TABLES_WITHOUT_RLS.to_vec();
    let unprotected = sqlx::query_scalar!(
        r#"
        SELECT c.relname::text AS "name!"
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public' AND c.relkind = 'r' AND NOT c.relrowsecurity
          AND c.relname::text <> ALL ($1)
        ORDER BY 1
        "#,
        &without_rls as &[&str]
    )
    .fetch_all(pool)
    .await?;
    if !unprotected.is_empty() {
        anyhow::bail!(
            "tables without row-level security: {unprotected:?}. Enable it in a migration \
             (or, if the table holds nothing that could be hidden, add it to \
             TABLES_WITHOUT_RLS in src/db/mod.rs)"
        );
    }

    let skips_rls = sqlx::query_scalar!(
        r#"SELECT rolsuper OR rolbypassrls AS "skips!" FROM pg_roles WHERE rolname = $1"#,
        APP_ROLE
    )
    .fetch_optional(pool)
    .await?;
    match skips_rls {
        None => anyhow::bail!("the role {APP_ROLE} doesn't exist (migration 0008 creates it)"),
        Some(true) => anyhow::bail!("the role {APP_ROLE} must not be a superuser or have BYPASSRLS"),
        Some(false) => {}
    }

    let not_leakproof = sqlx::query_scalar!(
        r#"
        SELECT proname::text AS "name!" FROM pg_proc
        WHERE proname IN ('word_similarity_op', 'texticlike') AND NOT proleakproof
        "#
    )
    .fetch_all(pool)
    .await?;
    if !not_leakproof.is_empty() {
        tracing::warn!(
            "{not_leakproof:?} not marked LEAKPROOF: search can't use its indexes and will be \
             slow on a big library (see migration 0008; needs a superuser)"
        );
    }
    Ok(())
}

/// Lets the rest of the transaction ignore row-level security, until `end` is called.
///
/// Only for *cascades*: when a change to something the user can see must also update rows
/// they can't (e.g. renaming an artist updates the artist text of hidden tracks too), or a
/// check must count hidden rows (e.g. "is this file already stored?"). Nothing read while
/// it's active may be returned to the user. Keep the span short and call `end` right after.
///
/// If the transaction ends first (error, rollback), the role goes back by itself: it's set
/// with `set_config(.., true)`, which only lasts until the end of the transaction.
pub async fn as_system(conn: &mut PgConnection) -> sqlx::Result<SystemScope> {
    let previous_role = sqlx::query_scalar!(r#"SELECT current_user::text AS "role!""#)
        .fetch_one(&mut *conn)
        .await?;
    // 'none' = the role we connected as (the owner).
    sqlx::query_scalar!("SELECT set_config('role', 'none', true)")
        .fetch_one(&mut *conn)
        .await?;
    Ok(SystemScope { previous_role })
}

/// Returned by `as_system`. `#[must_use]`: the compiler warns if it's dropped without `end`.
#[must_use = "call .end(conn) to go back to row-level security"]
pub struct SystemScope {
    previous_role: String,
}

impl SystemScope {
    /// Goes back to the role from before `as_system` (the app role in the server; the
    /// owner in the CLI, where nothing changes).
    pub async fn end(self, conn: &mut PgConnection) -> sqlx::Result<()> {
        sqlx::query_scalar!("SELECT set_config('role', $1, true)", self.previous_role)
            .fetch_one(conn)
            .await?;
        Ok(())
    }
}

/// Starts a transaction acting as `user`.
///
/// `None` = no user. In the CLI, whose pool connects as the owner, that's the system and
/// sees everything. On the server's pool (`connect_app`) it would only see what everyone
/// sees; it isn't used there (guests have no permissions, so they never get this far).
///
/// All database work on behalf of a user goes through this. It tells Postgres who the
/// user is (`app.user_id`, see migration 0002), which fills in "created by" columns and
/// is what row-level security filters on. So code using the transaction
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
/// migration 0003). Row-level security uses it to open up exactly the
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
