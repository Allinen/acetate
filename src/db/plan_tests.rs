//! Lookups under row-level security use their indexes (see migration 0019): a condition
//! Postgres can't prove leak-free is checked only after the policy, on every row.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;

/// The plan of a query run as `user` (so through the RLS policies), with sequential scans
/// made as expensive as possible: a scan that remains means no index could be used.
async fn plan(app: &sqlx::PgPool, user: uuid::Uuid, sql: &'static str, param: &str) -> anyhow::Result<String> {
    let mut tx = db::begin(app, Some(user)).await?;
    sqlx::query("SET LOCAL enable_seqscan = off").execute(&mut *tx).await?;
    let explain = format!("EXPLAIN {sql}");
    let lines: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(explain))
        .bind(param)
        .fetch_all(&mut *tx)
        .await?;
    Ok(lines.join("\n"))
}

#[sqlx::test]
async fn lookups_use_indexes_under_rls(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let id = lib.v.to_string();
    let lookups = [
        ("SELECT id FROM sources WHERE (track_id = $1::uuid OR album_id = $1::uuid OR artist_id = $1::uuid)", id.as_str()),
        ("SELECT id FROM artists WHERE name_lower = lower($1)", "Some Name"),
        ("SELECT id FROM artist_aliases WHERE alias_lower = lower($1)", "Some Name"),
        ("SELECT id FROM albums WHERE title_lower = lower($1)", "Some Title"),
    ];
    for (sql, param) in lookups {
        let plan = plan(&lib.app, lib.carol, sql, param).await?;
        // The table itself (the first line) is read through an index; the policy's
        // subqueries have their own.
        let first = plan.lines().next().unwrap_or_default();
        assert!(!first.contains("Seq Scan"), "{sql} reads the whole table:\n{plan}");
    }

    // What the migration fixed: the old forms can't use an index under RLS.
    let old = plan(
        &lib.app,
        lib.carol,
        "SELECT id FROM artists WHERE lower(name) = lower($1)",
        "Some Name",
    )
    .await?;
    assert!(old.lines().next().unwrap_or_default().contains("Seq Scan"), "{old}");
    Ok(())
}
