//! Merging albums, end to end (same setup as `rls_tests`).
//!
//! B is album A split off (another spelling and date). O is on B only (track 1), V on both
//! (track 5 on B). Both have a cover with the same image, B one more; B has a NetEase
//! album id and a share. Carol merges B into A.

use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::revisions::Source;
use crate::model::sources::{self, Kind, NewSource, SourceOwner};
use crate::model::{AlbumData, AlbumMembership, Credit, Relation, TrackFacts, TrackMetadata};

fn on(album_id: Uuid, track_number: Option<i16>) -> AlbumMembership {
    AlbumMembership { album_id, disc_number: None, track_number }
}

fn cover(sha: &str) -> NewCover<'_> {
    NewCover { blob_sha256: sha, mime: None, width: None, height: None, source: "upload" }
}

const SAME_IMAGE: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const B_IMAGE: &str = "3333333333333333333333333333333333333333333333333333333333333333";

struct Split {
    b: Uuid,
    a_cover: Uuid,
    /// B's copy of A's image, and B's own image.
    b_same: Uuid,
    b_cover: Uuid,
}

async fn split_album(lib: &Library) -> anyhow::Result<Split> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = AlbumData { title: "album a".into(), release_date: Some("2020".into()), ..Default::default() };
    let b = db::revisions::create_album(&mut tx, &data, Source::Import, "").await?;
    let facts = TrackFacts { albums: vec![on(b, Some(1))], ..Default::default() };
    db::revisions::write_external(&mut tx, lib.o, None, &facts, Source::Edit, "").await?;
    let facts = TrackFacts {
        albums: vec![on(lib.album, None), on(b, Some(5))],
        relations: vec![Relation { kind: "remix_of".into(), track_id: lib.h }],
        ..Default::default()
    };
    db::revisions::write_external(&mut tx, lib.v, None, &facts, Source::Edit, "").await?;

    let a_cover = db::covers::add(&mut tx, CoverOwner::Album(lib.album), &cover(SAME_IMAGE)).await?;
    let b_same = db::covers::add(&mut tx, CoverOwner::Album(b), &cover(SAME_IMAGE)).await?;
    let b_cover = db::covers::add(&mut tx, CoverOwner::Album(b), &cover(B_IMAGE)).await?;
    let source = sources::validate(
        Kind::Album,
        NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "album_id": 7 }) },
    )?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Album(b), &source).await?;
    sqlx::query("INSERT INTO shares (token_sha256, album_id, created_by) VALUES ($1, $2, $3)")
        .bind("d".repeat(64))
        .bind(b)
        .bind(lib.alice)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Split { b, a_cover, b_same, b_cover })
}

/// (track, track number) on an album, as the system sees it.
async fn tracks_on(lib: &Library, album: Uuid) -> anyhow::Result<Vec<(Uuid, Option<i16>)>> {
    Ok(sqlx::query_as("SELECT track_id, track_number FROM album_tracks WHERE album_id = $1 ORDER BY track_id")
        .bind(album)
        .fetch_all(&lib.owner)
        .await?)
}

#[sqlx::test]
async fn merging_albums_moves_everything_over(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let split = split_album(&lib).await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await?;
    tx.commit().await?;
    // O and V moved (V was on both); H is hidden from carol but stays on A too.
    assert_eq!((merged.tracks, merged.covers), (2, 1));

    // Tracks: O keeps its number; V keeps its place on A, not B's track 5.
    let mut expected = vec![(lib.v, None), (lib.h, None), (lib.o, Some(1))];
    expected.sort();
    assert_eq!(tracks_on(&lib, lib.album).await?, expected);
    assert_eq!(tracks_on(&lib, split.b).await?, vec![]);
    // Each moved track has a revision saying so.
    let reason: String = sqlx::query_scalar(
        "SELECT reason FROM track_revisions WHERE track_id = $1 AND kind = 'external' ORDER BY id DESC LIMIT 1",
    )
    .bind(lib.o)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(reason, "merged album \"album a\" into \"Album A\"");

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    // A keeps its own title and date. Covers: A's first, then B's in their order (not the
    // one with the same image as A's: that stays on B).
    let a = db::catalog::get_album(&mut tx, lib.album, false).await?.unwrap();
    assert_eq!((a.title.as_str(), a.release_date.as_deref()), ("Album A", None));
    assert_eq!(a.covers.iter().map(|c| c.id).collect::<Vec<_>>(), vec![split.a_cover, split.b_cover]);
    assert_eq!(covers_on(&lib, split.b).await?, vec![(split.b_same, 0)]);
    assert_eq!(a.sources.iter().map(|s| s.platform.as_str()).collect::<Vec<_>>(), vec!["netease"]);
    // B redirects, and isn't found by search anymore.
    assert_eq!(db::catalog::album_merged_into(&mut tx, split.b).await?, Some(lib.album));
    let hits = db::catalog::search_albums(&mut tx, "album a", 10).await?;
    assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![lib.album]);

    // An old album id still works: it means A (e.g. restoring an old revision).
    let facts = TrackFacts { albums: vec![on(split.b, Some(2))], ..Default::default() };
    db::revisions::write_external(&mut tx, lib.o, None, &facts, Source::Edit, "").await?;
    let o_albums: Vec<(Uuid, Option<i16>)> =
        sqlx::query_as("SELECT album_id, track_number FROM album_tracks WHERE track_id = $1")
            .bind(lib.o)
            .fetch_all(&mut *tx)
            .await?;
    assert_eq!(o_albums, vec![(lib.album, Some(2))]);

    // Merging again, or into itself: refused.
    assert!(db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await.is_err());
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::album_merges::merge_album(&mut tx, lib.album, lib.album, "").await.is_err());
    drop(tx);

    // Ingest finds A, not B: by B's NetEase id, and B's title + date find nothing.
    let mut tx = db::begin(&lib.owner, None).await?;
    let by_id = db::ingest::owners_with_platform_id(&mut tx, Kind::Album, "netease", "7").await?;
    assert_eq!(by_id, vec![lib.album]);
    assert_eq!(db::ingest::find_album(&mut tx, "album a", &[], Some("2020")).await?, None);
    drop(tx);

    // The share of B now opens A.
    let shared: Option<Uuid> = sqlx::query_scalar("SELECT album_id FROM shares").fetch_one(&lib.owner).await?;
    assert_eq!(shared, Some(lib.album));
    Ok(())
}

#[sqlx::test]
async fn a_merged_album_is_as_hidden_as_where_it_went(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Two albums with only H on them (restricted to friends): bob sees both, carol neither.
    let mut tx = db::begin(&lib.owner, None).await?;
    let mut albums = Vec::new();
    for title in ["Secret", "Secret (Deluxe)"] {
        let data = AlbumData { title: title.into(), ..Default::default() };
        albums.push(db::revisions::create_album(&mut tx, &data, Source::Import, "").await?);
    }
    let facts = TrackFacts {
        albums: vec![on(lib.album, None), on(albums[0], None), on(albums[1], None)],
        relations: vec![Relation { kind: "remix_to".into(), track_id: lib.v }], // (unchanged)
        ..Default::default()
    };
    db::revisions::write_external(&mut tx, lib.h, None, &facts, Source::Edit, "").await?;
    tx.commit().await?;

    // Carol can't merge what she can't see.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::album_merges::merge_album(&mut tx, albums[0], albums[1], "").await.is_err());
    drop(tx);

    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::album_merges::merge_album(&mut tx, albums[0], albums[1], "").await?;
    tx.commit().await?;

    // "Secret" has no tracks now, but it isn't visible to everyone like an empty album:
    // only to who can see where it went.
    for (user, visible) in [(lib.bob, true), (lib.carol, false)] {
        let mut tx = db::begin(&lib.app, Some(user)).await?;
        let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM albums WHERE id = ANY($1)")
            .bind(&albums)
            .fetch_one(&mut *tx)
            .await?;
        assert_eq!(seen, if visible { 2 } else { 0 }, "user {user}");
    }
    Ok(())
}

async fn merged_into(lib: &Library, album: Uuid) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar("SELECT merged_into FROM albums WHERE id = $1")
        .bind(album)
        .fetch_one(&lib.owner)
        .await?)
}

/// Covers of an album with their positions, as the system sees them.
async fn covers_on(lib: &Library, album: Uuid) -> anyhow::Result<Vec<(Uuid, i16)>> {
    Ok(sqlx::query_as("SELECT id, position FROM covers WHERE album_id = $1 ORDER BY id")
        .bind(album)
        .fetch_all(&lib.owner)
        .await?)
}

#[sqlx::test]
async fn undoing_right_away_puts_everything_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let split = split_album(&lib).await?;
    let before = (
        tracks_on(&lib, lib.album).await?,
        tracks_on(&lib, split.b).await?,
        covers_on(&lib, lib.album).await?,
        covers_on(&lib, split.b).await?,
    );

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await?;
    tx.commit().await?;

    // The preview changes nothing, and says every track goes back.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let preview = db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", false).await?;
    drop(tx);
    assert!(!preview.done && preview.blockers.is_empty(), "{preview:?}");
    assert!(preview.tracks.iter().all(|t| t.goes_back), "{:?}", preview.tracks);
    let v = preview.tracks.iter().find(|t| t.id == lib.v).unwrap();
    assert_eq!(v.detail, "back on \"album a\" (track 5); stays on \"Album A\" (no track number), as it was on both before");
    let o = preview.tracks.iter().find(|t| t.id == lib.o).unwrap();
    assert_eq!(o.detail, "back on \"album a\" (track 1), off \"Album A\"");
    assert!(preview.goes_back.iter().any(|l| l == "1 cover back to \"album a\", in their old order"), "{:?}", preview.goes_back);
    assert_eq!(merged_into(&lib, split.b).await?, Some(lib.album));

    // Undone: everything as it was before the merge.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", true).await?;
    tx.commit().await?;
    assert!(report.done);
    let after = (
        tracks_on(&lib, lib.album).await?,
        tracks_on(&lib, split.b).await?,
        covers_on(&lib, lib.album).await?,
        covers_on(&lib, split.b).await?,
    );
    assert_eq!(after, before);
    assert_eq!(merged_into(&lib, split.b).await?, None);
    let b_sources: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE album_id = $1")
        .bind(split.b)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(b_sources, 1);
    let shared: Option<Uuid> = sqlx::query_scalar("SELECT album_id FROM shares").fetch_one(&lib.owner).await?;
    assert_eq!(shared, Some(split.b));

    // Listed as undone on A's page, and can't be undone twice.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merges = db::album_merges::merges_into(&mut tx, lib.album).await?;
    assert!(merges[0].undone_at.is_some());
    assert!(db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", true).await.is_err());
    Ok(())
}

#[sqlx::test]
async fn undoing_later_leaves_what_changed_since(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let split = split_album(&lib).await?;
    // H (hidden from carol) is on B too.
    let mut tx = db::begin(&lib.owner, None).await?;
    let facts = TrackFacts {
        albums: vec![on(lib.album, None), on(split.b, Some(2))],
        relations: vec![Relation { kind: "remix_to".into(), track_id: lib.v }],
        ..Default::default()
    };
    db::revisions::write_external(&mut tx, lib.h, None, &facts, Source::Edit, "").await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await?;
    // Since then: O was renumbered on A.
    let facts = TrackFacts { albums: vec![on(lib.album, Some(9))], ..Default::default() };
    db::revisions::write_external(&mut tx, lib.o, None, &facts, Source::Edit, "").await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", true).await?;
    tx.commit().await?;
    // O stays (and the report says why); V goes back; H goes back too, but carol only
    // gets a count.
    let o = report.tracks.iter().find(|t| t.id == lib.o).unwrap();
    assert!(!o.goes_back);
    assert_eq!(
        o.detail,
        "its place on \"Album A\" was changed since (now track 9; the merge left track 1), so it stays"
    );
    assert!(report.tracks.iter().find(|t| t.id == lib.v).unwrap().goes_back);
    assert!(report.tracks.iter().all(|t| t.id != lib.h));
    assert_eq!((report.hidden_tracks_back, report.hidden_tracks_staying), (1, 0));

    assert_eq!(tracks_on(&lib, split.b).await?, {
        let mut on_b = vec![(lib.v, Some(5)), (lib.h, Some(2))];
        on_b.sort();
        on_b
    });
    let o_albums: Vec<(Uuid, Option<i16>)> =
        sqlx::query_as("SELECT album_id, track_number FROM album_tracks WHERE track_id = $1")
            .bind(lib.o)
            .fetch_all(&lib.owner)
            .await?;
    assert_eq!(o_albums, vec![(lib.album, Some(9))]);
    Ok(())
}

#[sqlx::test]
async fn merges_are_undone_newest_first(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let split = split_album(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = AlbumData { title: "Album A (Deluxe)".into(), ..Default::default() };
    let c = db::revisions::create_album(&mut tx, &data, Source::Import, "").await?;
    let first = db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await?;
    let second = db::album_merges::merge_album(&mut tx, lib.album, c, "").await?;
    tx.commit().await?;
    // B led to A, and now to C.
    assert_eq!(merged_into(&lib, split.b).await?, Some(c));

    let mut tx = db::begin(&lib.owner, None).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, first.merge_id, "", false).await?;
    assert_eq!(report.blockers, vec!["\"Album A\" was merged into \"Album A (Deluxe)\" afterwards: undo that merge first"]);
    assert!(db::album_merges::undo_album_merge(&mut tx, first.merge_id, "", true).await.is_err());
    drop(tx);

    let mut tx = db::begin(&lib.owner, None).await?;
    db::album_merges::undo_album_merge(&mut tx, second.merge_id, "", true).await?;
    // B leads to A again, and then that one can be undone too.
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT merged_into FROM albums WHERE id = $1")
            .bind(split.b)
            .fetch_one(&mut *tx)
            .await?,
        Some(lib.album)
    );
    db::album_merges::undo_album_merge(&mut tx, first.merge_id, "", true).await?;
    tx.commit().await?;
    assert_eq!(merged_into(&lib, split.b).await?, None);
    assert_eq!(merged_into(&lib, lib.album).await?, None);
    assert_eq!(tracks_on(&lib, c).await?, vec![]);
    Ok(())
}

/// A single "Other" with only O on it; O has a cover of its own (the album has none).
/// Returns (single, O's cover).
async fn single_with_track_cover(lib: &Library) -> anyhow::Result<(Uuid, Uuid)> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = AlbumData { title: "Other".into(), kind: "single".into(), ..Default::default() };
    let single = db::revisions::create_album(&mut tx, &data, Source::Import, "").await?;
    let facts = TrackFacts { albums: vec![on(single, Some(1))], ..Default::default() };
    db::revisions::write_external(&mut tx, lib.o, None, &facts, Source::Edit, "").await?;
    let o_cover = db::covers::add(&mut tx, CoverOwner::Track(lib.o), &cover(B_IMAGE)).await?;
    tx.commit().await?;
    Ok((single, o_cover))
}

/// The images of an album's own covers, in order (trashed ones marked).
async fn album_images(lib: &Library, album: Uuid) -> anyhow::Result<Vec<(String, bool)>> {
    Ok(sqlx::query_as(
        "SELECT blob_sha256, trashed_at IS NOT NULL FROM covers WHERE album_id = $1 ORDER BY position, id",
    )
    .bind(album)
    .fetch_all(&lib.owner)
    .await?)
}

#[sqlx::test]
async fn a_single_passes_on_the_cover_from_its_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (single, _) = single_with_track_cover(&lib).await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album(&mut tx, single, lib.album, "").await?;
    tx.commit().await?;
    assert_eq!(merged.covers, 1);
    assert_eq!(album_images(&lib, lib.album).await?, vec![(B_IMAGE.to_string(), false)]);

    // The undo says it, and puts the copy in the trash.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let report = db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", true).await?;
    tx.commit().await?;
    assert!(report.goes_back.iter().any(|l| l.contains("copy of \"Other\"'s cover")), "{:?}", report.goes_back);
    assert_eq!(album_images(&lib, lib.album).await?, vec![(B_IMAGE.to_string(), true)]);
    Ok(())
}

/// A new album of `kind` with the given tracks (title, track number), made as ingest does.
async fn album_with(lib: &Library, title: &str, kind: &str, tracks: &[(&str, Option<i16>)]) -> anyhow::Result<(Uuid, Vec<Uuid>)> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = AlbumData { title: title.into(), kind: kind.into(), ..Default::default() };
    let album = db::revisions::create_album(&mut tx, &data, Source::Import, "").await?;
    let mut ids = Vec::new();
    for (track, number) in tracks {
        // (Named apart first: its fake file's bytes are made from the name, and two files
        // can't be the same. Then its title.)
        let (id, _) = super::rls_tests::make_track(&mut tx, &format!("{track} {}", Uuid::now_v7()), &[lib.both], None).await?;
        let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(id).fetch_one(&mut *tx).await?;
        let meta = TrackMetadata {
            title: track.to_string(),
            credits: vec![Credit { artist_id: lib.both, role: "primary".into(), alias_id: None }],
            ..Default::default()
        };
        db::revisions::write_internal(&mut tx, id, variant, None, &meta, Source::Edit, "", false).await?;
        let facts = TrackFacts { albums: vec![on(album, *number)], ..Default::default() };
        db::revisions::write_external(&mut tx, id, None, &facts, Source::Edit, "").await?;
        ids.push(id);
    }
    tx.commit().await?;
    Ok((album, ids))
}

async fn track_merged_into(lib: &Library, track: Uuid) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar("SELECT merged_into FROM tracks WHERE id = $1").bind(track).fetch_one(&lib.owner).await?)
}

#[sqlx::test]
async fn merging_two_tracks_merges_their_singles(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (kept_single, kept) = album_with(&lib, "Song", "single", &[("Song", None)]).await?;
    let (dupe_single, dupe) = album_with(&lib, "Song", "single", &[("Song", Some(1))]).await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged =
        db::album_merges::merge_tracks_and_singles(&mut tx, kept[0], dupe[0], &Default::default(), None, None, "", true).await?;
    tx.commit().await?;
    assert!(merged.singles_merged.is_some());
    assert_eq!(merged_into(&lib, dupe_single).await?, Some(kept_single));
    assert_eq!(tracks_on(&lib, kept_single).await?, vec![(kept[0], None)]);

    // Undoing goes newest first: the singles' merge, then the tracks'.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let blockers = db::revisions::merge_undo_blockers(&mut tx, merged.merge_id).await?;
    assert_eq!(blockers, vec!["an album it's on was merged into \"Song\" since (undo that first, on \"Song\"'s page)"]);
    let album_merge = merged.singles_merged.unwrap().merge_id;
    db::album_merges::undo_album_merge(&mut tx, album_merge, "", true).await?;
    assert_eq!(db::revisions::merge_undo_blockers(&mut tx, merged.merge_id).await?, Vec::<String>::new());
    db::revisions::undo_track_merge(&mut tx, merged.merge_id, "").await?;
    tx.commit().await?;
    assert_eq!(tracks_on(&lib, dupe_single).await?, vec![(dupe[0], Some(1))]);
    Ok(())
}

#[sqlx::test]
async fn singles_with_more_on_them_stay(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (_, kept) = album_with(&lib, "Song", "single", &[("Song", None)]).await?;
    let (dupe_single, dupe) = album_with(&lib, "Song", "single", &[("Song", Some(1))]).await?;
    // H is on the duplicate's single too; carol can't see it, but it counts.
    let mut tx = db::begin(&lib.owner, None).await?;
    let facts = TrackFacts { albums: vec![on(lib.album, None), on(dupe_single, Some(2))], ..Default::default() };
    db::revisions::write_external(&mut tx, lib.h, None, &facts, Source::Edit, "").await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged =
        db::album_merges::merge_tracks_and_singles(&mut tx, kept[0], dupe[0], &Default::default(), None, None, "", true).await?;
    tx.commit().await?;
    assert!(merged.singles_merged.is_none());
    assert_eq!(merged_into(&lib, dupe_single).await?, None);

    // Nor an album of another kind, nor with the option off.
    let (_, kept) = album_with(&lib, "Other", "album", &[("Other", Some(1))]).await?;
    let (_, dupe) = album_with(&lib, "Other", "single", &[("Other", Some(1))]).await?;
    let (_, kept2) = album_with(&lib, "Third", "single", &[("Third", Some(1))]).await?;
    let (_, dupe2) = album_with(&lib, "Third", "single", &[("Third", Some(1))]).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let one = db::album_merges::merge_tracks_and_singles(&mut tx, kept[0], dupe[0], &Default::default(), None, None, "", true).await?;
    let two = db::album_merges::merge_tracks_and_singles(&mut tx, kept2[0], dupe2[0], &Default::default(), None, None, "", false).await?;
    assert!(one.singles_merged.is_none() && two.singles_merged.is_none());
    Ok(())
}

#[sqlx::test]
async fn merging_albums_with_the_same_tracklist_merges_the_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (from, from_tracks) = album_with(&lib, "EP", "ep", &[("One", Some(1)), ("Two", Some(2))]).await?;
    let (into, into_tracks) = album_with(&lib, "EP", "ep", &[("one", Some(1)), ("Two ", Some(2))]).await?;
    // The merged one's second track has the better file.
    sqlx::query("UPDATE files SET codec = 'flac' WHERE id = (SELECT file_id FROM tracks WHERE id = $1)")
        .bind(from_tracks[1])
        .execute(&lib.owner)
        .await?;
    let better: Uuid = sqlx::query_scalar("SELECT file_id FROM tracks WHERE id = $1").bind(from_tracks[1]).fetch_one(&lib.owner).await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album_and_tracks(&mut tx, from, into, "", true).await?;
    tx.commit().await?;
    assert_eq!(merged.tracks_merged, 2);
    for (a, b) in from_tracks.iter().zip(&into_tracks) {
        assert_eq!(track_merged_into(&lib, *a).await?, Some(*b));
    }
    assert_eq!(tracks_on(&lib, into).await?.len(), 2);
    // Each kept track keeps its own title; the better file is kept.
    let (title, file): (String, Uuid) =
        sqlx::query_as("SELECT title, file_id FROM tracks WHERE id = $1").bind(into_tracks[1]).fetch_one(&lib.owner).await?;
    assert_eq!((title.as_str(), file), ("Two ", better));

    // Different tracklists: only the albums merge.
    let (from, from_tracks) = album_with(&lib, "LP", "album", &[("One", Some(1)), ("Two", Some(2))]).await?;
    let (into, _) = album_with(&lib, "LP", "album", &[("One", Some(1)), ("Three", Some(2))]).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let merged = db::album_merges::merge_album_and_tracks(&mut tx, from, into, "", true).await?;
    tx.commit().await?;
    assert_eq!(merged.tracks_merged, 0);
    assert_eq!(track_merged_into(&lib, from_tracks[0]).await?, None);
    assert_eq!(tracks_on(&lib, into).await?.len(), 4);
    Ok(())
}
