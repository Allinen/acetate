//! Notes (`db::notes`, migration 0039). The library: see `rls_tests::setup` (V and H on
//! album A, H restricted to friends; O on no album; artists Both and X).

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::notes::Owner;
use crate::db::revisions::EditRejected;

#[sqlx::test]
async fn notes_are_written_edited_taken_off_and_kept(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let (note, first) = db::notes::create(&mut tx, Owner::Track(lib.v), "  Recording ", "\n\nTracked in one take.\n  Mixed later.  \n\n", "").await?;
    assert!(db::notes::create(&mut tx, Owner::Album(lib.album), " ", "\n ", "").await.is_err(), "empty");
    db::notes::create(&mut tx, Owner::Artist(lib.both), "Bio", "From the coast.", "from bio.txt").await?;

    let notes = db::notes::list(&mut tx, Owner::Track(lib.v)).await?;
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].title.as_str(), notes[0].body.as_str()), ("Recording", "Tracked in one take.\n  Mixed later."));
    assert_eq!(notes[0].revision_id, first);
    assert_eq!(notes[0].created_by.as_deref(), Some("carol"));

    // Edited from the first revision; again from it: stale.
    let (second, changed) = db::notes::edit(&mut tx, note, Some(first), "Recording", "Tracked in two takes.", "").await?;
    assert!(changed);
    let stale = db::notes::edit(&mut tx, note, Some(first), "Recording", "Something else.", "").await.unwrap_err();
    assert!(matches!(stale.downcast_ref::<EditRejected>(), Some(EditRejected::Stale { latest: Some(l) }) if *l == second));
    assert!(!db::notes::edit(&mut tx, note, Some(second), "Recording", "Tracked in two takes.", "").await?.1, "the same: nothing");

    // Taken off, still there (flagged), and brought back.
    assert!(db::notes::set_hidden(&mut tx, note, true, "wrong track").await?);
    assert!(!db::notes::set_hidden(&mut tx, note, true, "").await?);
    assert!(db::notes::list(&mut tx, Owner::Track(lib.v)).await?[0].hidden);
    assert!(db::notes::set_hidden(&mut tx, note, false, "").await?);
    let revisions = db::notes::revisions(&mut tx, note).await?;
    assert_eq!(revisions.iter().map(|r| r.hidden).collect::<Vec<_>>(), [false, true, false, false]);
    assert_eq!(revisions[1].reason, "wrong track");
    tx.commit().await?;

    // Not deleted, nor rewritten, even by hand.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("DELETE FROM notes").execute(&mut *tx).await.is_err());
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("UPDATE note_revisions SET body = ''").execute(&mut *tx).await.is_err());
    Ok(())
}

#[sqlx::test]
async fn notes_are_seen_as_far_as_their_track_is_and_follow_merges(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // Bob (friends) writes on H; carol can't see H, nor its note.
    let mut tx = db::begin(&lib.app, Some(lib.bob)).await?;
    let (on_h, _) = db::notes::create(&mut tx, Owner::Track(lib.h), "", "restricted story", "").await?;
    db::notes::create(&mut tx, Owner::Track(lib.o), "O's", "about O", "").await?;
    tx.commit().await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::notes::list(&mut tx, Owner::Track(lib.h)).await?.is_empty());
    assert!(db::notes::edit(&mut tx, on_h, None, "", "mine now", "").await.is_err());
    drop(tx);

    // O merged into V: V shows O's note too (saying where it came from); O takes no new ones.
    sqlx::query("UPDATE tracks SET merged_into = $1 WHERE id = $2").bind(lib.v).bind(lib.o).execute(&lib.owner).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let on_v = db::notes::list(&mut tx, Owner::Track(lib.v)).await?;
    assert_eq!(on_v.iter().map(|n| (n.title.as_str(), n.merged_from)).collect::<Vec<_>>(), [("O's", Some(lib.o))]);
    assert!(db::notes::create(&mut tx, Owner::Track(lib.o), "", "late", "").await.is_err());
    Ok(())
}
