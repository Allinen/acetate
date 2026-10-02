//! Review flags the server raises itself: what a track is missing (migration 0030).
//!
//! The library (see `rls_tests::setup`): V and H on album A, O on no album; none has a
//! cover; the release date isn't checked by default.

use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::storage::BlobStore;

/// The open flags of a track: (kind, id), sorted by kind.
async fn open(lib: &Library, track: Uuid) -> anyhow::Result<Vec<(String, Uuid)>> {
    Ok(sqlx::query_as("SELECT kind, id FROM review_flags WHERE track_id = $1 AND resolved_at IS NULL ORDER BY kind")
        .bind(track)
        .fetch_all(&lib.owner)
        .await?)
}

async fn kinds(lib: &Library, track: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(open(lib, track).await?.into_iter().map(|(kind, _)| kind).collect())
}

/// A cover on `owner`, added by carol (committed). Returns its id.
async fn add_cover(lib: &Library, owner: CoverOwner) -> anyhow::Result<Uuid> {
    let dir = tempfile::tempdir()?;
    let sha = BlobStore::open(dir.path())?.put_bytes(Uuid::now_v7().as_bytes()).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let cover = NewCover { blob_sha256: &sha, mime: None, width: None, height: None, source: "upload" };
    let id = db::covers::add(&mut tx, owner, &cover).await?;
    tx.commit().await?;
    Ok(id)
}

#[sqlx::test]
async fn whats_missing_is_flagged(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    assert_eq!(kinds(&lib, lib.v).await?, ["missing_cover"]);
    assert_eq!(kinds(&lib, lib.h).await?, ["missing_cover"]);
    assert_eq!(kinds(&lib, lib.o).await?, ["missing_album", "missing_cover"]);
    // Checked once, at commit: making a track and then its credits and album didn't
    // raise "no artists" or "no album" on the way (flags settled a moment later).
    let settled: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE resolved_at IS NOT NULL")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(settled, 0);
    Ok(())
}

#[sqlx::test]
async fn a_fix_settles_the_flag(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let before = open(&lib, lib.v).await?;
    // A cover on the album: its tracks show it, so they aren't missing one anymore.
    add_cover(&lib, CoverOwner::Album(lib.album)).await?;
    assert!(kinds(&lib, lib.v).await?.is_empty());
    assert!(kinds(&lib, lib.h).await?.is_empty());
    assert_eq!(kinds(&lib, lib.o).await?, ["missing_album", "missing_cover"]);
    let (resolution, by): (String, Option<Uuid>) = sqlx::query_as("SELECT resolution, resolved_by FROM review_flags WHERE id = $1")
        .bind(before[0].1)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!((resolution.as_str(), by), ("fixed", Some(lib.carol)), "settled as fixed, by who fixed it");
    Ok(())
}

#[sqlx::test]
async fn marked_fine_stays_fine_until_it_comes_back(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // O really has no cover: alice says so.
    let flag = open(&lib, lib.o).await?.into_iter().find(|(k, _)| k == "missing_cover").unwrap().1;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    assert!(db::review::resolve(&mut tx, flag, "fine as it is").await?);
    tx.commit().await?;
    // Checked again (an edit to it): still missing a cover, but nobody's asked again.
    sqlx::query("UPDATE tracks SET title = 'Other (edited)' WHERE id = $1").bind(lib.o).execute(&lib.owner).await?;
    assert_eq!(kinds(&lib, lib.o).await?, ["missing_album"]);

    // It gets a cover, which later goes to the trash: missing again, so flagged again.
    let cover = add_cover(&lib, CoverOwner::Track(lib.o)).await?;
    assert_eq!(kinds(&lib, lib.o).await?, ["missing_album"]);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::covers::trash(&mut tx, cover).await?);
    tx.commit().await?;
    assert_eq!(kinds(&lib, lib.o).await?, ["missing_album", "missing_cover"]);
    Ok(())
}

#[sqlx::test]
async fn flags_are_in_the_catalog_and_its_deltas(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let issues = |rows: &str| -> Vec<(String, Vec<String>)> {
        let rows: Vec<Vec<Value>> = serde_json::from_str(rows).unwrap();
        rows.iter()
            .map(|r| {
                let kinds = r[1].as_array().unwrap().iter().map(|f| f[1].as_str().unwrap().to_string()).collect();
                (r[0].as_str().unwrap().to_string(), kinds)
            })
            .collect()
    };
    // carol's catalog: V and O (H is hidden from her), each with its flags.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let version = db::library::version(&mut tx).await?;
    let all = issues(&db::library::issues_json(&mut tx, None).await?);
    drop(tx);
    let o = all.iter().find(|(id, _)| *id == lib.o.to_string()).unwrap();
    assert_eq!(o.1, ["missing_album", "missing_cover"], "in review_kinds order");
    assert!(all.iter().any(|(id, _)| *id == lib.v.to_string()));
    assert!(!all.iter().any(|(id, _)| *id == lib.h.to_string()), "hidden from carol");

    // V gets a cover of its own: the flag is settled, and the delta has V, with no flags.
    add_cover(&lib, CoverOwner::Track(lib.v)).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &version).await?;
    assert!(changes.tracks.contains(&lib.v));
    let delta = issues(&db::library::issues_json(&mut tx, Some(&changes.tracks)).await?);
    assert_eq!(delta.iter().find(|(id, _)| *id == lib.v.to_string()).unwrap().1, Vec::<String>::new());
    Ok(())
}

#[sqlx::test]
async fn checks_turn_on_and_off(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    // No track has a release date.
    assert_eq!(db::review::set_checked(&mut tx, "missing_date", true).await?, 3);
    tx.commit().await?;
    assert_eq!(kinds(&lib, lib.v).await?, ["missing_cover", "missing_date"]);

    let mut tx = db::begin(&lib.owner, None).await?;
    assert_eq!(db::review::set_checked(&mut tx, "missing_date", false).await?, 0);
    tx.commit().await?;
    assert_eq!(kinds(&lib, lib.v).await?, ["missing_cover"]);
    let resolution: String = sqlx::query_scalar("SELECT resolution FROM review_flags WHERE kind = 'missing_date' LIMIT 1")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(resolution, "not checked anymore");

    // Only automatic ones can be turned off.
    let mut tx = db::begin(&lib.owner, None).await?;
    assert!(db::review::set_checked(&mut tx, "fix_later", false).await.is_err());
    Ok(())
}

#[sqlx::test]
async fn the_list_filters_automatic_kinds_and_tracks(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::review::flag_fix_later(&mut tx, lib.o, "check it").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let list = |filter: db::review::Filter<'static>| {
        let lib = &lib;
        async move {
            let mut tx = db::begin(&lib.app, Some(lib.alice)).await.unwrap();
            let mut kinds: Vec<String> = db::review::list(&mut tx, &filter, None, 100).await.unwrap().into_iter().map(|f| f.kind).collect();
            kinds.sort();
            kinds
        }
    };
    assert_eq!(list(db::review::Filter { automatic: Some(false), ..Default::default() }).await, ["fix_later"]);
    assert_eq!(list(db::review::Filter { track: Some(lib.o), ..Default::default() }).await, ["fix_later", "missing_album", "missing_cover"]);
    // The kinds, with names, for clients.
    let kinds = db::review::kinds(&mut tx).await?;
    let cover = kinds.iter().find(|k| k.kind == "missing_cover").unwrap();
    assert!(cover.automatic && cover.checked && cover.name == "No cover");
    Ok(())
}

#[sqlx::test]
async fn credits_in_the_title_are_flagged(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    for (title, flagged) in [("Song V (feat. X)", true), ("Song V w/ X", true), ("Song V [prod. X]", true), ("Defeat w/o you", false)] {
        sqlx::query("UPDATE tracks SET title = $2 WHERE id = $1").bind(lib.v).bind(title).execute(&lib.owner).await?;
        assert_eq!(kinds(&lib, lib.v).await?.contains(&"credits_in_title".to_string()), flagged, "{title}");
    }
    // Fixed (the title without them): settled.
    let settled: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE kind = 'credits_in_title' AND resolution = 'fixed'")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(settled, 1, "raised once while the title had credits, settled once");
    Ok(())
}

#[sqlx::test]
async fn a_source_that_says_too_little_is_flagged(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let weak = |lib: &Library| {
        let lib = lib.owner.clone();
        async move {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE track_id = $1 AND kind = 'weak_source' AND resolved_at IS NULL")
                .bind(lib_track(&lib).await?)
                .fetch_one(&lib)
                .await?;
            anyhow::Ok(n)
        }
    };
    async fn lib_track(pool: &sqlx::PgPool) -> anyhow::Result<Uuid> {
        Ok(sqlx::query_scalar("SELECT id FROM tracks WHERE title = 'Song V'").fetch_one(pool).await?)
    }
    assert_eq!(weak(&lib).await?, 0, "a label and a description say enough");

    // Its origin loses the description: a label alone.
    let source: Uuid = sqlx::query_scalar("SELECT id FROM sources WHERE track_id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let label_only = crate::model::sources::validate(
        crate::model::sources::Kind::Track,
        crate::model::sources::NewSource { platform: "custom".into(), source_id: Some(source), captured_at: None, data: serde_json::json!({ "label": "test" }) },
    )?;
    db::sources::add_snapshot(&mut tx, crate::model::sources::SourceOwner::Track(lib.v), &label_only).await?;
    tx.commit().await?;
    assert_eq!(weak(&lib).await?, 1);

    // A real source: settled.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let netease = crate::model::sources::validate(
        crate::model::sources::Kind::Track,
        crate::model::sources::NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: serde_json::json!({ "song_id": 12345 }) },
    )?;
    db::sources::add_snapshot(&mut tx, crate::model::sources::SourceOwner::Track(lib.v), &netease).await?;
    tx.commit().await?;
    assert_eq!(weak(&lib).await?, 0);
    let fixed: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE kind = 'weak_source' AND resolution = 'fixed'").fetch_one(&lib.owner).await?;
    assert_eq!(fixed, 1);
    Ok(())
}

#[sqlx::test]
async fn a_duplicate_merged_since_shows_where_it_went(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // O looks like V and H; V looks like H.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::review::flag_same_song(&mut tx, lib.o, &[lib.v, lib.h], "test").await?;
    db::review::flag_same_song(&mut tx, lib.v, &[lib.h], "test").await?;
    tx.commit().await?;
    // H was merged into V since (by hand here: only the redirect matters).
    sqlx::query("UPDATE tracks SET merged_into = $2 WHERE id = $1").bind(lib.h).bind(lib.v).execute(&lib.owner).await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let flags = db::review::list(&mut tx, &db::review::Filter { kind: Some("possible_duplicate"), ..Default::default() }, None, 100).await?;
    let named = |track: Uuid| -> Vec<String> {
        let flag = flags.iter().find(|f| f.track_id == track).unwrap();
        flag.related_tracks.as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect()
    };
    // O's names V once (H is V now); V's names nothing (H is V itself).
    assert_eq!(named(lib.o), [lib.v.to_string()]);
    assert!(named(lib.v).is_empty());
    Ok(())
}

#[sqlx::test]
async fn a_recheck_settles_other_versions(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    for (track, title) in [(lib.o, "Song"), (lib.v, "Song (Instrumental)"), (lib.h, "Song")] {
        sqlx::query("UPDATE tracks SET title = $2 WHERE id = $1").bind(track).bind(title).execute(&lib.owner).await?;
    }
    // From before the rule: O looks like V and H, V looks like O; and O has V's platform song id.
    let mut tx = db::begin(&lib.owner, None).await?;
    let flag = |track: Uuid, tracks: &[Uuid]| {
        sqlx::query_scalar::<_, Uuid>("INSERT INTO review_flags (kind, track_id, related) VALUES ('possible_duplicate', $1, $2) RETURNING id")
            .bind(track)
            .bind(serde_json::json!({ "tracks": tracks }))
    };
    let o_flag = flag(lib.o, &[lib.v, lib.h]).fetch_one(&mut *tx).await?;
    let v_flag = flag(lib.v, &[lib.o]).fetch_one(&mut *tx).await?;
    db::review::flag_same_song(&mut tx, lib.o, &[lib.v], "same netease song id 7").await?;
    let settled = db::review::recheck_duplicates(&mut tx, "test").await?;
    tx.commit().await?;

    // O and V, once (though flagged both ways).
    assert_eq!(settled.iter().map(|p| (p.track.0, p.other.0)).collect::<Vec<_>>(), [(lib.o, lib.v)]);
    let related = |id: Uuid| sqlx::query_scalar::<_, Value>("SELECT related FROM review_flags WHERE id = $1").bind(id).fetch_one(&lib.owner);
    // O's stays open for H; V's is settled.
    assert_eq!(related(o_flag).await?["tracks"], serde_json::json!([lib.h]));
    let resolution: Option<String> = sqlx::query_scalar("SELECT resolution FROM review_flags WHERE id = $1 AND resolved_at IS NOT NULL")
        .bind(v_flag)
        .fetch_optional(&lib.owner)
        .await?;
    assert_eq!(resolution.as_deref(), Some("test"));
    // The same song id isn't about titles: still open.
    let same_song: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE track_id = $1 AND related ? 'reason' AND resolved_at IS NULL")
        .bind(lib.o)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(same_song, 1);
    Ok(())
}
