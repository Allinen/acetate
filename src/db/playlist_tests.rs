//! Playlists: who sees them, hidden tracks in them, duplicates, the log and its undo,
//! the trash, and merges moving entries along.
//!
//! The library (see `rls_tests::setup`): tracks V and O everyone can see; H only bob
//! (group "friends") and alice (admin). Carol can't see H.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{setup, Library};
use crate::db;

/// The tracks of a playlist's entries, in order, as `user` sees them.
async fn tracks_seen(lib: &Library, user: Uuid, playlist: Uuid) -> anyhow::Result<Option<Vec<Uuid>>> {
    let mut tx = db::begin(&lib.app, Some(user)).await?;
    Ok(db::playlists::get(&mut tx, playlist).await?.map(|p| p.entries.iter().map(|e| e.track_id).collect()))
}

/// All entries' tracks in order, hidden ones too.
async fn all_tracks(lib: &Library, playlist: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT track_id FROM playlist_entries WHERE playlist_id = $1 ORDER BY position")
        .bind(playlist)
        .fetch_all(&lib.owner)
        .await?)
}

async fn bobs_playlist(lib: &Library, public: bool, tracks: &[Uuid]) -> anyhow::Result<Uuid> {
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let id = db::playlists::create(&mut tx, "Bob's", "", public).await?;
    db::playlists::add(&mut tx, id, tracks, None).await?;
    tx.commit().await?;
    Ok(id)
}

#[sqlx::test]
async fn private_is_the_owners_and_public_is_read_only_for_others(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let id = bobs_playlist(&lib, false, &[lib.v, lib.o]).await?;

    // Private: carol doesn't see it at all.
    assert_eq!(tracks_seen(&lib, lib.carol, id).await?, None);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::playlists::list(&mut tx, false).await?.is_empty());
    drop(tx);

    // Public: she sees it, but can't change it.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::playlists::update(&mut tx, id, None, None, Some(true)).await?;
    tx.commit().await?;
    assert_eq!(tracks_seen(&lib, lib.carol, id).await?, Some(vec![lib.v, lib.o]));
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let listed = db::playlists::list(&mut tx, false).await?;
    assert_eq!((listed.len(), listed[0].mine), (1, false));
    let err = db::playlists::add(&mut tx, id, &[lib.v], None).await.unwrap_err();
    assert!(err.to_string().contains("only its owner"), "{err}");
    let err = db::playlists::update(&mut tx, id, Some("Carol's now"), None, None).await.unwrap_err();
    assert!(err.to_string().contains("only its owner"), "{err}");

    // Guests (no user) see none, public or not.
    let mut tx = db::begin(&lib.app, None).await?;
    assert!(db::playlists::get(&mut tx, id).await?.is_none());
    Ok(())
}

#[sqlx::test]
async fn hidden_tracks_are_left_out_and_keep_their_place(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let id = bobs_playlist(&lib, true, &[lib.v, lib.h, lib.o]).await?;

    // Carol can't see H: not in the entries, not in the count.
    assert_eq!(tracks_seen(&lib, lib.carol, id).await?, Some(vec![lib.v, lib.o]));
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert_eq!(db::playlists::list(&mut tx, false).await?[0].entries, 2);
    // Nor can she add it to a playlist of her own.
    let hers = db::playlists::create(&mut tx, "Carol's", "", false).await?;
    assert!(db::playlists::add(&mut tx, hers, &[lib.h], None).await.is_err());
    drop(tx);

    // H becomes hidden from bob too (admins only). His reorder of what he sees keeps
    // H in the middle.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::access::set(&mut tx, lib.h, true, &[], "test").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let entries = db::playlists::get(&mut tx, id).await?.unwrap().entries;
    assert_eq!(entries.len(), 2);
    let (v_entry, o_entry) = (entries[0].id, entries[1].id);
    db::playlists::reorder(&mut tx, id, &[o_entry, v_entry]).await?;
    // Not every entry he sees: refused.
    assert!(db::playlists::reorder(&mut tx, id, &[o_entry]).await.is_err());
    tx.commit().await?;
    assert_eq!(all_tracks(&lib, id).await?, [lib.o, lib.h, lib.v]);
    Ok(())
}

#[sqlx::test]
async fn the_same_track_twice_removing_and_undoing(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let id = bobs_playlist(&lib, false, &[lib.v, lib.o, lib.v]).await?;
    assert_eq!(all_tracks(&lib, id).await?, [lib.v, lib.o, lib.v]);

    // Add at the start ("before the 0th entry").
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::playlists::add(&mut tx, id, &[lib.o], Some(0)).await?;
    assert_eq!(tracks_seen_in(&mut tx, id).await?, [lib.o, lib.v, lib.o, lib.v]);

    // Remove the middle O: logged, and undoing puts it back where it was, same entry.
    let entries = db::playlists::get(&mut tx, id).await?.unwrap().entries;
    let middle = entries[2].id;
    db::playlists::remove(&mut tx, id, &[middle]).await?;
    assert_eq!(tracks_seen_in(&mut tx, id).await?, [lib.o, lib.v, lib.v]);
    let log = db::playlists::history(&mut tx, id).await?;
    assert_eq!(log[0].action, "remove");
    let summary = db::playlists::undo(&mut tx, id, log[0].id).await?;
    assert_eq!(summary, "put back the 1 entry that change removed");
    let entries = db::playlists::get(&mut tx, id).await?.unwrap().entries;
    assert_eq!(entries.iter().map(|e| e.track_id).collect::<Vec<_>>(), [lib.o, lib.v, lib.o, lib.v]);
    assert_eq!(entries[2].id, middle);
    // Not twice.
    assert!(db::playlists::undo(&mut tx, id, log[0].id).await.is_err());

    // Undoing the add at the start takes that O out again.
    let add = db::playlists::history(&mut tx, id).await?.into_iter().find(|l| l.action == "add" && l.undone_at.is_none()).unwrap();
    // (the newest add is the one at the start)
    db::playlists::undo(&mut tx, id, add.id).await?;
    assert_eq!(tracks_seen_in(&mut tx, id).await?, [lib.v, lib.o, lib.v]);
    tx.commit().await?;
    Ok(())
}

async fn tracks_seen_in(conn: &mut sqlx::PgConnection, id: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(db::playlists::get(conn, id).await?.unwrap().entries.iter().map(|e| e.track_id).collect())
}

#[sqlx::test]
async fn trash_and_restore(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let id = bobs_playlist(&lib, true, &[lib.v]).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::playlists::set_trashed(&mut tx, id, true).await?;
    assert!(db::playlists::list(&mut tx, false).await?.is_empty());
    assert_eq!(db::playlists::list(&mut tx, true).await?.len(), 1);
    tx.commit().await?;
    // A public playlist in the trash is gone for others.
    assert_eq!(tracks_seen(&lib, lib.carol, id).await?, None);
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::playlists::set_trashed(&mut tx, id, false).await?;
    assert_eq!(db::playlists::list(&mut tx, false).await?.len(), 1);
    Ok(())
}

#[sqlx::test]
async fn entries_follow_a_merge_and_its_undo(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Bob's private playlist has O; alice merges O into V.
    let id = bobs_playlist(&lib, false, &[lib.o, lib.v]).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let merged = db::revisions::merge_tracks(&mut tx, lib.v, lib.o, &Default::default(), None, None, "").await?;
    tx.commit().await?;
    assert_eq!(all_tracks(&lib, id).await?, [lib.v, lib.v], "O's entry plays V now");

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    db::revisions::undo_track_merge(&mut tx, merged.merge_id, "").await?;
    tx.commit().await?;
    assert_eq!(all_tracks(&lib, id).await?, [lib.o, lib.v], "and back");
    Ok(())
}

#[sqlx::test]
async fn an_import_is_brought_up_to_date_until_someone_changes_it(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    use db::playlists::Imported;
    let lib = setup(connect).await?;
    let import = |tracks: Vec<Uuid>| {
        let lib = &lib;
        async move {
            let mut tx = db::begin(&lib.owner, None).await?;
            let result = db::playlists::import(&mut tx, lib.bob, "navidrome:pl1", "Favourites", "", false, &tracks).await?;
            tx.commit().await?;
            anyhow::Ok(result)
        }
    };
    // A first run matched nothing (wrong paths, say); the next one does.
    let (id, first) = import(vec![]).await?;
    assert_eq!(first, Imported::Made);
    assert_eq!(import(vec![lib.v, lib.o]).await?, (id, Imported::Refreshed));
    assert_eq!(all_tracks(&lib, id).await?, [lib.v, lib.o]);

    // Bob changes it: from then on, imports leave it alone.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    db::playlists::add(&mut tx, id, &[lib.v], None).await?;
    tx.commit().await?;
    assert_eq!(import(vec![lib.o]).await?, (id, Imported::Kept));
    assert_eq!(all_tracks(&lib, id).await?, [lib.v, lib.o, lib.v]);
    Ok(())
}

#[sqlx::test]
async fn a_playlist_has_one_entries_key(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    // The detail once had the summary's count and the list under the same key: a
    // client keeping the first (serde) saw a number, not the entries.
    let lib = setup(connect).await?;
    let id = bobs_playlist(&lib, false, &[lib.v, lib.o]).await?;
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let json = serde_json::to_string(&db::playlists::get(&mut tx, id).await?.unwrap())?;
    assert_eq!(json.matches("\"entries\":").count(), 1, "{json}");
    assert!(json.contains("\"entries\":[{"), "{json}");
    Ok(())
}
