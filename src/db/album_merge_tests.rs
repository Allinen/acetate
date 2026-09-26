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
use crate::model::{AlbumData, AlbumMembership, Credit, Relation, TrackFacts};

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
    assert_eq!((merged.tracks, merged.covers), (2, 2));

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
    // A keeps its own title and date. Covers: A's first, then all of B's in their order
    // (also the one with the same image as A's).
    let a = db::catalog::get_album(&mut tx, lib.album, false).await?.unwrap();
    assert_eq!((a.title.as_str(), a.release_date.as_deref()), ("Album A", None));
    assert_eq!(a.covers.iter().map(|c| c.id).collect::<Vec<_>>(), vec![split.a_cover, split.b_same, split.b_cover]);
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

#[sqlx::test]
async fn duplicate_albums_merges_alike_albums(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // B: "album a" with O and V, A: V and H. Same track count; lengths set below.
    let split = split_album(&lib).await?;
    let options = |merge| crate::duplicate_albums_command::Options {
        max_track_difference: 0,
        max_length_difference_ms: 2000,
        min_title_similarity: 0.6,
        any_artists: false,
        merge,
    };

    // 10 s apart: left alone.
    sqlx::query("UPDATE files SET duration_ms = 100000").execute(&lib.owner).await?;
    sqlx::query("UPDATE files SET duration_ms = 110000 FROM tracks t WHERE t.file_id = files.id AND t.id = $1")
        .bind(lib.o)
        .execute(&lib.owner)
        .await?;
    crate::duplicate_albums_command::run(&lib.owner, options(true)).await?;
    assert_eq!(merged_into(&lib, split.b).await?, None);

    // 2 s apart, but no album artist in common: left alone.
    sqlx::query("UPDATE files SET duration_ms = 102000 FROM tracks t WHERE t.file_id = files.id AND t.id = $1")
        .bind(lib.o)
        .execute(&lib.owner)
        .await?;
    crate::duplicate_albums_command::run(&lib.owner, options(true)).await?;
    assert_eq!(merged_into(&lib, split.b).await?, None);

    // Both by Both: listed, but only merged with --merge (into the older album, A).
    let mut tx = db::begin(&lib.owner, None).await?;
    for (album, title, date) in [(lib.album, "Album A", None), (split.b, "album a", Some("2020"))] {
        let data = AlbumData {
            title: title.into(),
            release_date: date.map(String::from),
            credits: vec![Credit { artist_id: lib.both, role: "primary".into(), alias_id: None }],
            ..Default::default()
        };
        db::revisions::write_album(&mut tx, album, None, &data, Source::Edit, "").await?;
    }
    tx.commit().await?;
    crate::duplicate_albums_command::run(&lib.owner, options(false)).await?;
    assert_eq!(merged_into(&lib, split.b).await?, None);
    crate::duplicate_albums_command::run(&lib.owner, options(true)).await?;
    assert_eq!(merged_into(&lib, split.b).await?, Some(lib.album));
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
    assert!(preview.goes_back.iter().any(|l| l == "2 covers back to \"album a\", in their old order"), "{:?}", preview.goes_back);
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

#[sqlx::test]
async fn covers_left_behind_by_older_merges_are_moved_and_undone(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let split = split_album(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let merged = db::album_merges::merge_album(&mut tx, split.b, lib.album, "").await?;
    // As an older merge left it: B's copy of A's image still on B (and not in the record).
    sqlx::query("UPDATE covers SET album_id = $2, position = 0 WHERE id = $1")
        .bind(split.b_same)
        .bind(split.b)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE album_merges SET moved = jsonb_set(moved, '{covers}', \
         (SELECT jsonb_agg(c) FROM jsonb_array_elements(moved -> 'covers') c WHERE c ->> 'id' <> $2::text)) \
         WHERE id = $1",
    )
    .bind(merged.merge_id)
    .bind(split.b_same)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // A dry run changes nothing; the fix moves it to the end of A's covers.
    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, true).await?;
    assert_eq!(covers_on(&lib, split.b).await?, vec![(split.b_same, 0)]);
    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, false).await?;
    assert_eq!(covers_on(&lib, split.b).await?, vec![]);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let a = db::catalog::get_album(&mut tx, lib.album, false).await?.unwrap();
    assert_eq!(a.covers.last().map(|c| c.id), Some(split.b_same));

    // The undo takes it back to B too.
    db::album_merges::undo_album_merge(&mut tx, merged.merge_id, "", true).await?;
    tx.commit().await?;
    let mut on_b: Vec<Uuid> = covers_on(&lib, split.b).await?.into_iter().map(|(id, _)| id).collect();
    on_b.sort();
    let mut expected = vec![split.b_same, split.b_cover];
    expected.sort();
    assert_eq!(on_b, expected);
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

#[sqlx::test]
async fn older_merges_of_singles_get_their_covers_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (single, o_cover) = single_with_track_cover(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let merged = db::album_merges::merge_album(&mut tx, single, lib.album, "").await?;
    // As an older merge left it: no copy (and none in the record).
    sqlx::query("DELETE FROM covers WHERE album_id = $1").bind(lib.album).execute(&mut *tx).await?;
    sqlx::query("UPDATE album_merges SET moved = moved - 'copied_covers' WHERE id = $1")
        .bind(merged.merge_id)
        .execute(&mut *tx)
        .await?;
    // And then O was merged into V, and (as older merges did) O's cover went to the trash,
    // recorded in the merge.
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    simulate_old_track_merge_trash(&lib, o_cover).await?;
    let trashed: bool = sqlx::query_scalar("SELECT trashed_at IS NOT NULL FROM covers WHERE id = $1")
        .bind(o_cover)
        .fetch_one(&lib.owner)
        .await?;
    assert!(trashed);

    // A dry run changes nothing; the fix finds O's cover through the track merge, and
    // gives the album a copy.
    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, true).await?;
    assert_eq!(album_images(&lib, lib.album).await?, vec![]);
    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, false).await?;
    assert_eq!(album_images(&lib, lib.album).await?, vec![(B_IMAGE.to_string(), false)]);
    // V (which had none) got O's cover back out of the trash.
    let v_covers: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, trashed_at IS NOT NULL FROM covers WHERE track_id = $1")
            .bind(lib.v)
            .fetch_all(&lib.owner)
            .await?;
    assert_eq!(v_covers, vec![(o_cover, false)]);
    // Running it again changes nothing.
    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, false).await?;
    assert_eq!(album_images(&lib, lib.album).await?.len(), 1);

    // Undoing the track merge still works: O gets its cover back.
    let track_merge: Uuid = sqlx::query_scalar("SELECT id FROM track_merges").fetch_one(&lib.owner).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::revisions::undo_track_merge(&mut tx, track_merge, "").await?;
    tx.commit().await?;
    let o_covers: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, trashed_at IS NOT NULL FROM covers WHERE track_id = $1")
            .bind(lib.o)
            .fetch_all(&lib.owner)
            .await?;
    assert_eq!(o_covers, vec![(o_cover, false)]);
    Ok(())
}

#[sqlx::test]
async fn merge_duplicates_keeps_the_only_covers(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // O looks like V: same title and length. Only O has a cover.
    let (_, o_cover) = single_with_track_cover(&lib).await?;
    sqlx::query("UPDATE tracks SET title = 'Song V' WHERE id = $1").bind(lib.o).execute(&lib.owner).await?;
    sqlx::query("UPDATE files SET duration_ms = 180000").execute(&lib.owner).await?;
    sqlx::query("INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2)")
        .bind(lib.v)
        .bind(json!({ "tracks": [lib.o] }))
        .execute(&lib.owner)
        .await?;
    let options = crate::merge_duplicates_command::Options { max_length_difference_ms: 1000, ignore_case: false, dry_run: false };
    crate::merge_duplicates_command::run(&lib.owner, options).await?;

    // V (the older one) is kept, with O's cover instead of none.
    let covers: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, trashed_at IS NOT NULL FROM covers WHERE track_id = $1")
            .bind(lib.v)
            .fetch_all(&lib.owner)
            .await?;
    assert_eq!(covers, vec![(o_cover, false)]);
    Ok(())
}

/// Makes the (only) track merge look like one made before every cover was kept: `cover`
/// in the trash since the merge's own moment, and listed in its record as trashed.
async fn simulate_old_track_merge_trash(lib: &Library, cover: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE covers SET trashed_at = (SELECT merged_at FROM track_merges) WHERE id = $1",
    )
    .bind(cover)
    .execute(&lib.owner)
    .await?;
    sqlx::query("UPDATE track_merges SET moved = jsonb_set(moved, '{covers_trashed}', $1)")
        .bind(json!([cover]))
        .execute(&lib.owner)
        .await?;
    Ok(())
}

#[sqlx::test]
async fn covers_older_track_merges_trashed_come_back_after_the_kept_ones(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // O (cover X) merged into V (cover Y), keeping V's: X went to the trash back then.
    let (_, x) = single_with_track_cover(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let y = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover(SAME_IMAGE)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    simulate_old_track_merge_trash(&lib, x).await?;
    // Another cover of V, trashed by hand later: stays in the trash.
    let mut tx = db::begin(&lib.owner, None).await?;
    let z = db::covers::add(&mut tx, CoverOwner::Track(lib.v), &cover(B_IMAGE)).await?;
    sqlx::query("UPDATE covers SET trashed_at = now() + interval '1 hour' WHERE id = $1").bind(z).execute(&mut *tx).await?;
    tx.commit().await?;

    crate::duplicate_albums_command::fix_merged_covers(&lib.owner, false).await?;
    let v_covers: Vec<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, trashed_at IS NOT NULL FROM covers WHERE track_id = $1 ORDER BY trashed_at IS NOT NULL, position",
    )
    .bind(lib.v)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(v_covers, vec![(y, false), (x, false), (z, true)]);
    Ok(())
}
