//! Groupings (`db::groupings`, migration 0034): made and changed with every change logged,
//! seen as far as their tracks are, in the catalog and its deltas; a collective's account
//! taking its songs at ingest; an artist that is really a collective turned into one.
//! Same setup as `rls_tests` (and `ingest_tests` for the files).

use std::path::Path;

use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use crate::db;
use crate::ingest::describe::{ArtistHint, Hints, MatchedBy, Report};
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource, SourceOwner, ValidSource};
use crate::storage::BlobStore;

fn netease(kind: Kind, field: &str, id: i64) -> ValidSource {
    let source = NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ field: id }) };
    sources::validate(kind, source).unwrap()
}

/// "01 - Glass Harbor.flac" (ARTIST and ALBUM ARTIST Mira Vale), from NetEase song
/// `song`, with Mira Vale said to be NetEase artist `artist`.
async fn ingest_glass_harbor(pool: &PgPool, store: &BlobStore, file: &str, song: i64, artist: i64) -> anyhow::Result<Report> {
    let hints = Hints {
        artists: vec![ArtistHint { name: "Mira Vale".into(), id: None, sources: vec![netease(Kind::Artist, "artist_id", artist)], platform_name: None }],
        ..Default::default()
    };
    let options = IngestOptions {
        user: None,
        uploaded_filename: file.into(),
        origin: netease(Kind::Track, "song_id", song),
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints,
        local_path: None,
        given: Default::default(),
        tags_from: None,
        volume: None,
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file);
    let pending = ingest::receive_path(store, &path, "main").await?;
    ingest::ingest_with_report(pool, store, pending, &options, false).await
}

async fn in_grouping(pool: &PgPool, grouping: Uuid) -> anyhow::Result<(Vec<Uuid>, Vec<Uuid>)> {
    let tracks = sqlx::query_scalar("SELECT track_id FROM grouping_tracks WHERE grouping_id = $1 ORDER BY track_id").bind(grouping).fetch_all(pool).await?;
    let albums = sqlx::query_scalar("SELECT album_id FROM grouping_albums WHERE grouping_id = $1 ORDER BY album_id").bind(grouping).fetch_all(pool).await?;
    Ok((tracks, albums))
}

async fn credited(pool: &PgPool, track: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT a.name FROM track_credits c JOIN artists a ON a.id = c.artist_id WHERE c.track_id = $1 ORDER BY c.position")
        .bind(track)
        .fetch_all(pool)
        .await?)
}

#[sqlx::test]
async fn groupings_are_changed_logged_and_seen_as_far_as_their_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;

    // Made and filled by bob (who sees H, restricted to friends).
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let version = db::library::version(&mut tx).await?;
    let id = db::groupings::create(&mut tx, "collective", "  Tide Crew ", "", "").await?;
    assert_eq!(db::groupings::add(&mut tx, id, &[lib.v, lib.h], &[lib.album], "").await?, 3);
    assert_eq!(db::groupings::add(&mut tx, id, &[lib.v], &[], "").await?, 0, "already in it");
    db::groupings::edit(&mut tx, id, "collective", "Tide Crew", "friends posting together", "a description").await?;
    assert!(db::groupings::create(&mut tx, "Not A Kind", "x", "", "").await.is_err());
    tx.commit().await?;

    // Carol sees the grouping, and of what's in it only what she can see.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let d = db::groupings::detail(&mut tx, id).await?.expect("visible");
    assert_eq!((d.name.as_str(), d.description.as_str()), ("Tide Crew", "friends posting together"));
    assert_eq!(d.tracks, [lib.v]);
    assert_eq!(d.albums, [lib.album]);
    assert_eq!(d.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["Both"], "X is only on H");
    assert!(d.changes.iter().all(|c| c.track_id != Some(lib.h)), "not a word about H");
    assert_eq!(d.changes.first().map(|c| c.action.as_str()), Some("edited"), "newest first");
    // The catalog: V's row names it, the list counts what she can see.
    let tracks: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx, Some(&[lib.v])).await?)?;
    let at = db::library::TRACK_COLUMNS.iter().position(|c| *c == "groupings").unwrap();
    assert_eq!(tracks[0][at], json!([id]));
    let list: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::groupings_json(&mut tx).await?)?;
    assert_eq!(list, [vec![json!(id), json!("collective"), json!("Tide Crew"), json!(1), json!(1)]]);
    // A delta since before it has the grouping, and V.
    let changes = db::library::changes_since(&mut tx, &version).await?;
    assert!(changes.groupings && changes.tracks.contains(&lib.v));
    drop(tx);

    // Bob sees H in it; taking it out is logged, and nothing's deleted but the membership.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(db::groupings::detail(&mut tx, id).await?.unwrap().tracks.len(), 2);
    assert_eq!(db::groupings::remove(&mut tx, id, &[lib.h], &[], "not theirs").await?, 1);
    let d = db::groupings::detail(&mut tx, id).await?.unwrap();
    assert_eq!(d.tracks, [lib.v]);
    assert_eq!((d.changes[0].action.as_str(), d.changes[0].reason.as_str()), ("removed", "not theirs"));
    assert!(sqlx::query("DELETE FROM groupings").execute(&mut *tx).await.is_err(), "groupings aren't deleted");
    Ok(())
}

#[sqlx::test]
async fn a_collectives_account_takes_its_songs_instead_of_an_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // The collective is NetEase artist 10.
    let mut tx = db::begin(&lib.owner, None).await?;
    let crew = db::groupings::create(&mut tx, "collective", "Mira Vale", "", "").await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Grouping(crew), &netease(Kind::Artist, "artist_id", 10)).await?;
    tx.commit().await?;

    let report = ingest_glass_harbor(&lib.owner, &store, "01 - Glass Harbor.flac", 1, 10).await?;
    let track = report.track_id.unwrap();
    assert_eq!(report.collectives.len(), 1);
    assert_eq!((report.collectives[0].id, report.collectives[0].matched_by), (Some(crew), MatchedBy::PlatformId));
    assert!(report.artists.is_empty(), "no one credited as the collective: {:?}", report.artists);
    let album = report.album.unwrap().id.unwrap();
    assert_eq!(in_grouping(&lib.owner, crew).await?, (vec![track], vec![album]), "its track and album (its album artist)");
    assert!(credited(&lib.owner, track).await?.is_empty());
    let artists: i64 = sqlx::query_scalar("SELECT count(*) FROM artists WHERE name = 'Mira Vale'").fetch_one(&lib.owner).await?;
    assert_eq!(artists, 0, "the collective didn't become an artist");
    // No one credited: the server's own check flags it, for Review.
    let flags: Vec<String> = sqlx::query_scalar("SELECT kind FROM review_flags WHERE track_id = $1 AND resolved_at IS NULL").bind(track).fetch_all(&lib.owner).await?;
    assert!(flags.iter().any(|k| k == "missing_artists"), "{flags:?}");
    Ok(())
}

#[sqlx::test]
async fn an_artist_that_is_a_collective_is_turned_into_one(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Ingested before there were groupings: "Mira Vale" became an artist (NetEase 10).
    let first = ingest_glass_harbor(&lib.owner, &store, "01 - Glass Harbor.flac", 1, 10).await?;
    let mira = first.artists[0].id.unwrap();
    let track = first.track_id.unwrap();
    let album = first.album.unwrap().id.unwrap();

    // What it would do, first (as alice, an admin).
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let preview = db::groupings::collective_preview(&mut tx, mira).await?;
    assert_eq!((preview.name.as_str(), preview.tracks, preview.albums, preview.sources.len()), ("Mira Vale", 1, 1, 1));
    assert_eq!(preview.tracks_left_without_primary.iter().map(|t| t.id).collect::<Vec<_>>(), [track]);
    assert_eq!(preview.albums_left_without_artists.len(), 1);
    drop(tx);

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let crew = db::groupings::turn_into_collective(&mut tx, mira, "").await?;
    tx.commit().await?;
    assert_eq!(in_grouping(&lib.owner, crew).await?, (vec![track], vec![album]));
    assert!(credited(&lib.owner, track).await?.is_empty(), "its credit came off (a new revision)");
    let moved: Vec<Option<Uuid>> = sqlx::query_scalar("SELECT grouping_id FROM sources WHERE platform = 'netease' AND external_id = '10'").fetch_all(&lib.owner).await?;
    assert_eq!(moved, [Some(crew)], "the account is the collective's now");
    let kind: String = sqlx::query_scalar("SELECT kind FROM groupings WHERE id = $1").bind(crew).fetch_one(&lib.owner).await?;
    assert_eq!(kind, "collective");
    let restorable: i64 = sqlx::query_scalar("SELECT count(*) FROM track_revisions WHERE track_id = $1").bind(track).fetch_one(&lib.owner).await?;
    assert!(restorable >= 2, "the credited revision is still there to restore");

    // A new song from the account goes to the collective, with no one credited as it.
    let second = ingest_glass_harbor(&lib.owner, &store, "02 - Undertow.flac", 2, 10).await?;
    let second_track = second.track_id.unwrap();
    assert_eq!(second.collectives.iter().map(|c| c.id).collect::<Vec<_>>(), [Some(crew)]);
    assert!(in_grouping(&lib.owner, crew).await?.0.contains(&second_track));
    assert!(!credited(&lib.owner, second_track).await?.iter().any(|n| n == "Mira Vale"));
    Ok(())
}
