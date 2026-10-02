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
    assert_eq!(db::groupings::add(&mut tx, id, &[lib.v, lib.h], &[lib.album], "").await?, db::groupings::Changed { tracks: 2, albums: 1 });
    assert_eq!(db::groupings::add(&mut tx, id, &[lib.v], &[], "").await?, db::groupings::Changed::default(), "already in it");
    // Members, said by hand: Both is; X, credited only on H, isn't.
    assert_eq!(db::groupings::add_members(&mut tx, id, &[lib.both], "").await?, 1);
    db::groupings::edit(&mut tx, id, "collective", "Tide Crew", "friends posting together", "a description").await?;
    assert!(db::groupings::create(&mut tx, "Not A Kind", "x", "", "").await.is_err());
    tx.commit().await?;

    // Carol sees the grouping, and of what's in it only what she can see.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let d = db::groupings::detail(&mut tx, id).await?.expect("visible");
    assert_eq!((d.name.as_str(), d.description.as_str()), ("Tide Crew", "friends posting together"));
    assert_eq!(d.tracks, [lib.v]);
    assert_eq!(d.albums, [lib.album]);
    assert_eq!(d.members.iter().map(|m| (m.name.as_str(), m.tracks)).collect::<Vec<_>>(), [("Both", 1)], "the member, on the track she sees");
    assert!(d.credited.is_empty(), "X is only on H, which she can't see");
    assert!(d.changes.iter().all(|c| c.track_id != Some(lib.h)), "not a word about H");
    assert_eq!(d.changes.first().map(|c| c.action.as_str()), Some("edited"), "newest first");
    // The catalog: V's row names it, the list counts what she can see.
    let tracks: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx, Some(&[lib.v])).await?)?;
    let at = db::library::TRACK_COLUMNS.iter().position(|c| *c == "groupings").unwrap();
    assert_eq!(tracks[0][at], json!([id]));
    let list: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::groupings_json(&mut tx).await?)?;
    assert_eq!(list, [vec![json!(id), json!("collective"), json!("Tide Crew"), json!(1), json!(1), json!([lib.both])]]);
    // A delta since before it has the grouping, and V.
    let changes = db::library::changes_since(&mut tx, &version).await?;
    assert!(changes.groupings && changes.tracks.contains(&lib.v));
    drop(tx);

    // Bob sees H in it; taking it out is logged, and nothing's deleted but the membership.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let d = db::groupings::detail(&mut tx, id).await?.unwrap();
    assert_eq!(d.tracks.len(), 2);
    assert_eq!(d.credited.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["X"], "credited on H, not a member");
    assert_eq!(db::groupings::remove(&mut tx, id, &[lib.h], &[], "not theirs").await?, db::groupings::Changed { tracks: 1, albums: 0 });
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
    let preview = db::groupings::collective_preview(&mut tx, mira, None).await?;
    assert_eq!((preview.name.as_str(), preview.tracks, preview.albums, preview.sources.len()), ("Mira Vale", 1, 1, 1));
    assert_eq!(preview.tracks_left_without_primary.iter().map(|t| t.id).collect::<Vec<_>>(), [track]);
    assert_eq!(preview.albums_left_without_artists.len(), 1);
    drop(tx);

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let crew = db::groupings::turn_into_collective(&mut tx, mira, None, "").await?;
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

#[sqlx::test]
async fn a_single_and_its_only_track_go_together(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use crate::model::{AlbumData, AlbumMembership, TrackFacts};
    let lib = setup(connect).await?;
    // O alone on a single.
    let mut tx = db::begin(&lib.owner, None).await?;
    let single = db::revisions::create_album(&mut tx, &AlbumData { title: "O".into(), kind: "single".into(), ..Default::default() }, db::revisions::Source::Edit, "").await?;
    let facts = TrackFacts { status: "released".into(), albums: vec![AlbumMembership { album_id: single, disc_number: Some(1), track_number: Some(1) }], relations: Vec::new() };
    db::revisions::write_external(&mut tx, lib.o, None, &facts, db::revisions::Source::Edit, "").await?;
    let crew = db::groupings::create(&mut tx, "collective", "Crew", "", "").await?;
    let other = db::groupings::create(&mut tx, "era", "Early", "", "").await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    // The track brings its single; the single brings its track.
    assert_eq!(db::groupings::add(&mut tx, crew, &[lib.o], &[], "").await?, db::groupings::Changed { tracks: 1, albums: 1 });
    assert_eq!(db::groupings::add(&mut tx, other, &[], &[single], "").await?, db::groupings::Changed { tracks: 1, albums: 1 });
    // A track on a real album (A, with V and H) doesn't bring the album.
    assert_eq!(db::groupings::add(&mut tx, crew, &[lib.v], &[], "").await?, db::groupings::Changed { tracks: 1, albums: 0 });
    let d = db::groupings::detail(&mut tx, crew).await?.unwrap();
    let along = d.changes.iter().find(|c| c.album_id == Some(single)).unwrap();
    assert_eq!(along.detail["along_with"], json!(lib.o), "the history says why");
    // And out together.
    assert_eq!(db::groupings::remove(&mut tx, crew, &[], &[single], "").await?, db::groupings::Changed { tracks: 1, albums: 1 });
    tx.commit().await?;
    assert_eq!(in_grouping(&lib.owner, crew).await?, (vec![lib.v], vec![]));
    Ok(())
}

#[sqlx::test]
async fn an_artist_is_merged_into_an_existing_collective(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // The collective exists (its SoundCloud account, and already NetEase 10 too); another
    // import made an artist of the same account (NetEase 10).
    let mut tx = db::begin(&lib.owner, None).await?;
    let crew = db::groupings::create(&mut tx, "collective", "Tide Crew", "", "").await?;
    let soundcloud = sources::validate(Kind::Artist, NewSource { platform: "soundcloud".into(), source_id: None, captured_at: None, data: json!({ "artist_id": 77 }) })?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Grouping(crew), &soundcloud).await?;
    tx.commit().await?;
    let first = ingest_glass_harbor(&lib.owner, &store, "01 - Glass Harbor.flac", 1, 10).await?;
    let mira = first.artists[0].id.unwrap();
    let track = first.track_id.unwrap();
    let mut tx = db::begin(&lib.owner, None).await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Grouping(crew), &netease(Kind::Artist, "artist_id", 10)).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let preview = db::groupings::collective_preview(&mut tx, mira, Some(crew)).await?;
    assert_eq!(preview.into.as_ref().map(|n| n.title.as_str()), Some("Tide Crew"));
    let into = db::groupings::turn_into_collective(&mut tx, mira, Some(crew), "").await?;
    tx.commit().await?;
    assert_eq!(into, crew, "no new grouping");
    let groupings: i64 = sqlx::query_scalar("SELECT count(*) FROM groupings").fetch_one(&lib.owner).await?;
    assert_eq!(groupings, 1);
    assert!(in_grouping(&lib.owner, crew).await?.0.contains(&track));
    assert!(credited(&lib.owner, track).await?.is_empty());
    let accounts: Vec<String> = sqlx::query_scalar("SELECT platform || ':' || external_id FROM sources WHERE grouping_id = $1 ORDER BY 1").bind(crew).fetch_all(&lib.owner).await?;
    assert_eq!(accounts, ["netease:10", "soundcloud:77"], "the artist's account joined the collective's");
    Ok(())
}

/// Its extras go to the collective too: those the user can't see as well (the preview
/// lists the ones they can), and one linked to both keeps a single link.
#[sqlx::test]
async fn an_artists_extras_move_to_its_collective(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use crate::db::extras::{About, Access, Link};
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let first = ingest_glass_harbor(&lib.owner, &store, "01 - Glass Harbor.flac", 1, 10).await?;
    let mira = first.artists[0].id.unwrap();
    let track = first.track_id.unwrap();

    let mut tx = db::begin(&lib.owner, None).await?;
    let crew = db::groupings::create(&mut tx, "collective", "Tide Crew", "", "").await?;
    let (stems, _) = db::extras::create(&mut tx, "Stems", "stems", &About::default(), &[Link::Artist(mira), Link::Track(track)], None, "").await?;
    let (both, _) = db::extras::create(&mut tx, "Both", "sample", &About::default(), &[Link::Artist(mira), Link::Grouping(crew)], None, "").await?;
    let friends_only = Some(Access { restricted: true, groups: vec![lib.friends] });
    let (demo, _) = db::extras::create(&mut tx, "Demo", "clip", &About::default(), &[Link::Artist(mira)], friends_only, "").await?;
    tx.commit().await?;

    // Carol (not in friends) sees two of them move.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let preview = db::groupings::collective_preview(&mut tx, mira, Some(crew)).await?;
    assert_eq!(preview.extras.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Both", "Stems"]);
    db::groupings::turn_into_collective(&mut tx, mira, Some(crew), "").await?;
    let shown = db::groupings::detail(&mut tx, crew).await?.unwrap();
    assert_eq!(shown.extras.iter().map(|e| e.id).collect::<Vec<_>>(), [both, stems]);
    let logged = shown.changes.iter().find(|c| c.action == "from_artist").unwrap();
    let mut said: Vec<Uuid> = serde_json::from_value(logged.detail["extras"].clone())?;
    said.sort();
    let mut all = vec![stems, both, demo];
    all.sort();
    assert_eq!(said, all, "the log has the one she can't see too");
    tx.commit().await?;

    let mut tx = db::begin(&lib.owner, None).await?;
    assert!(db::extras::linked_to(&mut tx, Link::Artist(mira)).await?.is_empty(), "none left on the artist");
    assert_eq!(db::extras::linked_to(&mut tx, Link::Grouping(crew)).await?.len(), 3, "the restricted one moved too");
    let kinds = |e: db::extras::Extra| e.links.into_iter().map(|l| l.kind).collect::<Vec<_>>();
    assert_eq!(kinds(db::extras::get(&mut tx, both).await?.unwrap()), ["grouping"], "linked once");
    assert_eq!(kinds(db::extras::get(&mut tx, stems).await?.unwrap()), ["grouping", "track"], "the link kept its place");
    assert_eq!(db::extras::history(&mut tx, stems).await?[0].what, "moved to the collective");
    Ok(())
}

#[sqlx::test]
async fn undoing_a_merge_takes_the_kept_track_back_out(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let crew = db::groupings::create(&mut tx, "collective", "Crew", "", "").await?;
    let era = db::groupings::create(&mut tx, "era", "Early", "", "").await?;
    let mixtape = db::groupings::create(&mut tx, "project", "Mixtape", "", "").await?;
    // O is in all three; V was in the era already.
    db::groupings::add(&mut tx, crew, &[lib.o], &[], "").await?;
    db::groupings::add(&mut tx, era, &[lib.o, lib.v], &[], "").await?;
    db::groupings::add(&mut tx, mixtape, &[lib.o], &[], "").await?;
    let merged = db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    for g in [crew, era, mixtape] {
        assert!(in_grouping(&lib.owner, g).await?.0.contains(&lib.v), "the kept track joined them");
    }

    // Someone takes V out of the mixtape and puts it back: that's theirs now, not the merge's.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::groupings::remove(&mut tx, mixtape, &[lib.v], &[], "").await?;
    db::groupings::add(&mut tx, mixtape, &[lib.v], &[], "on purpose").await?;
    db::revisions::undo_track_merge(&mut tx, merged.merge_id, "").await?;
    tx.commit().await?;
    assert!(!in_grouping(&lib.owner, crew).await?.0.contains(&lib.v), "out of what the merge put it in");
    assert!(in_grouping(&lib.owner, era).await?.0.contains(&lib.v), "in what it was in before");
    assert!(in_grouping(&lib.owner, mixtape).await?.0.contains(&lib.v), "and what someone put it in since");
    assert!(in_grouping(&lib.owner, crew).await?.0.contains(&lib.o), "O is back, still in it");
    Ok(())
}
