//! Library tooling, end to end (same setup as `rls_tests`): the history's alias names,
//! and what's stored per volume.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::revisions::Source;
use crate::storage::BlobStore;

#[sqlx::test]
async fn history_names_aliases_not_listed_anymore(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // V credits Both as "B0th", then the alias is taken off the list (hidden, kept).
    let mut tx = db::begin(&lib.owner, None).await?;
    let alias = db::revisions::ensure_alias(&mut tx, lib.both, "B0th", Source::Edit, "").await?;
    let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(lib.v).fetch_one(&mut *tx).await?;
    let (_, mut meta) = db::revisions::latest_internal(&mut tx, lib.v, variant).await?.unwrap();
    meta.credits[0].alias_id = Some(alias);
    db::revisions::write_internal(&mut tx, lib.v, variant, None, &meta, Source::Edit, "", false).await?;
    sqlx::query("UPDATE artist_aliases SET listed = false WHERE id = $1").bind(alias).execute(&mut *tx).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let track = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert_eq!(track.alias_names.unwrap().get(&alias).map(String::as_str), Some("B0th"));
    let without_history = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
    assert!(without_history.alias_names.is_none());
    Ok(())
}

#[sqlx::test]
async fn totals_say_whats_stored_per_volume(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let cover = store.put_bytes(b"an image").await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::covers::add(&mut tx, CoverOwner::Track(lib.v), &NewCover { blob_sha256: &cover, mime: None, width: None, height: None, source: "upload" }).await?;
    tx.commit().await?;
    let v_sha: String = sqlx::query_scalar("SELECT f.sha256 FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::blobs::register(&mut tx, &v_sha, "main", Some(1000)).await?;
    db::blobs::register(&mut tx, &cover, "main", None).await?;
    db::blobs::register(&mut tx, &"f".repeat(64), "archive", Some(5)).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let totals: Vec<(String, String, i64, i64, i64)> =
        db::blobs::totals(&mut tx).await?.into_iter().map(|t| (t.volume, t.kind, t.blobs, t.bytes, t.unmeasured)).collect();
    assert_eq!(
        totals,
        [
            ("archive".into(), "other".into(), 1, 5, 0),
            ("main".into(), "audio".into(), 1, 1000, 0),
            ("main".into(), "covers".into(), 1, 0, 1),
        ]
    );
    Ok(())
}
