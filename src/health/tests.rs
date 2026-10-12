//! The health checks against a small library in a temporary blob store.

use std::path::Path;

use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::{Context, Finding, Health, Progress, run_check};
use crate::db;
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource};
use crate::storage::BlobStore;

struct Setup {
    owner: PgPool,
    pool: PgPool,
    store: BlobStore,
    sha: String,
    _dir: tempfile::TempDir,
}

async fn setup(connect: PgConnectOptions) -> anyhow::Result<Setup> {
    let owner = PgPoolOptions::new().connect_with(connect.clone()).await?;
    let pool = db::test_app_pool(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let file_name = "01 - Glass Harbor.flac";
    let options = IngestOptions {
        user: None,
        uploaded_filename: file_name.into(),
        origin: sources::validate(Kind::Track, NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": 7 }) })?,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: Default::default(),
        local_path: None,
        edited_after: false,
        tags_from: None,
        volume: None,
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file_name);
    let pending = ingest::receive_path(&store, &path, "main").await?;
    ingest::ingest_with_report(&pool, &store, pending, &options, false).await?;
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM files").fetch_one(&owner).await?;
    Ok(Setup { owner, pool, store, sha, _dir: dir })
}

impl Setup {
    fn context(&self) -> Context {
        Context::new(self.pool.clone(), self.store.clone(), std::sync::Arc::new(Progress::default()))
    }
}

async fn found(s: &Setup, id: &str) -> Vec<Finding> {
    run_check(id, &s.context()).await.unwrap()
}

fn messages(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.message.as_str()).collect()
}

#[sqlx::test]
async fn a_healthy_library_has_nothing_to_report(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    for check in super::CHECKS {
        assert_eq!(messages(&found(&s, check.id).await), Vec::<&str>::new(), "{}", check.id);
    }
    Ok(())
}

#[sqlx::test]
async fn unused_files_and_missing_files_are_found(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    // A file nothing refers to (an upload that failed after storing it).
    let stray = s.store.put_bytes(b"left behind").await?;
    let unused = found(&s, "blobs-unused").await;
    assert_eq!(unused.len(), 1, "{:?}", messages(&unused));
    assert!(unused[0].message.starts_with(&stray), "{}", unused[0].message);
    // Only reported: it is still there.
    assert!(s.store.find(&stray).await.is_some());

    // The track's file gone from the disk.
    let (_, path) = s.store.find(&s.sha).await.unwrap();
    let mut permissions = std::fs::metadata(&path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions)?;
    std::fs::remove_file(&path)?;
    let present = found(&s, "blobs-present").await;
    assert_eq!(present.len(), 1, "{:?}", messages(&present));
    assert_eq!(present[0].severity, super::Severity::Problem);
    assert!(present[0].message.contains(&s.sha));
    Ok(())
}

#[sqlx::test]
async fn a_damaged_file_is_found_by_reading_it(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let (_, path) = s.store.find(&s.sha).await.unwrap();
    let mut permissions = std::fs::metadata(&path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions)?;
    let mut bytes = std::fs::read(&path)?;
    bytes[100] ^= 0xff;
    std::fs::write(&path, bytes)?;
    // Listings alone don't see it.
    assert!(found(&s, "blobs-present").await.is_empty());
    let damaged = found(&s, "blobs-intact").await;
    assert_eq!(damaged.len(), 1, "{:?}", messages(&damaged));
    assert!(damaged[0].message.contains(&s.sha));
    Ok(())
}

#[sqlx::test]
async fn staged_images_are_unused_only_after_a_day(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    sqlx::query("INSERT INTO users (username, password_hash) VALUES ('carol', 'x')").execute(&s.owner).await?;
    let user: uuid::Uuid = sqlx::query_scalar("SELECT id FROM users WHERE username = 'carol'").fetch_one(&s.owner).await?;
    let recent = s.store.put_bytes(b"recent image").await?;
    let old = s.store.put_bytes(b"old image").await?;
    for (sha, age) in [(&recent, "1 hour"), (&old, "3 days")] {
        sqlx::query("INSERT INTO staged_covers (user_id, blob_sha256, staged_at) VALUES ($1, $2, now() - $3::interval)")
            .bind(user)
            .bind(sha)
            .bind(age)
            .execute(&s.owner)
            .await?;
    }
    let staged = found(&s, "staged-covers").await;
    assert_eq!(staged.len(), 1, "{:?}", messages(&staged));
    assert!(staged[0].message.contains(&old) && staged[0].message.contains("carol"));
    // Staged images aren't "unused" while they wait for their edit.
    assert!(found(&s, "blobs-unused").await.is_empty());
    Ok(())
}

#[sqlx::test]
async fn a_run_reports_what_it_found_and_one_runs_at_a_time(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    s.store.put_bytes(b"left behind").await?;
    let health = std::sync::Arc::new(Health::default());
    assert!(health.start(s.pool.clone(), s.store.clone(), &["nonsense".into()], "alice".into()).is_err());
    health.start(s.pool.clone(), s.store.clone(), &["blobs-unused".into()], "alice".into()).unwrap();
    assert!(health.start(s.pool.clone(), s.store.clone(), &["blobs-unused".into()], "alice".into()).is_err(), "one at a time");
    let report = loop {
        if let Some(last) = health.status().last {
            break last;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(report.by, "alice");
    assert_eq!(report.results.len(), 1);
    assert_eq!((report.results[0].id, report.results[0].total, report.results[0].error.is_none()), ("blobs-unused", 1, true));
    assert!(health.status().running.is_none());
    Ok(())
}
