//! Pointing a file's origin at another source of its track (same setup as `rls_tests`).
//!
//! V's file came from a custom "test" source (as `make_track` makes every track). V gets
//! a NetEase source too; carol points the file there, and back.

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use super::sources::OriginTarget;
use crate::db;
use crate::model::sources::{self, Kind, NewSource, SourceOwner};

async fn file_and_origin(conn: &mut sqlx::PgConnection, track: Uuid) -> anyhow::Result<(Uuid, Uuid)> {
    Ok(sqlx::query_as("SELECT f.id, f.origin_snapshot_id FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1")
        .bind(track)
        .fetch_one(&mut *conn)
        .await?)
}

#[sqlx::test]
async fn a_file_comes_from_another_source_of_its_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let netease = sources::validate(
        Kind::Track,
        NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": 5 }) },
    )?;
    let added = db::sources::add_snapshot(&mut tx, SourceOwner::Track(lib.v), &netease).await?;
    // O's own source: not V's.
    let o_source: Uuid = sqlx::query_scalar("SELECT id FROM sources WHERE track_id = $1").bind(lib.o).fetch_one(&mut *tx).await?;
    let (h_file, _) = file_and_origin(&mut tx, lib.h).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let (file, custom) = file_and_origin(&mut tx, lib.v).await?;
    let change = db::sources::set_file_origin(&mut tx, file, OriginTarget::Source(added.source_id), "it was NetEase").await?;
    assert!(change.is_some());
    assert_eq!(file_and_origin(&mut tx, lib.v).await?.1, added.snapshot_id);
    // Again: nothing to change.
    assert_eq!(db::sources::set_file_origin(&mut tx, file, OriginTarget::Source(added.source_id), "").await?, None);

    // The change is kept, named, and shown with the track.
    let changes = db::sources::origin_changes(&mut tx, file).await?;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].reason, "it was NetEase");
    assert_eq!(changes[0].old.as_ref().map(|o| o.snapshot_id), Some(custom));
    assert_eq!(changes[0].new.as_ref().map(|o| o.platform.as_str()), Some("netease"));
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert_eq!(v.origin_changes.map(|c| c.len()), Some(1));
    assert_eq!(v.file.origin.map(|o| o.platform), Some("netease".to_string()));

    // The source it comes from now can't go to the trash; the old one can.
    assert!(db::sources::trash(&mut tx, added.source_id).await.is_err());
    let custom_source: Uuid = sqlx::query_scalar("SELECT source_id FROM source_snapshots WHERE id = $1").bind(custom).fetch_one(&mut *tx).await?;
    assert!(db::sources::trash(&mut tx, custom_source).await?);
    // Back to a source in the trash: refused, until it's taken out.
    let err = db::sources::set_file_origin(&mut tx, file, OriginTarget::Snapshot(custom), "").await.unwrap_err();
    assert!(err.to_string().contains("in the trash"), "{err}");
    sqlx::query("UPDATE sources SET trashed_at = NULL, trashed_by = NULL WHERE id = $1").bind(custom_source).execute(&mut *tx).await?;
    // Undone: pointed back at the snapshot from before, a change of its own.
    db::sources::set_file_origin(&mut tx, file, OriginTarget::Snapshot(custom), "undo").await?;
    assert_eq!(file_and_origin(&mut tx, lib.v).await?.1, custom);
    assert_eq!(db::sources::origin_changes(&mut tx, file).await?.len(), 2);

    // Another track's source, or a file carol can't see: refused.
    let err = db::sources::set_file_origin(&mut tx, file, OriginTarget::Source(o_source), "").await.unwrap_err();
    assert!(err.to_string().contains("its own track"), "{err}");
    assert!(db::sources::set_file_origin(&mut tx, h_file, OriginTarget::Source(added.source_id), "").await.is_err());
    tx.commit().await?;

    // Files stay unchangeable otherwise.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("UPDATE files SET uploaded_filename = 'x' WHERE id = $1").bind(file).execute(&mut *tx).await.is_err());
    Ok(())
}
