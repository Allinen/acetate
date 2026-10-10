//! Annotations: personal, only on what the user can see, and plays count for albums and
//! artists too (same setup as `rls_tests`).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::annotations::{self, Item};
use crate::db::revisions::EditRejected;

#[sqlx::test]
async fn annotations_are_personal(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    annotations::played(&mut tx, lib.v).await?;
    annotations::set_starred(&mut tx, Item::Track(lib.v), true).await?;
    annotations::set_rating(&mut tx, Item::Album(lib.album), 7).await?;
    tx.commit().await?;

    // Carol's play counted for V, its album and its primary artist.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let v = annotations::get(&mut tx, Item::Track(lib.v)).await?;
    assert_eq!((v.play_count, v.starred), (1, true));
    assert!(v.last_played_at.is_some() && v.starred_at.is_some());
    let album = annotations::get(&mut tx, Item::Album(lib.album)).await?;
    assert_eq!((album.play_count, album.rating), (1, 7));
    assert_eq!(annotations::get(&mut tx, Item::Artist(lib.both)).await?.play_count, 1);
    // And in her play log.
    let plays: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT track_id FROM plays").fetch_all(&mut *tx).await?;
    assert_eq!(plays, [lib.v]);
    drop(tx);

    // Bob sees none of it.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert_eq!(annotations::get(&mut tx, Item::Track(lib.v)).await?, Default::default());
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM annotations").fetch_one(&mut *tx).await?;
    assert_eq!(rows, 0);
    let plays: i64 = sqlx::query_scalar("SELECT count(*) FROM plays").fetch_one(&mut *tx).await?;
    assert_eq!(plays, 0, "nor her plays");
    drop(tx);

    // Carol can't annotate H (restricted to friends), and a rating is 0 to 10.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let err = annotations::set_starred(&mut tx, Item::Track(lib.h), true).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::NotFound(_))), "{err:#}");
    drop(tx);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(annotations::set_rating(&mut tx, Item::Track(lib.v), 11).await.is_err());
    // 0 is allowed: not rated.
    annotations::set_rating(&mut tx, Item::Album(lib.album), 0).await?;
    Ok(())
}

#[sqlx::test]
async fn a_play_sent_twice_counts_once(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (key, now) = (uuid::Uuid::now_v7(), chrono::Utc::now());
    let yesterday = now - chrono::Duration::days(1);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    annotations::played_at(&mut tx, lib.v, now, None).await?;
    // Kept offline, sent later; then sent again (the answer was lost).
    annotations::played_at(&mut tx, lib.v, yesterday, Some(key)).await?;
    let again = annotations::played_at(&mut tx, lib.v, yesterday, Some(key)).await?;
    assert_eq!(again.play_count, 2);
    // The earlier play doesn't move the last one back, for the album either.
    let last = again.last_played_at.unwrap();
    assert!((last - now).num_milliseconds().abs() < 1, "{last} {now}");
    let album = annotations::get(&mut tx, Item::Album(lib.album)).await?;
    assert_eq!(album.play_count, 2);
    assert_eq!(album.last_played_at, again.last_played_at);
    let logged: Vec<(chrono::DateTime<chrono::Utc>, Option<String>)> =
        sqlx::query_as("SELECT played_at, imported_key FROM plays ORDER BY played_at").fetch_all(&mut *tx).await?;
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0].1, Some(format!("client:{key}")));
    // Not something she can see: not found.
    let err = annotations::played_at(&mut tx, lib.h, now, Some(uuid::Uuid::now_v7())).await.unwrap_err();
    assert!(matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::NotFound(_))), "{err:#}");
    Ok(())
}
