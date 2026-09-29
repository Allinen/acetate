//! "Credited as" in bulk (`db::credited_as`). The library: see `rls_tests::setup` (V and
//! H on album A, H restricted to friends; Both credited on all three tracks).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{Library, setup};
use crate::db;
use crate::db::credited_as::{Only, Target};
use crate::db::revisions::Source;
use crate::model::{AlbumData, Credit};

/// Both's aliases as credited on a track or album (`id`).
async fn aliases(lib: &Library, id: Uuid) -> anyhow::Result<Vec<Option<Uuid>>> {
    Ok(sqlx::query_scalar(
        "SELECT alias_id FROM track_credits WHERE track_id = $1 AND artist_id = $2
         UNION ALL SELECT alias_id FROM album_credits WHERE album_id = $1 AND artist_id = $2",
    )
    .bind(id)
    .bind(lib.both)
    .fetch_all(&lib.owner)
    .await?)
}

#[sqlx::test]
async fn an_artist_is_credited_as_one_name_on_tracks_and_their_albums(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let alias = db::revisions::ensure_alias(&mut tx, lib.both, "B0th", Source::Edit, "").await?;
    let credit = Credit { artist_id: lib.both, role: "primary".into(), alias_id: None };
    let album = AlbumData { title: "Album A".into(), credits: vec![credit], ..Default::default() };
    db::revisions::write_album(&mut tx, lib.album, None, &album, Source::Edit, "").await?;
    tx.commit().await?;

    // Carol can't see H: skipped. The album is changed once, for V.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let done = db::credited_as::credit_as(&mut tx, &[lib.v, lib.h, lib.o, lib.v], lib.both, Only::All, Target { artist: lib.both, alias: Some(alias) }, "").await?;
    tx.commit().await?;
    assert_eq!(done.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), [lib.v, lib.o]);
    assert_eq!(done.albums.iter().map(|a| a.id).collect::<Vec<_>>(), [lib.album]);
    assert_eq!((done.already, done.not_credited, done.not_found), (1, 0, 1), "V named twice, H unseen");
    assert_eq!(aliases(&lib, lib.v).await?, [Some(alias)]);
    assert_eq!(aliases(&lib, lib.o).await?, [Some(alias)]);
    assert_eq!(aliases(&lib, lib.h).await?, [None]);
    assert_eq!(aliases(&lib, lib.album).await?, [Some(alias)]);
    let reason: String = sqlx::query_scalar("SELECT reason FROM album_revisions WHERE album_id = $1 ORDER BY id DESC LIMIT 1").bind(lib.album).fetch_one(&lib.owner).await?;
    assert_eq!(reason, "Both credited as \"B0th\"");

    // Back to its own name. X, on H only, isn't credited on O.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let done = db::credited_as::credit_as(&mut tx, &[lib.o], lib.both, Only::All, Target { artist: lib.both, alias: None }, "own name").await?;
    assert_eq!(done.tracks.len(), 1);
    assert_eq!(db::credited_as::credit_as(&mut tx, &[lib.o], lib.x, Only::All, Target { artist: lib.x, alias: None }, "").await?.not_credited, 1);
    tx.commit().await?;
    assert_eq!(aliases(&lib, lib.o).await?, [None]);

    // Another artist's name isn't one of Both's.
    let mut tx = db::begin(&lib.owner, None).await?;
    let x_alias = db::revisions::ensure_alias(&mut tx, lib.x, "Ex", Source::Edit, "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    assert!(db::credited_as::credit_as(&mut tx, &[lib.o], lib.both, Only::All, Target { artist: lib.both, alias: Some(x_alias) }, "").await.is_err());
    Ok(())
}

/// Credits under one name that were really someone else's: moved to them, on the tracks
/// and their albums; the artist's other credits stay.
#[sqlx::test]
async fn credits_under_a_name_move_to_another_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    let alias = db::revisions::ensure_alias(&mut tx, lib.both, "Ex", Source::Edit, "").await?;
    let x_alias = db::revisions::ensure_alias(&mut tx, lib.x, "Ex", Source::Edit, "").await?;
    let credit = Credit { artist_id: lib.both, role: "primary".into(), alias_id: Some(alias) };
    let album = AlbumData { title: "Album A".into(), credits: vec![credit], ..Default::default() };
    db::revisions::write_album(&mut tx, lib.album, None, &album, Source::Edit, "").await?;
    tx.commit().await?;
    // V is credited to Both as "Ex"; O under Both's own name.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::credited_as::credit_as(&mut tx, &[lib.v], lib.both, Only::All, Target { artist: lib.both, alias: Some(alias) }, "").await?;
    let to = Target { artist: lib.x, alias: Some(x_alias) };
    let done = db::credited_as::credit_as(&mut tx, &[lib.v, lib.o], lib.both, Only::Alias(alias), to, "").await?;
    tx.commit().await?;
    assert_eq!(done.tracks.iter().map(|t| t.id).collect::<Vec<_>>(), [lib.v]);
    assert_eq!(done.albums.iter().map(|a| a.id).collect::<Vec<_>>(), [lib.album]);
    assert_eq!(done.not_credited, 1, "O credits Both under its own name");
    let credits = |id: Uuid| {
        sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
            "SELECT artist_id, alias_id FROM track_credits WHERE track_id = $1 UNION ALL SELECT artist_id, alias_id FROM album_credits WHERE album_id = $1",
        )
        .bind(id)
        .fetch_all(&lib.owner)
    };
    assert_eq!(credits(lib.v).await?, [(lib.x, Some(x_alias))]);
    assert_eq!(credits(lib.album).await?, [(lib.x, Some(x_alias))]);
    assert_eq!(credits(lib.o).await?, [(lib.both, None)]);
    let reason: String = sqlx::query_scalar("SELECT reason FROM album_revisions WHERE album_id = $1 ORDER BY id DESC LIMIT 1").bind(lib.album).fetch_one(&lib.owner).await?;
    assert_eq!(reason, "Both (as \"Ex\") credited to X instead, as \"Ex\"");
    // H credits X and Both (Bob sees it): Both's credit goes, X is credited once.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::credited_as::credit_as(&mut tx, &[lib.h], lib.both, Only::OwnName, Target { artist: lib.x, alias: None }, "").await?;
    tx.commit().await?;
    assert_eq!(credits(lib.h).await?, [(lib.x, None)]);
    Ok(())
}
