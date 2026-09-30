//! Library tooling, end to end (same setup as `rls_tests`): the history's alias names,
//! what's stored per volume, and the one-off commands (`own-album-covers`,
//! `measure-covers`, `producers-from-composer`), each read-only unless applied.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::revisions::Source;
use crate::storage::BlobStore;

#[sqlx::test]
async fn history_names_aliases_not_listed_anymore(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    // V credits Both as "B0th", then the alias is taken off the list (hidden, kept).
    let mut tx = db::begin(&lib.owner, None).await?;
    let alias = db::revisions::ensure_alias(&mut tx, lib.both, "B0th", Source::Edit, "").await?;
    let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(lib.v).fetch_one(&mut *tx).await?;
    let (_, mut meta) = db::revisions::latest_internal(&mut tx, lib.v, variant).await?.unwrap();
    meta.credits[0].alias_id = Some(alias);
    db::revisions::write_internal(&mut tx, lib.v, variant, None, &meta, Source::Edit, "", false).await?;
    sqlx::query("UPDATE artist_aliases SET listed = false WHERE id = $1").bind(alias).execute(&mut *tx).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let track = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert_eq!(track.alias_names.unwrap().get(&alias).map(String::as_str), Some("B0th"));
    let without_history = db::tracks::get_track(&mut tx, lib.v, false).await?.unwrap();
    assert!(without_history.alias_names.is_none());
    Ok(())
}

#[sqlx::test]
async fn totals_say_whats_stored_per_volume(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let cover = store.put_bytes(b"an image").await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::covers::add(&mut tx, CoverOwner::Track(lib.v), &NewCover { blob_sha256: &cover, mime: None, width: None, height: None, source: "upload" }).await?;
    tx.commit().await?;
    let v_sha: String = sqlx::query_scalar("SELECT f.sha256 FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1").bind(lib.v).fetch_one(&lib.owner).await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::blobs::register(&mut tx, &v_sha, "main", Some(1000)).await?;
    db::blobs::register(&mut tx, &cover, "main", None).await?;
    db::blobs::register(&mut tx, &"f".repeat(64), "archive", Some(5)).await?;
    tx.commit().await?;

    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let totals: Vec<(String, String, i64, i64, i64)> =
        db::blobs::totals(&mut tx).await?.into_iter().map(|t| (t.volume, t.kind, t.blobs, t.bytes, t.unmeasured)).collect();
    assert_eq!(
        totals,
        [
            ("archive".into(), "other".into(), 1, 5, 0),
            ("main".into(), "audio".into(), 1, 1000, 0),
            ("main".into(), "covers".into(), 1, 0, 1),
        ]
    );
    Ok(())
}

#[sqlx::test]
async fn albums_get_a_copy_of_the_cover_they_show(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let sha = BlobStore::open(dir.path())?.put_bytes(b"V's cover").await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::covers::add(&mut tx, CoverOwner::Track(lib.v), &NewCover { blob_sha256: &sha, mime: None, width: None, height: None, source: "upload" }).await?;
    tx.commit().await?;
    let own = || async {
        let covers: Vec<String> = sqlx::query_scalar("SELECT blob_sha256 FROM covers WHERE album_id = $1 AND trashed_at IS NULL").bind(lib.album).fetch_all(&lib.owner).await?;
        anyhow::Ok(covers)
    };

    crate::album_covers_command::run(&lib.owner, false).await?;
    assert!(own().await?.is_empty(), "a look changes nothing");
    crate::album_covers_command::run(&lib.owner, true).await?;
    assert_eq!(own().await?, [sha.clone()], "album A has the image it showed, as its own");
    crate::album_covers_command::run(&lib.owner, true).await?;
    assert_eq!(own().await?.len(), 1, "once");
    Ok(())
}

#[sqlx::test]
async fn covers_and_blobs_are_measured(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let mut png = Vec::new();
    image::RgbImage::new(2, 3).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let sha = store.put_bytes(&png).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    // Two covers of one image (a track's and its album's), neither measured.
    for owner in [CoverOwner::Track(lib.v), CoverOwner::Album(lib.album)] {
        db::covers::add(&mut tx, owner, &NewCover { blob_sha256: &sha, mime: None, width: None, height: None, source: "embedded" }).await?;
    }
    tx.commit().await?;
    let mut tx = db::begin(&lib.owner, None).await?;
    db::blobs::register(&mut tx, &sha, "main", None).await?;
    tx.commit().await?;
    let measured = || async {
        let sizes: Vec<(Option<i32>, Option<i32>)> = sqlx::query_as("SELECT width, height FROM covers WHERE blob_sha256 = $1").bind(&sha).fetch_all(&lib.owner).await?;
        let size: Option<i64> = sqlx::query_scalar("SELECT size_bytes FROM blobs WHERE sha256 = $1").bind(&sha).fetch_one(&lib.owner).await?;
        anyhow::Ok((sizes, size))
    };

    crate::measure_command::run(&lib.owner, &store, false).await?;
    assert_eq!(measured().await?, (vec![(None, None), (None, None)], None), "a look changes nothing");
    crate::measure_command::run(&lib.owner, &store, true).await?;
    assert_eq!(measured().await?, (vec![(Some(2), Some(3)), (Some(2), Some(3))], Some(png.len() as i64)));
    Ok(())
}

#[sqlx::test]
async fn composers_become_producers_later(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // A file whose COMPOSER names two producers (made on the spot: needs ffmpeg).
    let file = dir.path().join("composed.flac");
    let made = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-f", "lavfi", "-i", "sine=duration=1", "-metadata", "title=Composed", "-metadata", "artist=Singer"])
        .args(["-metadata", "composer=Beat Person; Other Producer"])
        .arg(&file)
        .status();
    if !made.is_ok_and(|s| s.success()) {
        return Ok(()); // no ffmpeg here
    }
    // Imported leaving COMPOSER out, as before ingest read it.
    let pending = crate::ingest::receive_path(&store, &file, "main").await?;
    let custom = crate::model::sources::validate(
        crate::model::sources::Kind::Track,
        crate::model::sources::NewSource { platform: "custom".into(), source_id: None, captured_at: None, data: serde_json::json!({ "label": "test" }) },
    )?;
    let options = crate::ingest::IngestOptions {
        user: None,
        uploaded_filename: "composed.flac".into(),
        origin: custom,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: crate::ingest::describe::Hints { no_composers: true, ..Default::default() },
        local_path: None,
        given: Default::default(),
        tags_from: None,
        volume: None,
    };
    let track = crate::ingest::ingest_with_report(&lib.owner, &store, pending, &options, false).await?.track_id.unwrap();
    let producers = || async {
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT a.name FROM track_credits c JOIN artists a ON a.id = c.artist_id WHERE c.track_id = $1 AND c.role = 'producer' ORDER BY c.position",
        )
        .bind(track)
        .fetch_all(&lib.owner)
        .await?;
        anyhow::Ok(names)
    };
    let tomorrow = chrono::Utc::now().date_naive() + chrono::Days::new(1);

    crate::producers_command::run(&lib.owner, &store, tomorrow, false).await?;
    assert!(producers().await?.is_empty(), "a look changes nothing");
    crate::producers_command::run(&lib.owner, &store, chrono::Utc::now().date_naive(), true).await?;
    assert!(producers().await?.is_empty(), "imported after `before`: left alone");
    crate::producers_command::run(&lib.owner, &store, tomorrow, true).await?;
    assert_eq!(producers().await?, ["Beat Person", "Other Producer"]);
    // A track that has had a producer credit is never touched again.
    crate::producers_command::run(&lib.owner, &store, tomorrow, true).await?;
    assert_eq!(producers().await?.len(), 2);
    Ok(())
}
