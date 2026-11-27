//! Lookups (`db::lookup`). The library: see `rls_tests::setup` (V and H on album A, H
//! restricted to friends, which bob is in and carol isn't; O on no album).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::lookup::PlatformId;

fn ids(value: &serde_json::Value, key: &str) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v[key].as_str().unwrap().to_string()).collect()
}

#[sqlx::test]
async fn files_are_found_by_their_bytes_where_the_user_can_see_them(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (v_sha, h_sha) = (crate::auth::token_hash("Song V"), crate::auth::token_hash("Song H"));
    let cover = "c".repeat(64);
    let unknown = "d".repeat(64);
    let mut tx = db::begin(&lib.owner, None).await?;
    sqlx::query("INSERT INTO covers (track_id, blob_sha256, source) VALUES ($1, $2, 'upload')").bind(lib.o).bind(&cover).execute(&mut *tx).await?;
    tx.commit().await?;

    let shas = vec![v_sha.clone(), h_sha.clone(), cover.clone(), unknown.clone()];
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let hits = db::lookup::files(&mut tx, &shas).await?;
    assert_eq!(hits.iter().map(|h| h.sha256.as_str()).collect::<Vec<_>>(), shas.iter().map(String::as_str).collect::<Vec<_>>(), "in the order asked");
    assert_eq!(ids(&hits[0].tracks, "track_id"), [lib.v.to_string()]);
    assert_eq!(hits[0].tracks[0]["current"], true);
    assert_eq!(ids(&hits[1].tracks, "track_id"), [lib.h.to_string()], "bob is a friend");
    assert_eq!(ids(&hits[2].covers, "track_id"), [lib.o.to_string()]);
    for empty in [&hits[3].tracks, &hits[3].extras, &hits[3].covers, &hits[3].problems] {
        assert_eq!(empty, &serde_json::json!([]));
    }
    tx.rollback().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let hits = db::lookup::files(&mut tx, &shas).await?;
    assert_eq!(hits[1].tracks, serde_json::json!([]), "carol can't see H");
    tx.rollback().await?;

    // `acetate verify` (as the owner): a cover's bytes are stored too.
    let mut conn = lib.owner.acquire().await?;
    assert_eq!(db::lookup::stored(&mut conn, &shas).await?, [(true, false), (true, false), (true, false), (false, false)]);
    Ok(())
}

#[sqlx::test]
async fn sources_are_found_by_platform_id(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    for (track, id) in [(lib.v, "1"), (lib.h, "2")] {
        sqlx::query("INSERT INTO sources (track_id, platform, external_id) VALUES ($1, 'soundcloud', $2)").bind(track).bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO sources (artist_id, platform, external_id, trashed_at) VALUES ($1, 'soundcloud', '1', now())").bind(lib.x).execute(&mut *tx).await?;
    tx.commit().await?;

    let asked: Vec<PlatformId> = ["1", "2", "3"].iter().map(|id| PlatformId { platform: "soundcloud".into(), external_id: id.to_string() }).collect();
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let hits = db::lookup::sources(&mut tx, &asked).await?;
    assert_eq!(hits[0].owners, serde_json::json!([{ "kind": "track", "id": lib.v }]), "not the trashed artist source");
    assert_eq!(hits[1].owners, serde_json::json!([]), "carol can't see H");
    assert_eq!(hits[2].owners, serde_json::json!([]));
    Ok(())
}

#[sqlx::test]
async fn tracks_are_found_by_title_and_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let found = |hits: Vec<db::lookup::TrackHit>| {
        let mut ids: Vec<_> = hits.into_iter().map(|h| h.id).collect();
        ids.sort();
        ids
    };
    let mut both = vec![lib.v, lib.h];
    both.sort();

    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(found(db::lookup::search_tracks(&mut tx, "song", None, 20).await?), both);
    assert_eq!(found(db::lookup::search_tracks(&mut tx, "song", Some("X"), 20).await?), [lib.h]);
    assert_eq!(found(db::lookup::search_tracks(&mut tx, "Song V", None, 20).await?).first(), Some(&lib.v));
    tx.rollback().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(found(db::lookup::search_tracks(&mut tx, "song", None, 20).await?), [lib.v]);
    Ok(())
}
