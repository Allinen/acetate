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
    assert_eq!(merged.covers_trashed, 1);

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
    // Covers: D's is the main one, V's is in the trash.
    assert_eq!(v.covers.iter().map(|c| c.id).collect::<Vec<_>>(), vec![dupe.d_cover]);
    let trashed: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT trashed_at FROM covers WHERE id = $1").bind(dupe.v_cover).fetch_one(&lib.owner).await?;
    assert!(trashed.is_some());
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
