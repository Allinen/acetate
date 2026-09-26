//! Merging duplicate tracks, end to end (same setup as `rls_tests`).
//!
//! D is a duplicate of V (both public; V is on album A and a remix of H, which carol can't
//! see). D has one of everything: a cover, lyrics, a NetEase source with the same id as
//! one of V's, a previous file, a relation with O and one with H, an open "possible
//! duplicate" flag and a share. Carol merges D into V.

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, make_track, setup};
use super::trash_tests::add_file;
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::revisions::{EditRejected, MergePicks, Side, Source};
use crate::model::sources::{self, Kind, NewSource, SourceOwner};
use crate::model::{LyricsData, Relation, TrackFacts};

struct Dupe {
    d: Uuid,
    d_file: Uuid,
    d_old_file: Uuid,
    v_file: Uuid,
    v_cover: Uuid,
    d_cover: Uuid,
}

async fn add_source(conn: &mut sqlx::PgConnection, track: Uuid, platform: &str, data: serde_json::Value) -> anyhow::Result<()> {
    let source = sources::validate(
        Kind::Track,
        NewSource { platform: platform.into(), source_id: None, captured_at: None, data },
    )?;
    db::sources::add_snapshot(conn, SourceOwner::Track(track), &source).await?;
    Ok(())
}

async fn make_duplicate(lib: &Library) -> anyhow::Result<Dupe> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let v_file: Uuid = sqlx::query_scalar("SELECT file_id FROM tracks WHERE id = $1")
        .bind(lib.v)
        .fetch_one(&mut *tx)
        .await?;

    // D, with a previous file of its own.
    let (d, d_old_file) = make_track(&mut tx, "Song V (copy)", &[lib.both], None).await?;
    let d_file = add_file(&mut tx, d, "D's upgrade").await?;
    db::ingest::replace_track_file(&mut tx, d, d_old_file, d_file, "upgrade").await?;

    // Relations: D with O, D with H (hidden from carol), and D with V itself.
    let facts = TrackFacts {
        relations: vec![
            Relation { kind: "alternate".into(), track_id: lib.o },
            Relation { kind: "instrumental_of".into(), track_id: lib.h },
            Relation { kind: "alternate".into(), track_id: lib.v },
        ],
        ..Default::default()
    };
    db::revisions::write_external(&mut tx, d, None, &facts, Source::Edit, "").await?;

    // Covers, lyrics, sources.
    let cover = |sha: &'static str| NewCover { blob_sha256: sha, mime: None, width: None, height: None, source: "upload" };
    let v_cover = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover("1111111111111111111111111111111111111111111111111111111111111111")).await?;
    let d_cover = db::covers::add(&mut tx, CoverOwner::Track(d), &cover("2222222222222222222222222222222222222222222222222222222222222222")).await?;
    let lyrics = LyricsData { body: "la la la".into(), ..Default::default() };
    db::revisions::create_lyrics(&mut tx, d, "manual", &lyrics, Source::Edit, "").await?;
    add_source(&mut tx, lib.v, "netease", json!({ "song_id": 1 })).await?;
    add_source(&mut tx, d, "netease", json!({ "song_id": 1 })).await?;

    // A flag saying they look alike, and a share of D.
    sqlx::query("INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2)")
        .bind(lib.v)
        .bind(json!({ "tracks": [d] }))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO shares (token_sha256, track_id, created_by) VALUES ($1, $2, $3)")
        .bind("c".repeat(64))
        .bind(d)
        .bind(lib.alice)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Dupe { d, d_file, d_old_file, v_file, v_cover, d_cover })
}

#[sqlx::test]
async fn merging_combines_and_picks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let picks = MergePicks {
        title: Side::Right,
        file: Side::Right,
        covers: Side::Right,
        ..Default::default()
    };

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &picks, None, None, "").await?;
    tx.commit().await?;
    assert_eq!(merged.variants_moved, 1);
    assert_eq!(merged.lyrics_moved, 1);

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    // Picks: right's title and file, left's credits.
    assert_eq!(v.title, "Song V (copy)");
    assert_eq!(v.artist_display, "Both");
    assert_eq!(v.file.id, dupe.d_file);
    // V's old file and D's previous file are now V's previous files.
    let previous: Vec<Uuid> = v.file_history.as_ref().unwrap().iter().map(|c| c.old_file_id).collect();
    assert!(previous.contains(&dupe.v_file) && previous.contains(&dupe.d_old_file), "{previous:?}");
    // D's variant (with its history) is V's now.
    assert_eq!(v.variants.len(), 2);
    assert!(v.variants.iter().any(|x| x.label.contains("(from \"Song V (copy)\")")));
    // Covers: D's is the main one, then V's (none in the trash).
    assert_eq!(v.covers.iter().map(|c| c.id).collect::<Vec<_>>(), vec![dupe.d_cover, dupe.v_cover]);
    // Lyrics moved; sources combined (the two NetEase song 1 sources are one now).
    assert_eq!(v.lyrics.len(), 1);
    let netease: Vec<_> = v.sources.iter().filter(|s| s.platform == "netease").collect();
    assert_eq!(netease.len(), 1);
    // Relations: D's with O moved, the one between V and D is gone.
    let related: Vec<(String, Uuid)> = v.relations.iter().map(|r| (r.kind.clone(), r.track_id)).collect();
    assert_eq!(related, vec![("alternate".to_string(), lib.o)], "carol doesn't see H");
    // D is gone from lists, and redirects.
    assert!(db::tracks::list_tracks(&mut tx, None, 100).await?.iter().all(|t| t.id != dupe.d));
    assert!(db::catalog::artist_tracks(&mut tx, lib.both, None, 100).await?.iter().all(|t| t.id != dupe.d));
    assert_eq!(db::tracks::merged_into(&mut tx, dupe.d).await?, Some(lib.v));
    // Old files can be cleaned up now (admins).
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let replaced: Vec<Uuid> = db::trash::replaced_files(&mut tx, None, 100).await?.iter().map(|f| f.id).collect();
    assert!(replaced.contains(&dupe.v_file) && replaced.contains(&dupe.d_old_file));
    drop(tx);

    // As the system: D's relation with H moved too (both halves), shares and flags follow.
    let v_to_h: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM track_relations
           WHERE (from_track_id = $1 AND kind = 'instrumental_of' AND to_track_id = $2)
              OR (from_track_id = $2 AND kind = 'instrumental_to' AND to_track_id = $1)"#,
    )
    .bind(lib.v)
    .bind(lib.h)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(v_to_h, 2, "the relation with the hidden track moved, from both sides");
    let with_d: i64 = sqlx::query_scalar("SELECT count(*) FROM track_relations WHERE $1 IN (from_track_id, to_track_id)")
        .bind(dupe.d)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(with_d, 0);
    let share_track: Uuid = sqlx::query_scalar("SELECT track_id FROM shares").fetch_one(&lib.owner).await?;
    assert_eq!(share_track, lib.v);
    let open_flags: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE resolved_at IS NULL")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(open_flags, 0);
    let moved_revisions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM track_revisions r JOIN track_variants v ON v.id = r.variant_id WHERE v.label LIKE '%(from%' AND r.track_id = $1",
    )
    .bind(lib.v)
    .fetch_one(&lib.owner)
    .await?;
    assert!(moved_revisions >= 1, "D's revisions moved with its variant");
    let logged: serde_json::Value = sqlx::query_scalar("SELECT picks FROM track_merges").fetch_one(&lib.owner).await?;
    assert_eq!(logged["title"], "right");
    Ok(())
}

#[sqlx::test]
async fn merging_is_refused_when_it_should_be(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let picks = MergePicks::default();
    let rejection = |err: anyhow::Error| err.downcast_ref::<EditRejected>().map(|e| e.to_string()).unwrap_or_else(|| format!("{err:#}"));

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::revisions::merge_tracks(&mut tx, lib.v, lib.v, &picks, None, None, "").await.unwrap_err();
    assert!(rejection(err).contains("itself"));
    drop(tx);

    // Made from an outdated view.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &picks, Some(Uuid::now_v7()), None, "").await.unwrap_err();
    assert!(matches!(err.downcast_ref(), Some(EditRejected::Stale { .. })));
    drop(tx);

    // A track carol can't see.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::revisions::merge_tracks(&mut tx, lib.v, lib.h, &picks, None, None, "").await.unwrap_err();
    assert!(rejection(err).contains("not found"));
    drop(tx);

    // Twice.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &picks, None, None, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::revisions::merge_tracks(&mut tx, lib.o, dupe.d, &picks, None, None, "").await.unwrap_err();
    assert!(rejection(err).contains("already merged"));
    Ok(())
}

/// Everything about a track that a merge changes and an undo must give back, as the
/// system sees it (hidden relations included), in a stable order. Not review flags: an
/// undo leaves those settled.
async fn snapshot(lib: &Library, track: Uuid) -> anyhow::Result<serde_json::Value> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let t = db::tracks::get_track(&mut tx, track, true).await?.expect("track exists");
    let sorted = |mut v: Vec<String>| { v.sort(); v };
    let shares: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM shares WHERE track_id = $1 ORDER BY id")
        .bind(track)
        .fetch_all(&mut *tx)
        .await?;
    Ok(json!({
        "title": t.title,
        "artist_display": t.artist_display,
        "status": t.status,
        "file": t.file.id,
        "previous_files": sorted(t.file_history.unwrap().iter().map(|c| c.old_file_id.to_string()).collect()),
        "variants": sorted(t.variants.iter().map(|v| format!("{} {}", v.id, v.label)).collect()),
        "credits": t.credits.iter().map(|c| format!("{} {}", c.artist_id, c.role)).collect::<Vec<_>>(),
        "covers": t.covers.iter().map(|c| format!("{} {}", c.id, c.position)).collect::<Vec<_>>(),
        "lyrics": sorted(t.lyrics.iter().map(|l| l.id.to_string()).collect()),
        "sources": sorted(t.sources.iter().map(|s| {
            let snaps: Vec<String> = s.snapshots.iter().map(|x| x.id.to_string()).collect();
            format!("{} {} {:?}", s.id, s.platform, snaps)
        }).collect()),
        "relations": sorted(t.relations.iter().map(|r| format!("{} {}", r.kind, r.track_id)).collect()),
        "albums": sorted(t.albums.iter().map(|a| format!("{} {:?}", a.album_id, a.track_number)).collect()),
        "shares": shares,
    }))
}

async fn merge_ids(lib: &Library) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT id FROM track_merges ORDER BY id").fetch_all(&lib.owner).await?)
}

#[sqlx::test]
async fn undo_gives_both_tracks_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let (v_before, d_before) = (snapshot(&lib, lib.v).await?, snapshot(&lib, dupe.d).await?);
    let o_before = snapshot(&lib, lib.o).await?;

    let picks = MergePicks { title: Side::Right, file: Side::Right, covers: Side::Right, ..Default::default() };
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &picks, None, None, "").await?;
    tx.commit().await?;
    // In the meantime, the file that wasn't picked went to the trash (undo takes it out).
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::trash::trash_files(&mut tx, &[dupe.v_file]).await?;
    tx.commit().await?;

    let merge = merge_ids(&lib).await?[0];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::revisions::merge_undo_blockers(&mut tx, merge).await?, Vec::<String>::new());
    let restored = db::revisions::undo_track_merge(&mut tx, merge, "").await?;
    tx.commit().await?;
    assert_eq!(restored, dupe.d);

    assert_eq!(snapshot(&lib, lib.v).await?, v_before);
    assert_eq!(snapshot(&lib, dupe.d).await?, d_before);
    assert_eq!(snapshot(&lib, lib.o).await?, o_before, "the third track's relation came back");
    let open_flags: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE resolved_at IS NULL")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(open_flags, 0, "\"V looks like D\" stays settled after the undo");
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::tracks::merged_into(&mut tx, dupe.d).await?, None);
    assert!(db::tracks::list_tracks(&mut tx, None, 100).await?.iter().any(|t| t.id == dupe.d));
    let trashed: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT trashed_at FROM files WHERE id = $1")
        .bind(dupe.v_file)
        .fetch_one(&lib.owner)
        .await?;
    assert!(trashed.is_none(), "V's own file is out of the trash");
    let _ = (dupe.d_file, dupe.d_old_file, dupe.v_cover, dupe.d_cover);
    Ok(())
}

#[sqlx::test]
async fn undo_goes_newest_first_and_once(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let picks = MergePicks::default();
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &picks, None, None, "").await?;
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &picks, None, None, "").await?;
    tx.commit().await?;
    let ids = merge_ids(&lib).await?;
    let (first, second) = (ids[0], ids[1]);

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let err = db::revisions::undo_track_merge(&mut tx, first, "").await.unwrap_err();
    assert!(err.to_string().contains("merged into it later"), "{err}");
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::undo_track_merge(&mut tx, second, "").await?;
    db::revisions::undo_track_merge(&mut tx, first, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let err = db::revisions::undo_track_merge(&mut tx, first, "").await.unwrap_err();
    assert!(err.to_string().contains("undone already"), "{err}");
    Ok(())
}

#[sqlx::test]
async fn undo_is_refused_after_later_changes(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &MergePicks::default(), None, None, "").await?;
    tx.commit().await?;
    let merge = merge_ids(&lib).await?[0];

    // The file that wasn't picked (D's) is trashed and purged.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::trash::trash_files(&mut tx, &[dupe.d_file]).await?;
    tx.commit().await?;
    sqlx::query("UPDATE files SET trashed_at = now() - interval '8 days' WHERE id = $1")
        .bind(dupe.d_file)
        .execute(&lib.owner)
        .await?;
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM files WHERE id = $1").bind(dupe.d_file).fetch_one(&lib.owner).await?;
    let dir = tempfile::tempdir()?;
    let store = crate::storage::BlobStore::open(dir.path())?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::trash::purge(&mut tx, &store, &[(db::trash::Kind::File, dupe.d_file)], &sha[..6]).await?;
    tx.commit().await?;

    // And the kept track's title is edited.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
    let primary = v.variants.iter().find(|x| Some(x.id) == v.primary_variant_id).unwrap();
    let mut data: crate::model::TrackMetadata = serde_json::from_value(primary.data.clone())?;
    data.title = "Edited later".into();
    db::revisions::write_internal(&mut tx, lib.v, primary.id, Some(primary.revision_id), &data, Source::Edit, "", false).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let blockers = db::revisions::merge_undo_blockers(&mut tx, merge).await?;
    assert!(blockers.iter().any(|b| b.contains("metadata")), "{blockers:?}");
    assert!(blockers.iter().any(|b| b.contains("purged")), "{blockers:?}");
    let err = db::revisions::undo_track_merge(&mut tx, merge, "").await.unwrap_err();
    assert!(matches!(err.downcast_ref(), Some(EditRejected::Invalid(_))));
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    assert_eq!(db::tracks::merged_into(&mut tx, dupe.d).await?, Some(lib.v), "nothing changed");
    Ok(())
}

#[sqlx::test]
async fn merge_combines_annotations_and_undo_splits_them(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    use crate::db::annotations::{self, Item};
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;

    // Carol played V twice and rated it 4; she played D 3 times and starred it.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    for _ in 0..2 {
        annotations::played(&mut tx, lib.v).await?;
    }
    annotations::set_rating(&mut tx, Item::Track(lib.v), 4).await?;
    for _ in 0..3 {
        annotations::played(&mut tx, dupe.d).await?;
    }
    annotations::set_starred(&mut tx, Item::Track(dupe.d), true).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &MergePicks::default(), None, None, "").await?;
    let merged = annotations::get(&mut tx, Item::Track(lib.v)).await?;
    tx.commit().await?;
    assert_eq!((merged.play_count, merged.rating, merged.starred), (5, 4, true));

    // After the merge she plays it once more and changes the rating; undo keeps those.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    annotations::played(&mut tx, lib.v).await?;
    annotations::set_rating(&mut tx, Item::Track(lib.v), 8).await?;
    tx.commit().await?;

    let merge = merge_ids(&lib).await?[0];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::undo_track_merge(&mut tx, merge, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let v = annotations::get(&mut tx, Item::Track(lib.v)).await?;
    let d = annotations::get(&mut tx, Item::Track(dupe.d)).await?;
    assert_eq!((v.play_count, v.rating, v.starred), (3, 8, false), "{v:?}");
    assert_eq!((d.play_count, d.starred), (3, true));
    Ok(())
}

// ---- Sharing a file: two tracks, one file (`track_files`) ----

async fn file_of(lib: &Library, track: Uuid) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar("SELECT file_id FROM tracks WHERE id = $1").bind(track).fetch_one(&lib.owner).await?)
}

#[sqlx::test]
async fn sharing_a_file_and_taking_it_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let v_before = snapshot(&lib, lib.v).await?;

    // D plays V's file from now on; everything else about both stays.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let switched = db::track_files::use_file_of(&mut tx, dupe.d, lib.v, "").await?;
    tx.commit().await?;
    assert_eq!((switched.old_file_id, switched.new_file_id), (dupe.d_file, dupe.v_file));
    assert_eq!(file_of(&lib, dupe.d).await?, dupe.v_file);
    assert_eq!(snapshot(&lib, lib.v).await?, v_before, "V itself is untouched");

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let d = db::tracks::get_track(&mut tx, dupe.d, true).await?.unwrap();
    assert_eq!(d.title, "Song V (copy)");
    let history = d.file_history.unwrap();
    assert_eq!(history[0].old_file_id, dupe.d_file, "D's own file is a previous file now");
    assert!(history[0].reason.contains("uses the file of"));
    let v = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
    assert_eq!(v.file_shared_with.iter().map(|t| t.id).collect::<Vec<_>>(), vec![dupe.d]);
    drop(tx);

    // D's own file is played by nothing now: listed for cleanup. The shared one isn't.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let replaced: Vec<Uuid> = db::trash::replaced_files(&mut tx, None, 100).await?.iter().map(|f| f.id).collect();
    assert!(replaced.contains(&dupe.d_file));
    assert!(!replaced.contains(&dupe.v_file));
    // Its bytes stored again are a duplicate of the older track playing it (V).
    let sha: String = sqlx::query_scalar("SELECT sha256 FROM files WHERE id = $1").bind(dupe.v_file).fetch_one(&mut *tx).await?;
    let found = db::ingest::find_file_by_sha256(&mut tx, &sha).await?.unwrap();
    assert_eq!(found.track_id, Some(lib.v.min(dupe.d)));
    // D's own file goes to the trash in the meantime.
    db::trash::trash_files(&mut tx, &[dupe.d_file]).await?;
    tx.commit().await?;

    // Taking it back: D plays its own file again (out of the trash), V keeps its file.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::track_files::use_previous_file(&mut tx, dupe.d, dupe.d_file, "").await?;
    tx.commit().await?;
    assert_eq!(file_of(&lib, dupe.d).await?, dupe.d_file);
    assert_eq!(file_of(&lib, lib.v).await?, dupe.v_file);
    let trashed: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar("SELECT trashed_at FROM files WHERE id = $1")
        .bind(dupe.d_file)
        .fetch_one(&lib.owner)
        .await?;
    assert!(trashed.is_none());
    Ok(())
}

#[sqlx::test]
async fn sharing_a_file_is_refused_when_it_should_be(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let rejection = |err: anyhow::Error| err.downcast_ref::<EditRejected>().map(|e| e.to_string()).unwrap_or_else(|| format!("{err:#}"));
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = db::track_files::use_file_of(&mut tx, lib.v, lib.v, "").await.unwrap_err();
    assert!(rejection(err).contains("itself"));
    // H is hidden from carol: as if it didn't exist.
    let err = db::track_files::use_file_of(&mut tx, dupe.d, lib.h, "").await.unwrap_err();
    assert!(rejection(err).contains("not found"));
    // Not one of V's previous files.
    let err = db::track_files::use_previous_file(&mut tx, lib.v, dupe.d_old_file, "").await.unwrap_err();
    assert!(rejection(err).contains("previous files"));
    drop(tx);

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::track_files::use_file_of(&mut tx, dupe.d, lib.v, "").await?;
    let err = db::track_files::use_file_of(&mut tx, lib.v, dupe.d, "").await.unwrap_err();
    assert!(rejection(err).contains("already play the same file"));
    Ok(())
}

#[sqlx::test]
async fn merging_tracks_that_share_a_file(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::track_files::use_file_of(&mut tx, dupe.d, lib.v, "").await?;
    tx.commit().await?;
    let (v_before, d_before) = (snapshot(&lib, lib.v).await?, snapshot(&lib, dupe.d).await?);

    // No second file to keep: V's history gets D's rows, but no "merged" row of its own.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &MergePicks::default(), None, None, "").await?;
    tx.commit().await?;
    assert_eq!(file_of(&lib, lib.v).await?, dupe.v_file);

    let merge = merge_ids(&lib).await?[0];
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::revisions::merge_undo_blockers(&mut tx, merge).await?, Vec::<String>::new());
    db::revisions::undo_track_merge(&mut tx, merge, "").await?;
    tx.commit().await?;
    assert_eq!(snapshot(&lib, lib.v).await?, v_before);
    assert_eq!(snapshot(&lib, dupe.d).await?, d_before);
    Ok(())
}

#[sqlx::test]
async fn sharing_a_file_settles_only_that_pairs_duplicate_flags(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?; // already flags V: "looks like D"
    let flag = |track: Uuid, tracks: Vec<Uuid>| {
        let pool = lib.owner.clone();
        async move {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2) RETURNING id",
            )
            .bind(track)
            .bind(json!({ "tracks": tracks }))
            .fetch_one(&pool)
            .await?;
            anyhow::Ok(id)
        }
    };
    let v_d: Uuid = sqlx::query_scalar("SELECT id FROM review_flags WHERE track_id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    let v_d_o = flag(lib.v, vec![dupe.d, lib.o]).await?; // V looks like D and O
    let d_v = flag(dupe.d, vec![lib.v]).await?; // the other way round
    let d_o = flag(dupe.d, vec![lib.o]).await?; // nothing to do with V

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::track_files::use_file_of(&mut tx, dupe.d, lib.v, "").await?;
    tx.commit().await?;

    let state = |id: Uuid| {
        let pool = lib.owner.clone();
        async move {
            let row: (bool, String, serde_json::Value) =
                sqlx::query_as("SELECT resolved_at IS NOT NULL, resolution, related FROM review_flags WHERE id = $1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await?;
            anyhow::Ok(row)
        }
    };
    for id in [v_d, d_v] {
        let (resolved, resolution, _) = state(id).await?;
        assert!(resolved && resolution.starts_with("shared file"), "{resolution}");
    }
    // Still open about O; D is off its list, noted as settled.
    let (resolved, _, related) = state(v_d_o).await?;
    assert!(!resolved);
    assert_eq!(related["tracks"], json!([lib.o]));
    assert_eq!(related["settled"][0]["track"], json!(dupe.d));
    // Untouched.
    let (resolved, _, related) = state(d_o).await?;
    assert!(!resolved);
    assert_eq!(related, json!({ "tracks": [lib.o] }));
    Ok(())
}

#[sqlx::test]
async fn merging_settles_only_that_pairs_duplicate_flags(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?;
    // Besides "V looks like D": "V looks like D and O".
    let v_d_o: Uuid = sqlx::query_scalar(
        "INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2) RETURNING id",
    )
    .bind(lib.v)
    .bind(json!({ "tracks": [dupe.d, lib.o] }))
    .fetch_one(&lib.owner)
    .await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, dupe.d, &MergePicks::default(), None, None, "").await?;
    tx.commit().await?;

    let (resolved, related): (bool, serde_json::Value) =
        sqlx::query_as("SELECT resolved_at IS NOT NULL, related FROM review_flags WHERE id = $1")
            .bind(v_d_o)
            .fetch_one(&lib.owner)
            .await?;
    assert!(!resolved, "still open: is V a duplicate of O?");
    assert_eq!(related["tracks"], json!([lib.o]));
    assert_eq!(related["settled"][0]["track"], json!(dupe.d));
    Ok(())
}

#[sqlx::test]
async fn merge_duplicates_merges_same_title_and_length(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dupe = make_duplicate(&lib).await?; // V is flagged: "looks like D"
    let options = |dry_run| crate::merge_duplicates_command::Options { max_length_difference_ms: 1000, ignore_case: false, dry_run };

    // Different titles ("Song V" and "Song V (copy)"): left alone.
    sqlx::query("UPDATE files SET duration_ms = 180000").execute(&lib.owner).await?;
    crate::merge_duplicates_command::run(&lib.owner, options(false)).await?;
    assert_eq!(db::tracks::merged_into(&mut *lib.owner.acquire().await?, dupe.d).await?, None);

    // Only the case differs: merged with --ignore-case only (a dry run changes nothing).
    sqlx::query("UPDATE tracks SET title = 'SONG V' WHERE id = $1").bind(dupe.d).execute(&lib.owner).await?;
    crate::merge_duplicates_command::run(&lib.owner, options(false)).await?;
    assert_eq!(db::tracks::merged_into(&mut *lib.owner.acquire().await?, dupe.d).await?, None);
    let ignoring_case = crate::merge_duplicates_command::Options { max_length_difference_ms: 1000, ignore_case: true, dry_run: true };
    crate::merge_duplicates_command::run(&lib.owner, ignoring_case).await?;
    assert_eq!(db::tracks::merged_into(&mut *lib.owner.acquire().await?, dupe.d).await?, None);

    // Same title, 0.5 s apart, D's file the bigger one: D is merged into V (the older),
    // with D's file. A dry run first changes nothing.
    sqlx::query("UPDATE tracks SET title = 'Song V' WHERE id = $1").bind(dupe.d).execute(&lib.owner).await?;
    sqlx::query("UPDATE files SET duration_ms = 180500, size_bytes = 9999999 WHERE id = $1").bind(dupe.d_file).execute(&lib.owner).await?;
    crate::merge_duplicates_command::run(&lib.owner, options(true)).await?;
    assert_eq!(db::tracks::merged_into(&mut *lib.owner.acquire().await?, dupe.d).await?, None);
    crate::merge_duplicates_command::run(&lib.owner, options(false)).await?;
    assert_eq!(db::tracks::merged_into(&mut *lib.owner.acquire().await?, dupe.d).await?, Some(lib.v));
    assert_eq!(file_of(&lib, lib.v).await?, dupe.d_file);

    // Nothing left to do.
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE resolved_at IS NULL")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(open, 0);
    Ok(())
}
