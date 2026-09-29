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
        "tracks" => db::library::tracks_json(&mut tx, None).await?,
        "albums" => db::library::albums_json(&mut tx, None).await?,
        "artists" => db::library::artists_json(&mut tx, None).await?,
        _ => db::library::annotations_json(&mut tx, None).await?,
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
    let artists: Vec<Vec<Value>> = serde_json::from_str(&db::library::artists_json(&mut tx, None).await?)?;
    let track_count = |id: Uuid| {
        let row = artists.iter().find(|r| r[0] == id.to_string()).unwrap();
        row[4].as_i64().unwrap()
    };
    assert_eq!((track_count(lib.both), track_count(lib.x)), (2, 0));
    // The album's tracks: V only, and V's row names the album and Both as primary.
    let albums: Vec<Vec<Value>> = serde_json::from_str(&db::library::albums_json(&mut tx, None).await?)?;
    let track_count = db::library::ALBUM_COLUMNS.iter().position(|c| *c == "track_count").unwrap();
    assert_eq!(albums[0][track_count], 1, "track_count");
    let tracks: Vec<Vec<Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx, None).await?)?;
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

/// What changed for `user` since `version`, as the API would see it.
async fn changes(lib: &Library, user: Uuid, since: String) -> anyhow::Result<db::library::Changes> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    db::library::changes_since(&mut tx, &since).await
}

async fn version(lib: &Library) -> anyhow::Result<String> {
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    Ok(db::library::version(&mut tx).await?)
}

#[sqlx::test]
async fn an_edit_is_in_the_next_delta_and_only_it(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let since = version(&lib).await?;
    assert!(changes(&lib, lib.carol, since.clone()).await?.is_empty(), "nothing yet");
    sqlx::query("UPDATE tracks SET title = 'Song V (edited)' WHERE id = $1").bind(lib.v).execute(&lib.owner).await?;
    let c = changes(&lib, lib.carol, since).await?;
    assert!(!c.everything);
    assert_eq!(c.tracks, vec![lib.v]);
    let later = version(&lib).await?;
    assert!(changes(&lib, lib.carol, later).await?.is_empty(), "nothing after it");

    // The delta's rows: those tracks only, as the full catalog would have them.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let rows: Vec<Vec<Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx, Some(&[lib.v])).await?)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], "Song V (edited)");
    Ok(())
}

#[sqlx::test]
async fn a_merge_is_removed_and_restricting_means_everything(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let since = version(&lib).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    let c = changes(&lib, lib.carol, since).await?;
    assert!(c.tracks.contains(&lib.o) && !c.everything);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let (gone, _, _) = db::library::merged(&mut tx, &c.tracks, &[], &[]).await?;
    assert_eq!(gone, vec![lib.o], "O only redirects now");
    drop(tx);

    // Restricting a track changes who sees what: the whole catalog, not a delta that
    // would name it.
    let since = version(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::access::set(&mut tx, lib.v, true, &[lib.friends], "test").await?;
    tx.commit().await?;
    let c = changes(&lib, lib.carol, since).await?;
    assert!(c.everything && c.tracks.is_empty());
    Ok(())
}

#[sqlx::test]
async fn annotations_are_only_the_users_and_alias_renames_resend_tracks(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // V is credited to Both under an alias.
    let alias: Uuid = sqlx::query_scalar("INSERT INTO artist_aliases (artist_id, alias) VALUES ($1, 'B0th') RETURNING id")
        .bind(lib.both)
        .fetch_one(&lib.owner)
        .await?;
    sqlx::query("UPDATE track_credits SET alias_id = $1 WHERE track_id = $2 AND artist_id = $3")
        .bind(alias)
        .bind(lib.v)
        .bind(lib.both)
        .execute(&lib.owner)
        .await?;

    let since = version(&lib).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::annotations::set_starred(&mut tx, db::annotations::Item::Track(lib.o), true).await?;
    tx.commit().await?;
    assert_eq!(changes(&lib, lib.bob, since.clone()).await?.annotations, vec![lib.o]);
    assert!(changes(&lib, lib.carol, since).await?.is_empty(), "bob's star isn't carol's change");

    let since = version(&lib).await?;
    sqlx::query("UPDATE artist_aliases SET alias = 'B-0th' WHERE id = $1").bind(alias).execute(&lib.owner).await?;
    let c = changes(&lib, lib.carol, since).await?;
    assert!(c.tracks.contains(&lib.v), "V's credit shows the alias: {c:?}");
    assert!(c.artists.contains(&lib.both));
    Ok(())
}

/// A name taken off an artist is hidden (credits still point at it), not deleted: the
/// catalog's aliases, like the artist's page, list only the names it's listed under, and
/// the change comes in the next delta.
#[sqlx::test]
async fn a_name_taken_off_an_artist_leaves_the_catalog(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use crate::db::revisions::Source;
    use crate::model::{Alias, ArtistData};
    let lib = setup(connect).await?;
    let aliases_in_catalog = || async {
        let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
        let rows: Vec<Vec<Value>> = serde_json::from_str(&db::library::artists_json(&mut tx, Some(&[lib.both])).await?)?;
        anyhow::Ok(rows[0][3].clone())
    };
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = ArtistData {
        name: "Both".into(),
        aliases: vec![Alias { id: None, name: "B0th".into() }, Alias { id: None, name: "Bothe".into() }],
        ..Default::default()
    };
    db::revisions::write_artist(&mut tx, lib.both, None, &data, Source::Edit, "").await?;
    tx.commit().await?;
    assert_eq!(aliases_in_catalog().await?, serde_json::json!(["B0th", "Bothe"]));

    let since = version(&lib).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let kept: Vec<Alias> = sqlx::query_as::<_, (Uuid, String)>("SELECT id, alias FROM artist_aliases WHERE artist_id = $1 AND alias = 'B0th'")
        .bind(lib.both)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|(id, name)| Alias { id: Some(id), name })
        .collect();
    db::revisions::write_artist(&mut tx, lib.both, None, &ArtistData { aliases: kept, ..data }, Source::Edit, "").await?;
    tx.commit().await?;
    assert_eq!(aliases_in_catalog().await?, serde_json::json!(["B0th"]), "Bothe is hidden now");
    assert!(changes(&lib, lib.carol, since).await?.artists.contains(&lib.both));
    let hidden: i64 = sqlx::query_scalar("SELECT count(*) FROM artist_aliases WHERE alias = 'Bothe' AND NOT listed").fetch_one(&lib.owner).await?;
    assert_eq!(hidden, 1, "hidden, not deleted");
    Ok(())
}
