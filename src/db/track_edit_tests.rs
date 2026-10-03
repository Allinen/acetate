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
use crate::db::track_edits::{AddSource, CoverItem, CoversEdit, Edit, Edited, FactsEdit, Failure, GroupsEdit, May, MetaEdit, NewAlbum, NewArtist, NewGrouping, SourcesEdit, TrackEdit};

const ALL: May = May { relations: true, sources: true };
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
    Ok(TrackEdit { track_id: track, meta: Some(MetaEdit { variant_id: Some(variant), base_revision_id: Some(base), new_variant: None, label: None, data }), facts: None, along: true, sync: false, covers: None, sources: None, single: false, groups: None , flags: None })
}

async fn facts_edit(conn: &mut PgConnection, track: Uuid, change: impl FnOnce(&mut TrackFacts)) -> anyhow::Result<TrackEdit> {
    let (_, _, _, base, mut data) = state(conn, track).await?;
    change(&mut data);
    Ok(TrackEdit { track_id: track, meta: None, facts: Some(FactsEdit { base_revision_id: base, data }), along: true, sync: false, covers: None, sources: None, single: false, groups: None , flags: None })
}

fn edit(tracks: Vec<TrackEdit>) -> Edit {
    Edit { reason: "test".into(), new_artists: vec![], new_albums: vec![], new_groupings: vec![], artist_sources: vec![], new_review_kinds: vec![], tracks }
}

async fn run(lib: &Library, user: Uuid, edit: &Edit, keep: bool) -> anyhow::Result<Edited> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    let edited = db::track_edits::edit(&mut tx, edit, ALL, describe).await?;
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

    // Synced, with nothing else changing: the EP matches the track (its own date goes:
    // the track has none).
    let sync = TrackEdit { track_id: lib.o, meta: None, facts: None, along: true, sync: true, covers: None, sources: None, single: false, groups: None , flags: None };
    let edited = run(&lib, lib.bob, &edit(vec![sync]), false).await?;
    let along = &edited.tracks[0].along;
    assert_eq!(along.len(), 1, "{:?}", edited.tracks[0]);
    assert_eq!((along[0].after.title.as_str(), along[0].after.release_date.as_deref()), ("Other Thing", None));
    assert_eq!(along[0].after.credits.iter().map(|c| c.artist_id).collect::<Vec<_>>(), [lib.both]);

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
    let edited = db::track_edits::edit(&mut tx, &edit(vec![unrelated]), May { relations: false, sources: true }, describe).await?;
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

/// A single whose title isn't its only track's: `along:on` with nothing else changed makes
/// it match (as Review's "a single for each" made it, then renamed by hand).
#[sqlx::test]
async fn a_single_made_to_match_with_nothing_else_changed(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let made = db::review_bulk::make_singles(&mut tx, &[lib.o], "").await?;
    tx.commit().await?;
    let single = made.made[0].album_id;
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = AlbumData { title: "Old Single Name".into(), kind: "single".into(), ..Default::default() };
    db::revisions::write_album(&mut tx, single, None, &data, Source::Edit, "renamed by hand").await?;
    tx.commit().await?;

    let sync = TrackEdit { track_id: lib.o, meta: None, facts: None, along: true, sync: true, covers: None, sources: None, single: false, groups: None , flags: None };
    let edited = run(&lib, lib.bob, &edit(vec![sync]), true).await?;
    assert!(edited.tracks[0].error.is_none(), "{:?}", edited.tracks[0].error);
    assert_eq!(edited.tracks[0].along.len(), 1);
    assert_eq!(title(&lib, "albums", single).await?, "Other");
    let credited: Vec<Uuid> = sqlx::query_scalar("SELECT artist_id FROM album_credits WHERE album_id = $1").bind(single).fetch_all(&lib.owner).await?;
    assert_eq!(credited, [lib.both]);

    // Without sync, nothing changed: nothing goes along.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::revisions::write_album(&mut tx, single, None, &data, Source::Edit, "renamed again").await?;
    tx.commit().await?;
    let plain = TrackEdit { track_id: lib.o, meta: None, facts: None, along: true, sync: false, covers: None, sources: None, single: false, groups: None , flags: None };
    assert!(run(&lib, lib.bob, &edit(vec![plain]), true).await?.tracks[0].along.is_empty());
    assert_eq!(title(&lib, "albums", single).await?, "Old Single Name");
    Ok(())
}

/// O's covers (it has none), as the editor sends them.
fn covers_edit(track: Uuid, base: Vec<Uuid>, covers: Vec<CoverItem>) -> TrackEdit {
    TrackEdit { track_id: track, meta: None, facts: None, along: true, sync: false, covers: Some(CoversEdit { base, covers }), sources: None, single: false, groups: None , flags: None }
}

async fn covers_of(lib: &Library, owner: Uuid) -> anyhow::Result<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as("SELECT id, blob_sha256 FROM covers WHERE (track_id = $1 OR album_id = $1) AND trashed_at IS NULL ORDER BY position, id")
        .bind(owner)
        .fetch_all(&lib.owner)
        .await?)
}

async fn stage(lib: &Library, user: Uuid, sha: &str) -> anyhow::Result<()> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    db::covers::stage(&mut tx, sha, Some("image/png"), Some(10), Some(10)).await?;
    tx.commit().await?;
    Ok(())
}

#[sqlx::test]
async fn covers_are_staged_copied_ordered_and_trashed_with_the_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (pasted, on_v) = ("a".repeat(64), "b".repeat(64));
    let v_cover: Uuid = sqlx::query_scalar("INSERT INTO covers (track_id, blob_sha256, source) VALUES ($1, $2, 'upload') RETURNING id")
        .bind(lib.v)
        .bind(&on_v)
        .fetch_one(&lib.owner)
        .await?;
    stage(&lib, lib.bob, &pasted).await?;

    // A pasted image as the main cover, then a copy of V's.
    let new = covers_edit(lib.o, vec![], vec![CoverItem::Staged { staged: pasted.clone() }, CoverItem::Copy { copy_of: v_cover }]);
    let preview = run(&lib, lib.bob, &edit(vec![new]), false).await?;
    assert!(preview.tracks[0].error.is_none(), "{:?}", preview.tracks[0].error);
    assert!(!preview.tracks[0].flags.contains(&"missing_cover".to_string()), "{:?}", preview.tracks[0].flags);
    assert!(covers_of(&lib, lib.o).await?.is_empty(), "a preview keeps nothing");
    let new = covers_edit(lib.o, vec![], vec![CoverItem::Staged { staged: pasted.clone() }, CoverItem::Copy { copy_of: v_cover }]);
    run(&lib, lib.bob, &edit(vec![new]), true).await?;
    let covers = covers_of(&lib, lib.o).await?;
    assert_eq!(covers.iter().map(|c| c.1.clone()).collect::<Vec<_>>(), [pasted.clone(), on_v.clone()]);

    // Someone else's staged image isn't Carol's to use.
    let carols = covers_edit(lib.o, covers.iter().map(|c| c.0).collect(), vec![CoverItem::Staged { staged: pasted.clone() }]);
    assert!(run(&lib, lib.carol, &edit(vec![carols]), false).await?.tracks[0].error.is_some());

    // From covers it no longer has: stale, nothing written (its title neither).
    let mut conn = lib.owner.acquire().await?;
    let mut stale = meta_edit(&mut conn, lib.o, |m| m.title = "Renamed".into()).await?;
    stale.covers = Some(CoversEdit { base: vec![], covers: vec![] });
    let edited = run(&lib, lib.bob, &edit(vec![stale]), true).await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("stale"));
    assert_eq!(title(&lib, "tracks", lib.o).await?, "Other");

    // The copy made the main one, the pasted one to the trash.
    let reorder = covers_edit(lib.o, covers.iter().map(|c| c.0).collect(), vec![CoverItem::Kept { id: covers[1].0 }]);
    run(&lib, lib.bob, &edit(vec![reorder]), true).await?;
    assert_eq!(covers_of(&lib, lib.o).await?, [covers[1].clone()]);
    let trashed: bool = sqlx::query_scalar("SELECT trashed_at IS NOT NULL FROM covers WHERE id = $1").bind(covers[0].0).fetch_one(&lib.owner).await?;
    assert!(trashed);
    // Another track's cover can't be kept as if it were its own.
    let theirs = covers_edit(lib.o, vec![covers[1].0], vec![CoverItem::Kept { id: v_cover }]);
    assert_eq!(run(&lib, lib.bob, &edit(vec![theirs]), false).await?.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    Ok(())
}

#[sqlx::test]
async fn new_covers_go_along_to_albums_its_alone_on(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let ep = db::revisions::create_album(&mut tx, &AlbumData { title: "Other EP".into(), kind: "ep".into(), ..Default::default() }, Source::Edit, "").await?;
    tx.commit().await?;
    let on_ep = facts_edit(&mut conn, lib.o, |f| f.albums.push(AlbumMembership { album_id: ep, disc_number: None, track_number: None })).await?;
    run(&lib, lib.bob, &edit(vec![on_ep]), true).await?;
    let old = "c".repeat(64);
    sqlx::query("INSERT INTO covers (album_id, blob_sha256, source) VALUES ($1, $2, 'upload')").bind(ep).bind(&old).execute(&lib.owner).await?;
    let (main, second) = ("d".repeat(64), "e".repeat(64));
    stage(&lib, lib.bob, &main).await?;
    stage(&lib, lib.bob, &second).await?;

    let new = covers_edit(lib.o, vec![], vec![CoverItem::Staged { staged: main.clone() }, CoverItem::Staged { staged: second.clone() }]);
    let edited = run(&lib, lib.bob, &edit(vec![new]), true).await?;
    let along = &edited.tracks[0].along;
    assert_eq!(along.iter().map(|a| (a.album_id, a.covers)).collect::<Vec<_>>(), [(ep, 2)]);
    // The track's new main cover is the EP's main one too; the other after its own.
    let blobs: Vec<String> = covers_of(&lib, ep).await?.into_iter().map(|c| c.1).collect();
    assert_eq!(blobs, [main.clone(), old, second.clone()]);

    // Album A has two tracks to Bob: V's new cover stays V's.
    let v = covers_edit(lib.v, vec![], vec![CoverItem::Staged { staged: main.clone() }]);
    assert!(run(&lib, lib.bob, &edit(vec![v]), true).await?.tracks[0].along.is_empty());
    assert!(covers_of(&lib, lib.album).await?.is_empty());
    Ok(())
}

#[sqlx::test]
async fn sources_added_made_the_origin_and_trashed_with_the_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // O's file came from its custom source "test".
    let (old, file): (Uuid, Uuid) = sqlx::query_as("SELECT s.id, t.file_id FROM sources s JOIN tracks t ON t.id = s.track_id WHERE s.track_id = $1")
        .bind(lib.o)
        .fetch_one(&lib.owner)
        .await?;
    let stand_in = Uuid::from_u128(0x5);
    let sources = |trash: Vec<Uuid>, origin: Option<Uuid>| SourcesEdit {
        add: vec![AddSource { id: Some(stand_in), platform: "netease".into(), data: serde_json::json!({ "song_id": 123 }) }],
        trash,
        origin,
        ..Default::default()
    };
    let with = |s: SourcesEdit| TrackEdit { track_id: lib.o, meta: None, facts: None, along: true, sync: false, covers: None, sources: Some(s), single: false, groups: None , flags: None };

    // The file's origin can't go to the trash while it's the origin.
    let edited = run(&lib, lib.bob, &edit(vec![with(sources(vec![old], None))]), false).await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    // Its file from the new one, then the old one in the trash: fine.
    let edited = run(&lib, lib.bob, &edit(vec![with(sources(vec![old], Some(stand_in)))]), true).await?;
    assert!(edited.tracks[0].error.is_none(), "{:?}", edited.tracks[0].error);
    let (origin, platform): (Uuid, String) =
        sqlx::query_as("SELECT s.id, s.platform FROM files f JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id JOIN sources s ON s.id = ss.source_id WHERE f.id = $1")
            .bind(file)
            .fetch_one(&lib.owner)
            .await?;
    assert_eq!(platform, "netease");
    let trashed: bool = sqlx::query_scalar("SELECT trashed_at IS NOT NULL FROM sources WHERE id = $1").bind(old).fetch_one(&lib.owner).await?;
    assert!(trashed);

    // Without `edit_sources`: refused. Another track's source: refused.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let correct = SourcesEdit { origin: Some(origin), ..Default::default() };
    let edited = db::track_edits::edit(&mut tx, &edit(vec![with(correct)]), May { relations: true, sources: false }, describe).await?;
    tx.rollback().await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    let vs: Uuid = sqlx::query_scalar("SELECT id FROM sources WHERE track_id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    let edited = run(&lib, lib.bob, &edit(vec![with(SourcesEdit { trash: vec![vs], ..Default::default() })]), false).await?;
    assert_eq!(edited.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    Ok(())
}

/// Replacing every source of several tracks with one (and their files' origins), as
/// `source:=…` sends it: V and O get "google drive", the files come from it, their old
/// sources go. A track that had it already keeps that one (not trashed, though asked).
#[sqlx::test]
async fn every_source_replaced_with_one_on_many_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let drive = serde_json::json!({ "label": "google drive", "description": "a shared folder" });
    // O has it already.
    let mut tx = db::begin(&lib.owner, None).await?;
    let valid = crate::model::sources::validate(
        crate::model::sources::Kind::Track,
        crate::model::sources::NewSource { platform: "custom".into(), source_id: None, captured_at: None, data: drive.clone() },
    )?;
    let had = db::sources::add_snapshot(&mut tx, crate::model::sources::SourceOwner::Track(lib.o), &valid).await?.source_id;
    tx.commit().await?;
    let stand_in = Uuid::from_u128(0x7);
    let mut tracks = Vec::new();
    for track in [lib.v, lib.o] {
        let old: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM sources WHERE track_id = $1").bind(track).fetch_all(&lib.owner).await?;
        let sources = SourcesEdit {
            add: vec![AddSource { id: Some(stand_in), platform: "custom".into(), data: drive.clone() }],
            trash: old,
            origin: Some(stand_in),
            ..Default::default()
        };
        tracks.push(TrackEdit { track_id: track, meta: None, facts: None, along: true, sync: false, covers: None, sources: Some(sources), single: false, groups: None , flags: None });
    }
    let edited = run(&lib, lib.bob, &edit(tracks), true).await?;
    assert!(edited.tracks.iter().all(|t| t.error.is_none()), "{:?}", edited.tracks);
    for track in [lib.v, lib.o] {
        let left: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT s.id, ss.data FROM sources s JOIN source_snapshots ss ON ss.source_id = s.id WHERE s.track_id = $1 AND s.trashed_at IS NULL",
        )
        .bind(track)
        .fetch_all(&lib.owner)
        .await?;
        assert_eq!(left.iter().map(|l| l.1.clone()).collect::<Vec<_>>(), std::slice::from_ref(&drive), "{track}");
        let origin: Uuid = sqlx::query_scalar("SELECT ss.source_id FROM tracks t JOIN files f ON f.id = t.file_id JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id WHERE t.id = $1")
            .bind(track)
            .fetch_one(&lib.owner)
            .await?;
        assert_eq!(origin, left[0].0);
    }
    let o_now: Uuid = sqlx::query_scalar("SELECT id FROM sources WHERE track_id = $1 AND trashed_at IS NULL").bind(lib.o).fetch_one(&lib.owner).await?;
    assert_eq!(o_now, had);
    Ok(())
}

/// V (on Album A) renamed, a new cover, and a single of its own in the same edit: the
/// single takes the new title, its primary artists and the new cover; V stays on A too.
#[sqlx::test]
async fn a_single_of_its_own_as_the_edit_leaves_the_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let image = "f".repeat(64);
    stage(&lib, lib.bob, &image).await?;
    let mut change = meta_edit(&mut conn, lib.v, |m| m.title = "Song V (Edit)".into()).await?;
    change.covers = Some(CoversEdit { base: vec![], covers: vec![CoverItem::Staged { staged: image.clone() }] });
    change.single = true;

    let preview = run(&lib, lib.bob, &edit(vec![change]), false).await?;
    assert!(preview.tracks[0].single.is_some(), "{:?}", preview.tracks[0]);
    let singles = || async { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM albums WHERE kind = 'single'").fetch_one(&lib.owner).await };
    assert_eq!(singles().await?, 0, "a preview keeps nothing");

    let mut change = meta_edit(&mut conn, lib.v, |m| m.title = "Song V (Edit)".into()).await?;
    change.covers = Some(CoversEdit { base: vec![], covers: vec![CoverItem::Staged { staged: image.clone() }] });
    change.single = true;
    let edited = run(&lib, lib.bob, &edit(vec![change]), true).await?;
    let single = edited.tracks[0].single.expect("made");
    assert_eq!(title(&lib, "albums", single).await?, "Song V (Edit)");
    let credited: Vec<Uuid> = sqlx::query_scalar("SELECT artist_id FROM album_credits WHERE album_id = $1").bind(single).fetch_all(&lib.owner).await?;
    assert_eq!(credited, [lib.both]);
    let on: Vec<(Uuid, Option<i16>)> = sqlx::query_as("SELECT album_id, track_number FROM album_tracks WHERE track_id = $1 ORDER BY album_id = $2").bind(lib.v).bind(single).fetch_all(&lib.owner).await?;
    assert_eq!(on, [(lib.album, None), (single, Some(1))]);
    assert_eq!(covers_of(&lib, single).await?.into_iter().map(|c| c.1).collect::<Vec<_>>(), [image]);
    Ok(())
}

#[sqlx::test]
async fn groupings_made_joined_and_left_with_the_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let early = Uuid::from_u128(0xE);
    let with = |groups: GroupsEdit, single: bool| TrackEdit { track_id: lib.o, meta: None, facts: None, along: true, sync: false, covers: None, sources: None, single, groups: Some(groups), flags: None };
    let (owner, o) = (lib.owner.clone(), lib.o);
    let in_it = |grouping: Uuid| {
        let owner = owner.clone();
        async move { sqlx::query_scalar::<_, i64>("SELECT count(*) FROM grouping_tracks WHERE grouping_id = $1 AND track_id = $2").bind(grouping).bind(o).fetch_one(&owner).await }
    };
    // A new era, O in it, and a single of its own (which goes in too: alone on it).
    let new = Edit {
        new_groupings: vec![NewGrouping { id: early, kind: "era".into(), name: " Early ".into() }],
        ..edit(vec![with(GroupsEdit { add: vec![early], remove: vec![] }, true)])
    };
    let edited = run(&lib, lib.bob, &new, true).await?;
    assert!(edited.tracks[0].error.is_none(), "{:?}", edited.tracks[0].error);
    let grouping = edited.groupings[&early];
    let (kind, name): (String, String) = sqlx::query_as("SELECT kind, name FROM groupings WHERE id = $1").bind(grouping).fetch_one(&lib.owner).await?;
    assert_eq!((kind.as_str(), name.as_str()), ("era", "Early"));
    assert_eq!(in_it(grouping).await?, 1);
    let single = edited.tracks[0].single.unwrap();
    let album_in: i64 = sqlx::query_scalar("SELECT count(*) FROM grouping_albums WHERE grouping_id = $1 AND album_id = $2").bind(grouping).bind(single).fetch_one(&lib.owner).await?;
    assert_eq!(album_in, 1);

    // Out of it again (its single with it).
    run(&lib, lib.bob, &edit(vec![with(GroupsEdit { add: vec![], remove: vec![grouping] }, false)]), true).await?;
    assert_eq!(in_it(grouping).await?, 0);
    Ok(())
}

#[sqlx::test]
async fn imported_metadata_is_a_version_of_its_own(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let (shown, _, before, _, _) = state(&mut conn, lib.o).await?;
    // A new version, "SoundCloud": only what's in it, not shown.
    let data = TrackMetadata { title: "Other (SoundCloud)".into(), credits: vec![Credit { artist_id: lib.x, role: "primary".into(), alias_id: None }], ..Default::default() };
    let mut change = edit(vec![TrackEdit {
        meta: Some(MetaEdit { variant_id: None, base_revision_id: None, new_variant: Some("SoundCloud".into()), label: None, data: data.clone() }),
        ..meta_edit(&mut conn, lib.o, |_| {}).await?
    }]);
    let done = run(&lib, lib.alice, &change, true).await?;
    assert!(done.tracks[0].error.is_none(), "{:?}", done.tracks[0].error);
    let variants: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT v.label, (SELECT data FROM track_revisions r WHERE r.variant_id = v.id ORDER BY r.id DESC LIMIT 1) FROM track_variants v WHERE v.track_id = $1 ORDER BY v.id",
    )
    .bind(lib.o)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(variants.len(), 2);
    assert_eq!((variants[1].0.as_str(), serde_json::from_value::<TrackMetadata>(variants[1].1.clone())?), ("SoundCloud", data));
    let (still, _, now, _, _) = state(&mut conn, lib.o).await?;
    assert_eq!((still, now), (shown, before), "the shown version is as it was");

    // The version edited, renamed along with it (a draft's tags' version, imported over).
    change.tracks = vec![meta_edit(&mut conn, lib.o, |m| m.title = "Other (renamed)".into()).await?];
    change.tracks[0].meta.as_mut().unwrap().label = Some("YouTube".into());
    run(&lib, lib.alice, &change, true).await?;
    let label: String = sqlx::query_scalar("SELECT label FROM track_variants WHERE id = $1").bind(shown).fetch_one(&lib.owner).await?;
    assert_eq!((label.as_str(), title(&lib, "tracks", lib.o).await?.as_str()), ("YouTube", "Other (renamed)"));

    // Neither a version nor a new one: refused.
    change.tracks = vec![meta_edit(&mut conn, lib.o, |_| {}).await?];
    change.tracks[0].meta.as_mut().unwrap().variant_id = None;
    let done = run(&lib, lib.alice, &change, false).await?;
    assert_eq!(done.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    Ok(())
}

#[sqlx::test]
async fn artists_get_their_accounts_with_the_edit(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use crate::db::track_edits::ArtistSource;
    let lib = setup(connect).await?;
    let mut conn = lib.owner.acquire().await?;
    let new = Uuid::now_v7();
    let account = |artist_id: Uuid, id: i64| ArtistSource { artist_id, platform: "soundcloud".into(), data: serde_json::json!({ "artist_id": id, "artist_permalinks": ["someone"] }) };
    let credited = meta_edit(&mut conn, lib.o, |m| m.credits.push(Credit { artist_id: new, role: "featured".into(), alias_id: None })).await?;
    let change = Edit {
        new_artists: vec![NewArtist { id: new, name: "Uploader".into() }],
        artist_sources: vec![account(new, 77), account(lib.x, 78)],
        ..edit(vec![credited])
    };
    // Sources need their permission.
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let refused = db::track_edits::edit(&mut tx, &change, May { relations: true, sources: false }, describe).await;
    assert!(refused.is_err());
    drop(tx);

    let done = run(&lib, lib.alice, &change, true).await?;
    let made = done.artists[&new];
    // Found by them, as an import looks for them.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let found = db::sources::accounts(&mut tx, "soundcloud", &["77".into(), "78".into(), "79".into()]).await?;
    let found: Vec<(&str, Uuid, &str)> = found.iter().map(|o| (o.kind, o.id, o.account.as_str())).collect();
    assert_eq!(found, [("artist", made, "77"), ("artist", lib.x, "78")]);
    Ok(())
}

#[sqlx::test]
async fn flags_by_hand_of_kinds_people_make(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use crate::db::track_edits::FlagsEdit;
    let lib = setup(connect).await?;
    let flagged = |add: &[&str], settle: &[&str]| TrackEdit {
        track_id: lib.o,
        meta: None,
        facts: None,
        along: true,
        sync: false,
        covers: None,
        sources: None,
        single: false,
        groups: None,
        flags: Some(FlagsEdit { add: add.iter().map(|s| s.to_string()).collect(), settle: settle.iter().map(|s| s.to_string()).collect() }),
    };
    let open = |kind: &'static str| {
        let pool = lib.owner.clone();
        let track = lib.o;
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM review_flags WHERE track_id = $1 AND kind = $2 AND resolved_at IS NULL")
                .bind(track)
                .bind(kind)
                .fetch_one(&pool)
                .await
        }
    };

    // A new kind, made with the edit, the track flagged with it (and "Fix later").
    let change = Edit { new_review_kinds: vec!["Missing  SoundCloud metadata".into()], ..edit(vec![flagged(&["Missing  SoundCloud metadata", "fix_later"], &[])]) };
    let done = run(&lib, lib.alice, &change, true).await?;
    assert_eq!(done.review_kinds["Missing  SoundCloud metadata"], "missing_soundcloud_metadata");
    assert!(done.tracks[0].flags.contains(&"missing_soundcloud_metadata".to_string()), "{:?}", done.tracks[0].flags);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let kind = db::review::kinds(&mut tx).await?.into_iter().find(|k| k.kind == "missing_soundcloud_metadata").unwrap();
    assert!(kind.custom && !kind.automatic && kind.name == "Missing SoundCloud metadata");
    // Its name again (any case): refused.
    assert!(db::review::make_kind(&mut tx, "missing soundcloud METADATA").await.is_err());
    drop(tx);

    // Again: still one open flag. Then settled.
    run(&lib, lib.alice, &edit(vec![flagged(&["missing_soundcloud_metadata"], &[])]), true).await?;
    assert_eq!(open("missing_soundcloud_metadata").await?, 1);
    run(&lib, lib.alice, &edit(vec![flagged(&[], &["missing_soundcloud_metadata"])]), true).await?;
    assert_eq!((open("missing_soundcloud_metadata").await?, open("fix_later").await?), (0, 1));

    // Kinds the server raises aren't raised by hand.
    let done = run(&lib, lib.alice, &edit(vec![flagged(&["missing_cover"], &[])]), false).await?;
    assert_eq!(done.tracks[0].error.as_ref().map(|e| e.code.as_str()), Some("invalid"));
    Ok(())
}
