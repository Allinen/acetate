//! Lyrics history, the trash and purging, end to end (same setup as `rls_tests`: a fresh
//! database per test, acting as users through a pool like the server's).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::revisions::{EditRejected, Source};
use crate::db::trash::Kind;
use crate::model::LyricsData;
use crate::model::sources::{self, NewSource, SourceOwner};
use crate::storage::BlobStore;

/// Pretends something was trashed `days` ago (tests can't wait a week).
async fn age(lib: &Library, table: &str, id: Uuid, days: i32) -> anyhow::Result<()> {
    let sql = match table {
        "covers" => "UPDATE covers SET trashed_at = now() - make_interval(days => $1) WHERE id = $2",
        "files" => "UPDATE files SET trashed_at = now() - make_interval(days => $1) WHERE id = $2",
        other => panic!("no such table here: {other}"),
    };
    sqlx::query(sql).bind(days).bind(id).execute(&lib.owner).await?;
    Ok(())
}

fn rejected_message(err: &anyhow::Error) -> String {
    match err.downcast_ref::<EditRejected>() {
        Some(rejected) => rejected.to_string(),
        None => panic!("expected a rejection, got: {err:#}"),
    }
}

#[sqlx::test]
async fn lyrics_are_revisioned(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let first = LyricsData {
        body: "hello\nworld".into(),
        ..Default::default()
    };
    let (id, rev0) = db::revisions::create_lyrics(&mut tx, lib.v, "manual", &first, Source::Edit, "").await?;

    let synced = LyricsData {
        body: "[00:01.00] hello\n[00:02.00] world".into(),
        is_official: true,
        language: Some("en".into()),
        removed: false,
    };
    let rev1 = db::revisions::write_lyrics(&mut tx, id, Some(rev0.revision_id), &synced, Source::Edit, "").await?;
    assert!(rev1.changed);
    let again = db::revisions::write_lyrics(&mut tx, id, Some(rev1.revision_id), &synced, Source::Edit, "").await?;
    assert!(!again.changed, "the same data writes nothing");
    let stale = db::revisions::write_lyrics(&mut tx, id, Some(rev0.revision_id), &first, Source::Edit, "").await;
    assert!(matches!(stale.unwrap_err().downcast_ref(), Some(EditRejected::Stale { .. })));

    let is_synced: bool = sqlx::query_scalar("SELECT is_synced FROM lyrics WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    assert!(is_synced, "derived from the body");

    db::revisions::restore_lyrics_revision(&mut tx, rev0.revision_id, "").await?;
    let track = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    let lyrics = track.lyrics.iter().find(|l| l.id == id).unwrap();
    assert_eq!(lyrics.body, "hello\nworld");
    assert!(!lyrics.is_synced);
    assert_eq!(lyrics.revisions.as_ref().unwrap().len(), 3, "created, edited, restored");
    Ok(())
}

#[sqlx::test]
async fn lyrics_are_removed_and_brought_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let data = LyricsData { body: "on the wrong track".into(), ..Default::default() };
    let (id, first) = db::revisions::create_lyrics(&mut tx, lib.v, "manual", &data, Source::Edit, "").await?;
    let removed = LyricsData { removed: true, ..data.clone() };
    assert!(db::revisions::create_lyrics(&mut tx, lib.v, "manual", &removed, Source::Edit, "").await.is_err(), "not new and removed");

    let gone = db::revisions::remove_lyrics(&mut tx, id, Some(first.revision_id), "wrong track").await?;
    assert!(gone.changed);
    let track = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert!(track.lyrics.is_empty());
    assert_eq!(track.removed_lyrics.iter().map(|l| (l.id, l.body.as_str())).collect::<Vec<_>>(), [(id, "on the wrong track")]);
    assert!(!db::revisions::remove_lyrics(&mut tx, id, None, "").await?.changed, "removed already");

    // The revision from before brings it back.
    db::revisions::restore_lyrics_revision(&mut tx, first.revision_id, "").await?;
    let track = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert_eq!(track.lyrics.len(), 1);
    assert!(track.removed_lyrics.is_empty());
    Ok(())
}

#[sqlx::test]
async fn covers_purge_only_when_nothing_uses_the_blob(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let sha = store.put_bytes(b"not really an image").await?;

    // A cover on V, copied to the album: two covers, one blob.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let cover = NewCover { blob_sha256: &sha, mime: None, width: None, height: None, source: "upload" };
    let on_track = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover).await?;
    let on_album = db::covers::copy(&mut tx, CoverOwner::Album(lib.album), on_track).await?;
    assert!(db::covers::trash(&mut tx, on_track).await?);
    tx.commit().await?;

    // Only admins purge, and only after the grace period, and with confirmation.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let err = db::trash::purge(&mut tx, &store, &[(Kind::Cover, on_track)], &sha[..8]).await.unwrap_err();
    assert!(rejected_message(&err).contains("can be purged from"), "{err}");
    drop(tx);
    age(&lib, "covers", on_track, 8).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let err = db::trash::purge(&mut tx, &store, &[(Kind::Cover, on_track)], "abc").await.unwrap_err();
    assert!(rejected_message(&err).contains("to confirm"), "{err}");
    let purged = db::trash::purge(&mut tx, &store, &[(Kind::Cover, on_track)], &sha[..8]).await?;
    tx.commit().await?;
    assert_eq!(purged.items, 1);
    assert!(purged.delete_blobs.is_empty(), "the album's copy still uses the blob");

    // Purge the copy too: now the blob goes.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::covers::trash(&mut tx, on_album).await?);
    tx.commit().await?;
    age(&lib, "covers", on_album, 8).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let purged = db::trash::purge(&mut tx, &store, &[(Kind::Cover, on_album)], &sha).await?;
    tx.commit().await?;
    assert_eq!(purged.delete_blobs, vec![sha.clone()]);
    assert_eq!(purged.bytes_freed, 19);

    let logged: i64 = sqlx::query_scalar("SELECT count(*) FROM purge_log WHERE kind = 'cover'")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(logged, 2);
    // carol isn't an admin: the log isn't hers to read.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM purge_log").fetch_one(&mut *tx).await?;
    assert_eq!(seen, 0);
    Ok(())
}

#[sqlx::test]
async fn old_files_become_tombstones(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // V's file is replaced by a new one: the old one is now a previous file.
    let mut tx = db::begin(&lib.owner, None).await?;
    let old_file = db::ingest::lock_track_file(&mut tx, lib.v).await?.unwrap();
    let new_file = add_file(&mut tx, lib.v, "V, upgraded").await?;
    db::ingest::replace_track_file(&mut tx, lib.v, old_file, new_file, "upgrade").await?;
    tx.commit().await?;
    let old_sha: String = sqlx::query_scalar("SELECT sha256 FROM files WHERE id = $1")
        .bind(old_file)
        .fetch_one(&lib.owner)
        .await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let replaced = db::trash::replaced_files(&mut tx, None, 100).await?;
    assert_eq!(replaced.iter().map(|f| f.id).collect::<Vec<_>>(), vec![old_file]);
    // A file a track plays can't be trashed; the whole request is refused.
    let err = db::trash::trash_files(&mut tx, &[old_file, new_file]).await.unwrap_err();
    assert!(rejected_message(&err).contains("currently plays"), "{err}");
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    assert_eq!(db::trash::trash_files(&mut tx, &[old_file]).await?, 1);
    tx.commit().await?;
    age(&lib, "files", old_file, 8).await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::trash::purge(&mut tx, &store, &[(Kind::File, old_file)], &old_sha[..6]).await?;
    tx.commit().await?;

    // The row stays, marked purged, still in V's history.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    let change = &v.file_history.unwrap()[0];
    assert_eq!(change.old_file_id, old_file);
    assert!(change.old_purged_at.is_some());
    assert_eq!(change.old_purged_by.as_deref(), Some("alice"));
    // Its bytes can be stored again, as a new file.
    assert!(db::ingest::find_file_by_sha256(&mut tx, &old_sha).await?.is_none());
    drop(tx);
    let mut tx = db::begin(&lib.owner, None).await?;
    let (o_track, _) = super::rls_tests::make_track(&mut tx, "Reupload", &[lib.both], None).await?;
    sqlx::query("UPDATE files SET sha256 = $1 WHERE id = (SELECT file_id FROM tracks WHERE id = $2)")
        .bind(&old_sha)
        .bind(o_track)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A new file for `track` (not yet its current one), like ingest stores one.
pub(super) async fn add_file(conn: &mut sqlx::PgConnection, track: Uuid, name: &str) -> anyhow::Result<Uuid> {
    let origin = sources::validate(
        sources::Kind::Track,
        NewSource {
            platform: "custom".into(),
            source_id: None,
            captured_at: None,
            data: serde_json::json!({ "label": name }),
        },
    )?;
    let snapshot = db::sources::add_snapshot(conn, SourceOwner::Track(track), &origin).await?;
    let id = Uuid::now_v7();
    db::ingest::insert_file(
        conn,
        &db::ingest::NewFile {
            id,
            origin_snapshot_id: snapshot.snapshot_id,
            sha256: &crate::auth::token_hash(name),
            original_filename: Some(name),
            uploaded_filename: name,
            size_bytes: 1,
            container: None,
            codec: None,
            bitrate: None,
            sample_rate: None,
            channels: None,
            bit_depth: None,
            duration_ms: None,
            embedded_tags: &serde_json::json!({}),
            imported_from: None,
        },
    )
    .await?;
    Ok(id)
}

#[sqlx::test]
async fn trash_respects_visibility(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;

    // A trashed cover of H, the track carol can't see.
    let mut tx = db::begin(&lib.owner, None).await?;
    let cover = NewCover { blob_sha256: &"a".repeat(64), mime: None, width: None, height: None, source: "upload" };
    let h_cover = db::covers::add(&mut tx, CoverOwner::Track(lib.h), &cover).await?;
    db::covers::trash(&mut tx, h_cover).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::trash::list(&mut tx).await?.iter().all(|i| i.id != h_cover));
    assert!(!db::trash::restore(&mut tx, Kind::Cover, h_cover).await?);
    assert!(!db::covers::trash(&mut tx, h_cover).await?);
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(db::trash::list(&mut tx).await?.iter().any(|i| i.id == h_cover), "bob can see H");
    drop(tx);

    // A source that's where a file came from can't be trashed.
    let origin_source: Uuid = sqlx::query_scalar(
        r#"SELECT ss.source_id FROM files f JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id
           WHERE f.id = (SELECT file_id FROM tracks WHERE id = $1)"#,
    )
    .bind(lib.v)
    .fetch_one(&lib.owner)
    .await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::sources::trash(&mut tx, origin_source).await.unwrap_err();
    assert!(rejected_message(&err).contains("came from"), "{err}");
    Ok(())
}

#[sqlx::test]
async fn album_cover_goes_on_its_tracks_once(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let sha = "b".repeat(64);
    let cover = NewCover { blob_sha256: &sha, mime: None, width: None, height: None, source: "upload" };

    // carol can see V but not H (both on the album).
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let album_cover = db::covers::add(&mut tx, CoverOwner::Album(lib.album), &cover).await?;
    assert_eq!(db::covers::apply_to_album_tracks(&mut tx, lib.album, album_cover).await?, (1, 0));
    assert_eq!(db::covers::apply_to_album_tracks(&mut tx, lib.album, album_cover).await?, (0, 1), "V has it now");
    let v_covers = db::tracks::covers_of(&mut tx, lib.v).await?;
    assert_eq!(v_covers.len(), 1);
    assert_eq!(v_covers[0].position, 0, "V had none, so it's the main cover");
    assert_eq!(v_covers[0].blob_sha256, sha, "the same image, not a new one");
    let album = db::catalog::get_album(&mut tx, lib.album, false).await?.unwrap();
    assert_eq!(album.display_cover_id, Some(album_cover));
    tx.commit().await?;

    let h_covers: i64 = sqlx::query_scalar("SELECT count(*) FROM covers WHERE track_id = $1")
        .bind(lib.h)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(h_covers, 0, "tracks carol can't see are left alone");
    Ok(())
}

#[sqlx::test]
async fn an_image_is_a_cover_once_per_owner(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let cover = NewCover { blob_sha256: &"b".repeat(64), mime: None, width: None, height: None, source: "upload" };
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let first = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover).await?;
    let err = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover).await.unwrap_err();
    assert!(rejected_message(&err).contains("already"), "{err}");
    // Another owner may have it, and after the trash the image can be added again...
    db::covers::add(&mut tx, CoverOwner::Album(lib.album), &cover).await?;
    assert!(db::covers::trash(&mut tx, first).await?);
    let again = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover).await?;
    // ...but then the trashed one can't come back beside it.
    let err = db::trash::restore(&mut tx, Kind::Cover, first).await.unwrap_err();
    assert!(rejected_message(&err).contains("already"), "{err}");
    assert!(db::covers::trash(&mut tx, again).await?);
    assert!(db::trash::restore(&mut tx, Kind::Cover, first).await?);
    tx.commit().await?;
    Ok(())
}
