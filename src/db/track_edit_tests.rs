//! Editing many tracks in one go (`db::track_edits`).
//!
//! The library (see `rls_tests::setup`): V (Both) and H (X, Both; restricted to friends)
//! on album A, O (Both) on no album. Carol can't see H; Bob can.

use sqlx::PgConnection;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::revisions::{EditRejected, Source};
use crate::db::track_edits::{Edit, Edited, FactsEdit, Failure, MetaEdit, NewAlbum, NewArtist, TrackEdit};
use crate::model::{AlbumData, AlbumMembership, Credit, TrackFacts, TrackMetadata};

fn describe(err: anyhow::Error) -> Failure {
    let code = match err.downcast_ref::<EditRejected>() {
        Some(EditRejected::Stale { .. }) => "stale",
        Some(EditRejected::Invalid(_)) => "invalid",
        _ => "error",
    };
    Failure { code: code.into(), message: err.to_string() }
}

/// The track's shown version, its latest revision and metadata, and its latest facts revision.
async fn state(conn: &mut PgConnection, track: Uuid) -> anyhow::Result<(Uuid, Uuid, TrackMetadata, Uuid, TrackFacts)> {
    let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(track).fetch_one(&mut *conn).await?;
    let (meta_rev, meta): (Uuid, serde_json::Value) =
        sqlx::query_as("SELECT id, data FROM track_revisions WHERE variant_id = $1 ORDER BY id DESC LIMIT 1").bind(variant).fetch_one(&mut *conn).await?;
    let (facts_rev, facts): (Uuid, serde_json::Value) = sqlx::query_as(
        "SELECT id, data FROM track_revisions WHERE track_id = $1 AND kind = 'external' ORDER BY id DESC LIMIT 1",
    )
    .bind(track)
    .fetch_one(&mut *conn)
    .await?;
    Ok((variant, meta_rev, serde_json::from_value(meta)?, facts_rev, serde_json::from_value(facts)?))
}

/// An edit of `track`'s metadata, from its latest revision.
async fn meta_edit(conn: &mut PgConnection, track: Uuid, change: impl FnOnce(&mut TrackMetadata)) -> anyhow::Result<TrackEdit> {
    let (variant, base, mut data, _, _) = state(conn, track).await?;
    change(&mut data);
    Ok(TrackEdit { track_id: track, meta: Some(MetaEdit { variant_id: variant, base_revision_id: base, data }), facts: None, along: true })
}

async fn facts_edit(conn: &mut PgConnection, track: Uuid, change: impl FnOnce(&mut TrackFacts)) -> anyhow::Result<TrackEdit> {
    let (_, _, _, base, mut data) = state(conn, track).await?;
    change(&mut data);
    Ok(TrackEdit { track_id: track, meta: None, facts: Some(FactsEdit { base_revision_id: base, data }), along: true })
}

fn edit(tracks: Vec<TrackEdit>) -> Edit {
    Edit { reason: "test".into(), new_artists: vec![], new_albums: vec![], tracks }
}

async fn run(lib: &Library, user: Uuid, edit: &Edit, keep: bool) -> anyhow::Result<Edited> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    let edited = db::track_edits::edit(&mut tx, edit, true, describe).await?;
    if keep {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(edited)
}

async fn title(lib: &Library, table: &str, id: Uuid) -> anyhow::Result<String> {
    let sql = if table == "albums" { "SELECT title FROM albums WHERE id = $1" } else { "SELECT title FROM tracks WHERE id = $1" };
    Ok(sqlx::query_scalar(sql).bind(id).fetch_one(&lib.owner).await?)
}

async fn revisions(lib: &Library, track: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM track_revisions WHERE track_id = $1").bind(track).fetch_one(&lib.owner).await?)
}

#[sqlx::test]
async fn a_preview_says_what_would_happen_and_keeps_nothing(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let before = revisions(&lib, lib.o).await?;
    let change = meta_edit(&mut conn, lib.o, |m| m.title = "Other (feat. X)".into()).await?;
    let edited = run(&lib, lib.bob, &edit(vec![change]), false).await?;

    let o = &edited.tracks[0];
    assert!(o.error.is_none(), "{:?}", o.error);
    assert!(o.meta.is_some_and(|w| w.changed));
    // The checks ran: credits in the title now, and no album as before.
    assert!(o.flags.contains(&"credits_in_title".to_string()), "{:?}", o.flags);
    assert!(o.flags.contains(&"missing_album".to_string()), "{:?}", o.flags);
    // Nothing kept.
    assert_eq!(title(&lib, "tracks", lib.o).await?, "Other");
    assert_eq!(revisions(&lib, lib.o).await?, before);
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE track_id = $1 AND kind = 'credits_in_title'")
        .bind(lib.o)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(open, 0);
    Ok(())
}

#[sqlx::test]
async fn new_artists_and_albums_are_made_once_under_their_stand_ins(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let (artist, album) = (Uuid::from_u128(0xA), Uuid::from_u128(0xB));
    let feature = Credit { artist_id: artist, role: "featured".into(), alias_id: None };
    let mut tracks = Vec::new();
    for track in [lib.v, lib.o] {
        let mut change = meta_edit(&mut conn, track, |m| m.credits.push(feature.clone())).await?;
        let facts = facts_edit(&mut conn, track, |f| f.albums.push(AlbumMembership { album_id: album, disc_number: Some(1), track_number: None })).await?;
        change.facts = facts.facts;
        tracks.push(change);
    }
    let new_album = AlbumData { title: "New EP".into(), kind: "ep".into(), credits: vec![feature.clone()], ..Default::default() };
    let edit = Edit {
        new_artists: vec![NewArtist { id: artist, name: " New Person ".into() }],
        new_albums: vec![NewAlbum { id: album, data: new_album }],
        ..edit(tracks)
    };
    let edited = run(&lib, lib.bob, &edit, true).await?;
    assert!(edited.tracks.iter().all(|t| t.error.is_none()), "{:?}", edited.tracks);

    let real_artist = edited.artists[&artist];
    let real_album = edited.albums[&album];
    let name: String = sqlx::query_scalar("SELECT name FROM artists WHERE id = $1").bind(real_artist).fetch_one(&lib.owner).await?;
    assert_eq!(name, "New Person");
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM artists WHERE name = 'New Person'").fetch_one(&lib.owner).await?;
    assert_eq!(made, 1);
    for track in [lib.v, lib.o] {
        let credited: Vec<Uuid> = sqlx::query_scalar("SELECT artist_id FROM track_credits WHERE track_id = $1 ORDER BY position").bind(track).fetch_all(&lib.owner).await?;
        assert_eq!(credited, [lib.both, real_artist]);
        let on: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM album_tracks WHERE track_id = $1 AND album_id = $2)").bind(track).bind(real_album).fetch_one(&lib.owner).await?;
        assert!(on);
    }
    let album_credit: Uuid = sqlx::query_scalar("SELECT artist_id FROM album_credits WHERE album_id = $1").bind(real_album).fetch_one(&lib.owner).await?;
    assert_eq!(album_credit, real_artist);
    Ok(())
}

#[sqlx::test]
async fn a_track_that_fails_leaves_nothing_and_the_others_go(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    // O: new metadata (fine) and new facts from a base someone changed since (stale).
    let mut o = meta_edit(&mut conn, lib.o, |m| m.title = "Other, renamed".into()).await?;
    o.facts = facts_edit(&mut conn, lib.o, |f| f.status = "unreleased".into()).await?.facts;
    let (_, _, _, _, facts) = state(&mut conn, lib.o).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::revisions::write_external(&mut tx, lib.o, None, &TrackFacts { status: "demo".into(), ..facts }, Source::Edit, "meanwhile").await?;
    tx.commit().await?;
    let v = meta_edit(&mut conn, lib.v, |m| m.title = "Song V, renamed".into()).await?;

    let edited = run(&lib, lib.bob, &edit(vec![o, v]), true).await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("stale"));
    assert!(edited.tracks[1].error.is_none());
    // O's metadata, written before its facts failed, went with them.
    assert_eq!(title(&lib, "tracks", lib.o).await?, "Other");
    assert_eq!(title(&lib, "tracks", lib.v).await?, "Song V, renamed");
    Ok(())
}

#[sqlx::test]
async fn albums_its_alone_on_change_along_with_what_changed(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    // O on an EP of its own.
    let mut tx = db::begin(&lib.owner, None).await?;
    let ep = AlbumData { title: "Other EP".into(), kind: "ep".into(), release_date: Some("2018".into()), ..Default::default() };
    let ep = db::revisions::create_album(&mut tx, &ep, Source::Edit, "").await?;
    tx.commit().await?;
    let on_ep = facts_edit(&mut conn, lib.o, |f| f.albums.push(AlbumMembership { album_id: ep, disc_number: None, track_number: None })).await?;
    run(&lib, lib.bob, &edit(vec![on_ep]), true).await?;

    // A status change: nothing along.
    let status = facts_edit(&mut conn, lib.o, |f| f.status = "demo".into()).await?;
    assert!(run(&lib, lib.bob, &edit(vec![status]), true).await?.tracks[0].along.is_empty());

    // The title: the EP's title follows, its own date stays.
    let renamed = meta_edit(&mut conn, lib.o, |m| m.title = "Other Thing".into()).await?;
    let edited = run(&lib, lib.bob, &edit(vec![renamed]), true).await?;
    let along = &edited.tracks[0].along;
    assert_eq!(along.len(), 1);
    assert_eq!((along[0].before.title.as_str(), along[0].after.title.as_str()), ("Other EP", "Other Thing"));
    assert_eq!(title(&lib, "albums", ep).await?, "Other Thing");
    let date: Option<String> = sqlx::query_scalar("SELECT release_date FROM albums WHERE id = $1").bind(ep).fetch_one(&lib.owner).await?;
    assert_eq!(date.as_deref(), Some("2018"));

    // Turned off: the track alone.
    let mut quiet = meta_edit(&mut conn, lib.o, |m| m.title = "Other Again".into()).await?;
    quiet.along = false;
    run(&lib, lib.bob, &edit(vec![quiet]), true).await?;
    assert_eq!(title(&lib, "albums", ep).await?, "Other Thing");

    // Put on a new album in the same edit: that one isn't renamed after it.
    let mut tx = db::begin(&lib.owner, None).await?;
    let lp = db::revisions::create_album(&mut tx, &AlbumData { title: "Some LP".into(), ..Default::default() }, Source::Edit, "").await?;
    tx.commit().await?;
    let mut both = meta_edit(&mut conn, lib.o, |m| m.title = "Other Once More".into()).await?;
    both.facts = facts_edit(&mut conn, lib.o, |f| f.albums.push(AlbumMembership { album_id: lp, disc_number: None, track_number: None })).await?.facts;
    let edited = run(&lib, lib.bob, &edit(vec![both]), true).await?;
    assert_eq!(edited.tracks[0].along.iter().map(|a| a.album_id).collect::<Vec<_>>(), [ep]);
    assert_eq!(title(&lib, "albums", lp).await?, "Some LP");
    Ok(())
}

/// Album A has V and H; Carol can't see H. To her, V is alone on it: renaming V renames
/// the album too (the price of not telling her H is there). To Bob it has two tracks.
#[sqlx::test]
async fn alone_on_an_album_is_as_far_as_the_user_can_see(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let renamed = meta_edit(&mut conn, lib.v, |m| m.title = "Song V (Bob)".into()).await?;
    assert!(run(&lib, lib.bob, &edit(vec![renamed]), true).await?.tracks[0].along.is_empty());
    assert_eq!(title(&lib, "albums", lib.album).await?, "Album A");

    let renamed = meta_edit(&mut conn, lib.v, |m| m.title = "Song V (Carol)".into()).await?;
    let edited = run(&lib, lib.carol, &edit(vec![renamed]), true).await?;
    assert_eq!(edited.tracks[0].along.len(), 1);
    assert_eq!(title(&lib, "albums", lib.album).await?, "Song V (Carol)");
    // H itself wasn't touched.
    assert_eq!(title(&lib, "tracks", lib.h).await?, "Song H");
    Ok(())
}

#[sqlx::test]
async fn related_tracks_need_their_permission(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let unrelated = facts_edit(&mut conn, lib.v, |f| f.relations.clear()).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let edited = db::track_edits::edit(&mut tx, &edit(vec![unrelated]), false, describe).await?;
    tx.commit().await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    let related: i64 = sqlx::query_scalar("SELECT count(*) FROM track_relations WHERE from_track_id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    assert_eq!(related, 1);
    Ok(())
}

#[sqlx::test]
async fn an_artist_can_be_credited_in_two_roles(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let producer = Credit { artist_id: lib.both, role: "producer".into(), alias_id: None };
    let both_roles = meta_edit(&mut conn, lib.o, |m| m.credits.push(producer.clone())).await?;
    let edited = run(&lib, lib.bob, &edit(vec![both_roles]), true).await?;
    assert!(edited.tracks[0].error.is_none(), "{:?}", edited.tracks[0].error);
    let roles: Vec<String> = sqlx::query_scalar("SELECT role FROM track_credits WHERE track_id = $1 ORDER BY position").bind(lib.o).fetch_all(&lib.owner).await?;
    assert_eq!(roles, ["primary", "producer"]);

    // The same role twice is refused.
    let twice = meta_edit(&mut conn, lib.o, |m| m.credits.push(producer.clone())).await?;
    let edited = run(&lib, lib.bob, &edit(vec![twice]), true).await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    Ok(())
}

/// V's "Both" in the tags matched Both and X; ingest credited Both. Editing the credits
/// in a way that touches those two settles it; editing something else doesn't.
#[sqlx::test]
async fn editing_the_credits_settles_an_ambiguous_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::review::flag_ambiguous_artist(&mut tx, lib.v, "Both", lib.both, &[lib.both, lib.x]).await?;
    let producer = db::ingest::find_or_create_artist(&mut tx, "Producer").await?.id;
    tx.commit().await?;
    let open = || async {
        sqlx::query_as::<_, (bool, Option<String>)>("SELECT resolved_at IS NULL, resolution FROM review_flags WHERE track_id = $1 AND kind = 'ambiguous_artist'")
            .bind(lib.v)
            .fetch_one(&lib.owner)
            .await
    };
    let mut conn = lib.owner.acquire().await?;

    // A producer added: nobody decided who "Both" is.
    let produced = meta_edit(&mut conn, lib.v, |m| m.credits.push(Credit { artist_id: producer, role: "producer".into(), alias_id: None })).await?;
    run(&lib, lib.bob, &edit(vec![produced]), true).await?;
    assert!(open().await?.0);

    // Both replaced by X: settled, saying so.
    let replaced = meta_edit(&mut conn, lib.v, |m| m.credits[0].artist_id = lib.x).await?;
    run(&lib, lib.bob, &edit(vec![replaced]), true).await?;
    let (still_open, resolution) = open().await?;
    assert!(!still_open);
    assert_eq!(resolution.as_deref(), Some("credits edited: \"X\" credited"));
    Ok(())
}
