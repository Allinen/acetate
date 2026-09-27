//! The catalog (`db::library`): what's in it for whom, and its row format.

use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{setup, Library};
use crate::db;
use crate::db::annotations::Item;

/// The ids in one part of `user`'s catalog (the first column of each row).
async fn ids(lib: &Library, user: Uuid, part: &str) -> anyhow::Result<Vec<Uuid>> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    let json = match part {
        "tracks" => db::library::tracks_json(&mut tx).await?,
        "albums" => db::library::albums_json(&mut tx).await?,
        "artists" => db::library::artists_json(&mut tx).await?,
        _ => db::library::annotations_json(&mut tx).await?,
    };
    let rows: Vec<Vec<Value>> = serde_json::from_str(&json)?;
    let column = if part == "annotations" { 1 } else { 0 };
    let mut ids: Vec<Uuid> = rows.iter().map(|r| r[column].as_str().unwrap().parse().unwrap()).collect();
    ids.sort();
    Ok(ids)
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

#[sqlx::test]
async fn each_user_gets_what_they_can_see(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // carol can't see H; bob (friends) can.
    assert_eq!(ids(&lib, lib.carol, "tracks").await?, sorted(vec![lib.v, lib.o]));
    assert_eq!(ids(&lib, lib.bob, "tracks").await?, sorted(vec![lib.v, lib.h, lib.o]));
    assert_eq!(ids(&lib, lib.carol, "albums").await?, vec![lib.album]);
    // Artists are visible to everyone logged in (X is only on H).
    assert_eq!(ids(&lib, lib.carol, "artists").await?, sorted(vec![lib.both, lib.x]));

    // Counts only count what the user sees: carol's X has no tracks, Both has V and O.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let artists: Vec<Vec<Value>> = serde_json::from_str(&db::library::artists_json(&mut tx).await?)?;
    let track_count = |id: Uuid| {
        let row = artists.iter().find(|r| r[0] == id.to_string()).unwrap();
        row[4].as_i64().unwrap()
    };
    assert_eq!((track_count(lib.both), track_count(lib.x)), (2, 0));
    // The album's tracks: V only, and V's row names the album and Both as primary.
    let albums: Vec<Vec<Value>> = serde_json::from_str(&db::library::albums_json(&mut tx).await?)?;
    assert_eq!(albums[0][9], 1, "track_count");
    let tracks: Vec<Vec<Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx).await?)?;
    let v = tracks.iter().find(|r| r[0] == lib.v.to_string()).unwrap();
    assert_eq!(v[4][0][0], lib.album.to_string());
    assert_eq!(v[3][0][0], lib.both.to_string());
    assert_eq!(v[3][0][1], "primary");
    // H's credit isn't in carol's catalog anywhere.
    let all = serde_json::to_string(&(tracks, albums, artists))?;
    assert!(!all.contains(&lib.h.to_string()));
    Ok(())
}

#[sqlx::test]
async fn annotations_are_the_users_own(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::annotations::set_starred(&mut tx, Item::Track(lib.h), true).await?;
    db::annotations::set_starred(&mut tx, Item::Album(lib.album), true).await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::annotations::played(&mut tx, lib.v).await?;
    tx.commit().await?;

    assert_eq!(ids(&lib, lib.bob, "annotations").await?, sorted(vec![lib.h, lib.album]));
    // A play counts for the track, its album and its artists too; none of it is bob's.
    let carols = ids(&lib, lib.carol, "annotations").await?;
    assert!(carols.contains(&lib.v) && !carols.contains(&lib.h), "{carols:?}");
    // bob loses access to H: his star on it is no longer in his catalog.
    sqlx::query("DELETE FROM user_groups WHERE user_id = $1 AND group_id = $2")
        .bind(lib.bob)
        .bind(lib.friends)
        .execute(&lib.owner)
        .await?;
    assert_eq!(ids(&lib, lib.bob, "annotations").await?, vec![lib.album]);
    Ok(())
}

#[sqlx::test]
async fn merged_tracks_are_left_out(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    assert_eq!(ids(&lib, lib.alice, "tracks").await?, sorted(vec![lib.v, lib.h]));
    Ok(())
}
