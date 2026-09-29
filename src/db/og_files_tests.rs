//! Original files corrected by hand (`db::og_files`, migration 0038). The library: see
//! `rls_tests::setup` (V, H and O, their files uploaded as "Song V", "Song H", "Other";
//! H restricted to friends). Their files are made plain files here first, as if their
//! tags hadn't said.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::{setup, Library};
use crate::db;

async fn file_of(lib: &Library, track: Uuid) -> anyhow::Result<(Uuid, Option<String>)> {
    Ok(sqlx::query_as("SELECT f.id, f.original_filename FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?)
}

#[sqlx::test]
async fn tracks_are_marked_as_original_files_under_their_uploaded_names(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    sqlx::query("UPDATE files SET original_filename = NULL").execute(&lib.owner).await?;
    let version = {
        let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
        db::library::version(&mut tx).await?
    };
    // Carol doesn't see H: left out.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let marked = db::og_files::mark_original(&mut tx, &[lib.v, lib.h, lib.o], "").await?;
    tx.commit().await?;
    assert_eq!(marked.tracks.iter().map(|t| (t.track_id, t.name.as_str())).collect::<Vec<_>>(), [(lib.v, "Song V"), (lib.o, "Other")]);
    assert_eq!((marked.already, marked.not_found), (0, 1));
    assert_eq!(file_of(&lib, lib.v).await?.1.as_deref(), Some("Song V"));
    assert_eq!(file_of(&lib, lib.h).await?.1, None);

    // The catalog says so, and the change comes with the next delta.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let changes = db::library::changes_since(&mut tx, &version).await?;
    assert!(changes.tracks.contains(&lib.v) && changes.tracks.contains(&lib.o));
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&db::library::tracks_json(&mut tx, Some(&[lib.v])).await?)?;
    let at = db::library::TRACK_COLUMNS.iter().position(|c| *c == "original_filename").unwrap();
    assert_eq!(rows[0][at], "Song V");

    // Again: they're original files already.
    let again = db::og_files::mark_original(&mut tx, &[lib.v], "").await?;
    assert_eq!((again.tracks.len(), again.already), (0, 1));

    // One file by hand: another name, then not an original file after all. Each kept.
    let (file, _) = file_of(&lib, lib.v).await?;
    assert!(db::og_files::set(&mut tx, file, Some(" song v MASTER.wav "), "the real name").await?);
    assert!(!db::og_files::set(&mut tx, file, Some("song v MASTER.wav"), "").await?, "the same: nothing");
    assert!(db::og_files::set(&mut tx, file, None, "a rip after all").await?);
    assert!(db::og_files::set(&mut tx, file, Some(""), "").await.is_err());
    let changes = db::og_files::changes(&mut tx, file).await?;
    let names: Vec<(Option<&str>, Option<&str>)> = changes.iter().map(|c| (c.old_name.as_deref(), c.new_name.as_deref())).collect();
    assert_eq!(names, [(Some("song v MASTER.wav"), None), (Some("Song V"), Some("song v MASTER.wav")), (None, Some("Song V"))]);
    assert_eq!(changes[2].reason, "marked as an original file");
    assert_eq!(changes[0].changed_by.as_deref(), Some("carol"));
    tx.commit().await?;

    // The file's own record can't be rewritten otherwise, nor its history.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("UPDATE files SET uploaded_filename = 'x' WHERE id = $1").bind(file).execute(&mut *tx).await.is_err());
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(sqlx::query("DELETE FROM file_original_filename_changes").execute(&mut *tx).await.is_err());
    Ok(())
}
