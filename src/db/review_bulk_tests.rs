//! The Review page's bulk tools (`db::review_bulk`, `review::resolve_many`) and the
//! catalog's `platforms` column (migration 0036) that `source:` filters on.
//!
//! The library (see `rls_tests::setup`): V (Both) and H (X, Both; restricted to friends)
//! on album A, O (Both) on no album.

use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::review_bulk::Settled;
use crate::model::sources::{self, Kind, NewSource, SourceOwner};

async fn credited(lib: &Library, track: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT artist_id FROM track_credits WHERE track_id = $1 ORDER BY position")
        .bind(track)
        .fetch_all(&lib.owner)
        .await?)
}

async fn open_kinds(lib: &Library, track: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT kind FROM review_flags WHERE track_id = $1 AND resolved_at IS NULL ORDER BY kind")
        .bind(track)
        .fetch_all(&lib.owner)
        .await?)
}

/// "Both" matched Both and X on each track; ingest chose Both. Returns the flags' ids.
async fn flag_ambiguous(lib: &Library, tracks: &[Uuid]) -> anyhow::Result<Vec<Uuid>> {
    let mut tx = db::begin(&lib.owner, None).await?;
    for &track in tracks {
        db::review::flag_ambiguous_artist(&mut tx, track, "Both", lib.both, &[lib.both, lib.x]).await?;
    }
    tx.commit().await?;
    let mut ids = Vec::new();
    for &track in tracks {
        ids.push(sqlx::query_scalar("SELECT id FROM review_flags WHERE track_id = $1 AND kind = 'ambiguous_artist'").bind(track).fetch_one(&lib.owner).await?);
    }
    Ok(ids)
}

#[sqlx::test]
async fn ambiguous_names_settle_on_one_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let flags = flag_ambiguous(&lib, &[lib.v, lib.h, lib.o]).await?;

    // Carol can't see H: its flag is left alone.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let done = db::review_bulk::settle_ambiguous(&mut tx, &flags, lib.x, "").await?;
    tx.commit().await?;
    assert_eq!(done, Settled { settled: 2, changed: 2, skipped: 1 });
    assert_eq!(credited(&lib, lib.v).await?, [lib.x]);
    assert_eq!(credited(&lib, lib.o).await?, [lib.x]);
    assert_eq!(credited(&lib, lib.h).await?, [lib.x, lib.both], "not seen, not changed");
    let (resolution, reason): (String, String) = sqlx::query_as(
        "SELECT f.resolution, r.reason FROM review_flags f JOIN track_revisions r ON r.track_id = f.track_id
         WHERE f.id = $1 ORDER BY r.created_at DESC LIMIT 1",
    )
    .bind(flags[0])
    .fetch_one(&lib.owner)
    .await?;
    assert!(resolution.contains("credited \"X\""), "{resolution}");
    assert_eq!(reason, "ambiguous artist name: \"X\"");

    // Bob sees H: X is credited already, so the chosen Both's credit just goes (once is enough).
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let done = db::review_bulk::settle_ambiguous(&mut tx, &flags, lib.x, "it was X").await?;
    tx.commit().await?;
    assert_eq!(done, Settled { settled: 1, changed: 1, skipped: 2 }, "V's and O's are settled already");
    assert_eq!(credited(&lib, lib.h).await?, [lib.x]);
    Ok(())
}

#[sqlx::test]
async fn the_right_one_already_credited_just_settles(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let flags = flag_ambiguous(&lib, &[lib.v]).await?;
    let revisions = || sqlx::query_scalar::<_, i64>("SELECT count(*) FROM track_revisions WHERE track_id = $1").bind(lib.v).fetch_one(&lib.owner);
    let before = revisions().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let done = db::review_bulk::settle_ambiguous(&mut tx, &flags, lib.both, "").await?;
    tx.commit().await?;
    assert_eq!(done, Settled { settled: 1, changed: 0, skipped: 0 });
    assert_eq!(revisions().await?, before, "no revision for nothing");
    assert!(!open_kinds(&lib, lib.v).await?.contains(&"ambiguous_artist".to_string()));
    // An artist that isn't there: nothing happens.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::review_bulk::settle_ambiguous(&mut tx, &flags, Uuid::now_v7(), "").await.is_err());
    Ok(())
}

#[sqlx::test]
async fn many_flags_resolve_at_once(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM review_flags WHERE kind = 'missing_cover' AND resolved_at IS NULL")
        .fetch_all(&lib.owner)
        .await?;
    assert_eq!(ids.len(), 3);
    // Carol resolves those she sees (not H's), once.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::review::resolve_many(&mut tx, &ids, "covers later").await?, 2);
    assert_eq!(db::review::resolve_many(&mut tx, &ids, "again").await?, 0);
    tx.commit().await?;
    assert_eq!(open_kinds(&lib, lib.v).await?, Vec::<String>::new());
    assert_eq!(open_kinds(&lib, lib.o).await?, ["missing_album"]);
    assert_eq!(open_kinds(&lib, lib.h).await?, ["missing_cover"]);
    Ok(())
}

#[sqlx::test]
async fn tracks_on_no_album_become_singles(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let made = db::review_bulk::make_singles(&mut tx, &[lib.o, lib.v], "").await?;
    tx.commit().await?;
    assert_eq!(made.skipped, [lib.v], "V is on an album");
    assert_eq!(made.made.len(), 1);
    let single = made.made[0].album_id;
    let (title, kind): (String, String) = sqlx::query_as("SELECT title, kind FROM albums WHERE id = $1").bind(single).fetch_one(&lib.owner).await?;
    assert_eq!((title.as_str(), kind.as_str()), ("Other", "single"));
    let artists: Vec<Uuid> = sqlx::query_scalar("SELECT artist_id FROM album_credits WHERE album_id = $1").bind(single).fetch_all(&lib.owner).await?;
    assert_eq!(artists, [lib.both]);
    let on: (Uuid, Option<i16>, Option<i16>) = sqlx::query_as("SELECT album_id, disc_number, track_number FROM album_tracks WHERE track_id = $1")
        .bind(lib.o)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(on, (single, Some(1), Some(1)));
    assert_eq!(open_kinds(&lib, lib.o).await?, ["missing_cover"], "no album settled itself");
    // Again: it's on one now.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::review_bulk::make_singles(&mut tx, &[lib.o], "").await?.skipped, [lib.o]);
    Ok(())
}

#[sqlx::test]
async fn the_catalog_says_where_tracks_come_from(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let platforms = |rows: &str, track: Uuid| -> Value {
        let rows: Vec<Vec<Value>> = serde_json::from_str(rows).unwrap();
        let at = db::library::TRACK_COLUMNS.iter().position(|c| *c == "platforms").unwrap();
        rows.into_iter().find(|r| r[0] == json!(track)).unwrap()[at].clone()
    };
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let version = db::library::version(&mut tx).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.owner, None).await?;
    let youtube = sources::validate(Kind::Track, NewSource { platform: "youtube".into(), source_id: None, captured_at: None, data: json!({ "song_id": "dQw4w9WgXcQ" }) })?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Track(lib.v), &youtube).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    // The source changed the track: it's in the next delta.
    let changes = db::library::changes_since(&mut tx, &version).await?;
    assert_eq!(changes.tracks, [lib.v]);
    let rows = db::library::tracks_json(&mut tx, None).await?;
    assert_eq!(platforms(&rows, lib.v), json!(["custom", "youtube"]));
    assert_eq!(platforms(&rows, lib.o), json!(["custom"]));
    Ok(())
}
