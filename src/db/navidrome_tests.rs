//! The Navidrome import end to end, against a small Navidrome database made here (same
//! tables and formats as Navidrome's), pointing at files in `test-audio/`.

use std::path::Path;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::rls_tests::setup;
use crate::db;
use crate::db::annotations::{self, Item};
use crate::ingest::describe::Hints;
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource};
use crate::navidrome;
use crate::storage::BlobStore;

fn encrypt(password: &str) -> String {
    let key = Sha256::digest(navidrome::DEFAULT_PASSWORD_KEY.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce = [3u8; 12];
    let mut data = nonce.to_vec();
    data.extend(cipher.encrypt((&nonce).into(), password.as_bytes()).unwrap());
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[sqlx::test]
async fn navidrome_users_and_annotations_move_over(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let audio = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio");

    // In Acetate: Glass Harbor (album Tidelines, artist Mira Vale).
    let origin = sources::validate(Kind::Track, NewSource {
        platform: "custom".into(),
        source_id: None,
        captured_at: None,
        data: serde_json::json!({ "label": "test" }),
    })?;
    let options = IngestOptions {
        user: None,
        uploaded_filename: "01 - Glass Harbor.flac".into(),
        origin,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: Hints::default(),
        local_path: None,
        edited_after: false,
        tags_from: None,
        volume: None,
    };
    let pending = ingest::receive_path(&store, &audio.join("01 - Glass Harbor.flac"), "main").await?;
    let report = ingest::ingest_with_report(&lib.owner, &store, pending, &options, false).await?;
    let track = report.track_id.unwrap();
    let album = report.album.unwrap().id.unwrap();
    let artist = report.artists[0].id.unwrap();

    // Navidrome, with its library mounted at /music (really test-audio/).
    let nd_path = dir.path().join("navidrome.db");
    let nd = SqlitePoolOptions::new()
        .connect_with(SqliteConnectOptions::new().filename(&nd_path).create_if_missing(true))
        .await?;
    for sql in [
        "CREATE TABLE user (id varchar primary key, user_name varchar, password varchar, is_admin bool)",
        "CREATE TABLE library (id integer primary key, name varchar, path varchar)",
        "CREATE TABLE media_file (id varchar primary key, path varchar, album_id varchar, library_id integer, disc_number integer, track_number integer)",
        "CREATE TABLE album (id varchar primary key, name varchar, library_id integer)",
        "CREATE TABLE library_artist (library_id integer, artist_id varchar)",
        "CREATE TABLE artist (id varchar primary key, name varchar)",
        "CREATE TABLE media_file_artists (media_file_id varchar, artist_id varchar)",
        "CREATE TABLE playlist (id varchar primary key, name varchar, comment varchar, public bool, owner_id varchar, rules varchar)",
        "CREATE TABLE playlist_tracks (id integer, playlist_id varchar, media_file_id varchar)",
        // Dora's playlist: Glass Harbor twice, a file not in Acetate, one in the other
        // library. And a smart playlist (rules), which has no fixed tracks.
        "INSERT INTO playlist VALUES ('pl1', 'Favourites', 'the best', 1, 'u1', NULL)",
        "INSERT INTO playlist_tracks VALUES (1, 'pl1', 'mf1'), (2, 'pl1', 'mf2'), (3, 'pl1', 'mfb'), (4, 'pl1', 'mf1')",
        "INSERT INTO playlist VALUES ('pl2', 'Recent', '', 0, 'u1', '{\"all\":[]}')",
        "CREATE TABLE annotation (user_id varchar, item_id varchar, item_type varchar, play_count integer, play_date datetime, rating integer, starred bool, starred_at datetime, rated_at datetime)",
        "INSERT INTO library VALUES (1, 'Music Library', '/music')",
        // Another library, whose files aren't on this machine (like an HDD on the server).
        "INSERT INTO library VALUES (2, 'MusicHD', '/musichd')",
        "INSERT INTO media_file VALUES ('mfb', 'elsewhere.flac', 'alb', 2, 1, 1)",
        "INSERT INTO album VALUES ('alb', 'Elsewhere', 2)",
        "INSERT INTO annotation VALUES ('u1', 'mfb', 'media_file', 99, NULL, 0, 1, NULL, NULL)",
        "INSERT INTO annotation VALUES ('u1', 'alb', 'album', 99, NULL, 0, 0, NULL, NULL)",
        "INSERT INTO library_artist VALUES (1, 'ar1')",
        "INSERT INTO media_file VALUES ('mf1', '01 - Glass Harbor.flac', 'al1', 1, 1, 1)",
        "INSERT INTO media_file VALUES ('mf2', 'not imported.flac', 'al1', 1, 1, 2)",
        "INSERT INTO album VALUES ('al1', 'Tidelines', 1)",
        "INSERT INTO artist VALUES ('ar1', 'mira vale')",
        "INSERT INTO media_file_artists VALUES ('mf1', 'ar1')",
        "INSERT INTO annotation VALUES ('u1', 'mf1', 'media_file', 12, '2026-08-24 12:46:49.070923065+00:00', 4, 1, '2025-10-13 15:36:14.759689571+00:00', NULL)",
        "INSERT INTO annotation VALUES ('u1', 'mf2', 'media_file', 3, NULL, 0, 0, NULL, NULL)",
        "INSERT INTO annotation VALUES ('u1', 'al1', 'album', 20, NULL, 5, 0, NULL, NULL)",
        "INSERT INTO annotation VALUES ('u2', 'ar1', 'artist', 7, NULL, 0, 1, NULL, NULL)",
        "CREATE TABLE scrobbles (media_file_id varchar, user_id varchar, submission_time integer)",
        // Two plays of Glass Harbor, one of a file not in Acetate, one in the other library.
        "INSERT INTO scrobbles VALUES ('mf1', 'u1', 1785015389), ('mf1', 'u1', 1785065930), ('mf2', 'u1', 1785070000), ('mfb', 'u1', 1785080000)",
    ] {
        sqlx::query(sql).execute(&nd).await?;
    }
    sqlx::query("INSERT INTO user VALUES ('u1', 'dora', ?, 1), ('u2', 'carol', ?, 0)")
        .bind(encrypt("dora's password"))
        .bind(encrypt("irrelevant"))
        .execute(&nd)
        .await?;
    nd.close().await;

    // Only the first library, found here at test-audio/.
    let run = |dry_run| navidrome::Options {
        db: nd_path.clone(),
        parts: navidrome::Part::ALL.into(),
        path_maps: Vec::new(),
        libraries: vec![navidrome::parse_library(&format!("1={}", audio.display()))],
        password_key: navidrome::DEFAULT_PASSWORD_KEY.into(),
        dry_run,
    };

    // A dry run changes nothing.
    navidrome::run(&lib.owner, run(true)).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    assert!(db::users::find_user_id(&mut tx, "dora").await?.is_none());
    drop(tx);

    // For real, twice: the second run doesn't count anything twice.
    navidrome::run(&lib.owner, run(false)).await?;
    navidrome::run(&lib.owner, run(false)).await?;

    // Dora is a new admin with her Navidrome password; carol already existed.
    let login = db::users::find_login(&lib.owner, "dora").await?.unwrap();
    assert!(crate::auth::verify_password("dora's password", login.password_hash.as_deref().unwrap()));
    let mut tx = db::begin(&lib.owner, None).await?;
    let dora = db::users::find_user_id(&mut tx, "dora").await?.unwrap();
    let t = annotations::get_for(&mut tx, dora, Item::Track(track)).await?;
    assert_eq!((t.play_count, t.rating, t.starred), (12, 8, true));
    // Postgres keeps microseconds (Navidrome has nanoseconds).
    assert_eq!(t.last_played_at.unwrap().to_rfc3339(), "2026-08-24T12:46:49.070923+00:00");
    let a = annotations::get_for(&mut tx, dora, Item::Album(album)).await?;
    assert_eq!((a.play_count, a.rating), (20, 10));
    let c = annotations::get_for(&mut tx, lib.carol, Item::Artist(artist)).await?;
    assert_eq!((c.play_count, c.starred), (7, true), "matched by name, ignoring case");
    // The other library's annotations weren't touched (nothing of it is here).
    let annotated: i64 = sqlx::query_scalar("SELECT count(*) FROM annotations WHERE user_id = $1")
        .bind(dora)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(annotated, 2, "Glass Harbor and Tidelines only");
    // Her plays of Glass Harbor, with their times, once each.
    let plays: Vec<(uuid::Uuid, chrono::DateTime<chrono::Utc>, String)> =
        sqlx::query_as("SELECT track_id, played_at, source FROM plays WHERE user_id = $1 ORDER BY played_at").bind(dora).fetch_all(&lib.owner).await?;
    assert_eq!(plays.iter().map(|p| (p.0, p.1.timestamp(), p.2.as_str())).collect::<Vec<_>>(), [(track, 1785015389, "navidrome"), (track, 1785065930, "navidrome")]);

    // Her playlist, once (the second run left it alone): Glass Harbor twice, in order,
    // public as it was. The smart one isn't there.
    let playlists: Vec<(uuid::Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT id, name, description, public FROM playlists WHERE owner_id = $1",
    )
    .bind(dora)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(playlists.len(), 1);
    let (playlist, name, description, public) = &playlists[0];
    assert_eq!((name.as_str(), description.as_str(), *public), ("Favourites", "the best", true));
    let tracks: Vec<uuid::Uuid> = sqlx::query_scalar("SELECT track_id FROM playlist_entries WHERE playlist_id = $1 ORDER BY position")
        .bind(playlist)
        .fetch_all(&lib.owner)
        .await?;
    assert_eq!(tracks, [track, track]);
    Ok(())
}
