//! Automated ingestion (`ingest::describe`): platform ids carried over and used for
//! matching, album grouping, same-song flags, re-runs and dry runs. Uses the small files in
//! `test-audio/` (same setup as `rls_tests`).
//!
//! The two files: "01 - Glass Harbor.flac" (ARTIST Mira Vale, ALBUM Tidelines, ALBUM
//! ARTIST Mira Vale, date 2021-06-04T11:11:11Z) and "02 - Undertow.flac" (ARTIST "Mira Vale
//! feat. Oskar Lind", same album and album artist, date 2021-06-04).

use std::path::Path;

use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use super::rls_tests::setup;
use crate::db;
use crate::db::revisions::EditRejected;
use crate::ingest::describe::{AlbumHint, ArtistHint, Hints, MatchedBy, Report, ReportStatus};
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource, ValidSource};
use crate::storage::BlobStore;

fn netease(kind: Kind, field: &str, id: i64) -> ValidSource {
    let source = NewSource {
        platform: "netease".into(),
        source_id: None,
        captured_at: None,
        data: json!({ field: id }),
    };
    sources::validate(kind, source).unwrap()
}

/// Options for a file from NetEase song `song_id`, with these hints.
fn options(user: Option<Uuid>, song_id: i64, hints: Hints) -> IngestOptions {
    IngestOptions {
        user,
        uploaded_filename: String::new(),
        origin: netease(Kind::Track, "song_id", song_id),
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints,
        local_path: None,
        given: Default::default(),
        volume: None,
    }
}

/// "Mira Vale is NetEase artist `artist_id`, the album is NetEase album `album_id`".
fn mira_vale(artist_id: i64, album_id: Option<i64>) -> Hints {
    Hints {
        artists: vec![ArtistHint {
            name: "Mira Vale".into(),
            id: None,
            sources: vec![netease(Kind::Artist, "artist_id", artist_id)],
        }],
        album: album_id.map(|id| AlbumHint {
            sources: vec![netease(Kind::Album, "album_id", id)],
            ..Default::default()
        }),
    }
}

async fn ingest_file(
    pool: &PgPool,
    store: &BlobStore,
    file: &str,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<Report> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file);
    let pending = ingest::receive_path(store, &path, "main").await?;
    let options = IngestOptions {
        uploaded_filename: file.to_string(),
        ..options.clone()
    };
    ingest::ingest_with_report(pool, store, pending, &options, dry_run).await
}

fn artist<'a>(report: &'a Report, name: &str) -> &'a crate::ingest::describe::Resolved {
    report
        .artists
        .iter()
        .find(|a| a.name == name)
        .unwrap_or_else(|| panic!("no {name} in {:?}", report.artists))
}

async fn count(pool: &PgPool, sql: &'static str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await?)
}

#[sqlx::test]
async fn platform_ids_are_kept_and_matched_first(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, mira_vale(10, Some(20))), false).await?;
    assert_eq!(first.status, ReportStatus::Created);
    assert_eq!(artist(&first, "Mira Vale").matched_by, MatchedBy::Created);
    let album = first.album.clone().unwrap();
    assert_eq!(album.matched_by, MatchedBy::Created);

    // The second file's album has another date, so by the tags it'd be another album, but
    // the NetEase album id says it's the same one. Its track artist is "Mira Vale feat.
    // Oskar Lind" (one name: only `;` is split), a new artist; the album artist is Mira
    // Vale, found by her NetEase id.
    let second = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, mira_vale(10, Some(20))), false).await?;
    assert_eq!(artist(&second, "Mira Vale").matched_by, MatchedBy::PlatformId);
    assert_eq!(artist(&second, "Mira Vale").id, artist(&first, "Mira Vale").id);
    assert_eq!(artist(&second, "Mira Vale feat. Oskar Lind").matched_by, MatchedBy::Created);
    let second_album = second.album.clone().unwrap();
    assert_eq!(second_album.matched_by, MatchedBy::PlatformId);
    assert_eq!(second_album.id, album.id);
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);

    // The ids are sources of the artist, the album and the tracks, once each.
    let mira = artist(&first, "Mira Vale").id.unwrap();
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT platform || ':' || external_id FROM sources WHERE artist_id = $1 OR album_id = $2 ORDER BY 1",
    )
    .bind(mira)
    .bind(album.id)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(ids, ["netease:10", "netease:20"]);
    assert_eq!(count(&lib.owner, "SELECT count(*) FROM sources WHERE platform = 'netease' AND track_id IS NOT NULL").await?, 2);
    Ok(())
}

#[sqlx::test]
async fn same_name_with_another_platform_id_is_another_artist(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, mira_vale(10, None)), false).await?;
    let second = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, mira_vale(11, None)), false).await?;
    let other = artist(&second, "Mira Vale");
    assert_eq!(other.matched_by, MatchedBy::Created);
    assert_ne!(other.id, artist(&first, "Mira Vale").id);
    assert!(second.warnings.iter().any(|w| w.contains("different netease id")), "{:?}", second.warnings);

    Ok(())
}

#[sqlx::test]
async fn albums_need_the_same_artists_and_date(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Same title and album artist, but the dates differ ("…T11:11:11Z" vs "2021-06-04"):
    // two albums. Merging fixes that if they're really one.
    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, Hints::default()), false).await?;
    let second = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, Hints::default()), false).await?;
    assert_ne!(first.album.unwrap().id, second.album.unwrap().id);

    let mut tx = db::begin(&lib.owner, None).await?;
    let a = db::ingest::find_or_create_artist(&mut tx, "A").await?;
    let b = db::ingest::find_or_create_artist(&mut tx, "B").await?;
    let album = db::ingest::create_album(&mut tx, "Same Title", &[a.clone(), b.clone()], true, Some("2020")).await?;
    let find = async |tx: &mut sqlx::PgConnection, artists: &[Uuid], date: Option<&str>| {
        db::ingest::find_album(tx, "same title", artists, date).await.unwrap()
    };
    assert_eq!(find(&mut tx, &[b.id, a.id], Some("2020")).await, Some(album), "any order, any case");
    assert_eq!(find(&mut tx, &[a.id], Some("2020")).await, None, "an artist missing");
    assert_eq!(find(&mut tx, &[a.id, b.id, lib.x], Some("2020")).await, None, "an artist too many");
    assert_eq!(find(&mut tx, &[a.id, b.id], None).await, None, "no date");
    assert_eq!(find(&mut tx, &[a.id, b.id], Some("2020-01-01")).await, None, "another date");
    Ok(())
}

#[sqlx::test]
async fn same_song_id_in_another_file_is_flagged(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 7, Hints::default()), false).await?;
    // Another file claiming to be the same NetEase song: a new track, flagged.
    let second = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 7, Hints::default()), false).await?;
    assert_eq!(second.status, ReportStatus::Created);
    assert_eq!(second.same_song_as, vec![first.track_id.unwrap()]);

    let related: serde_json::Value = sqlx::query_scalar(
        "SELECT related FROM review_flags WHERE track_id = $1 AND kind = 'possible_duplicate'",
    )
    .bind(second.track_id)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(related["reason"], "same netease song id 7");
    assert_eq!(related["tracks"], json!([first.track_id]));
    Ok(())
}

#[sqlx::test]
async fn reruns_and_dry_runs_change_nothing(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let everything = "SELECT (SELECT count(*) FROM tracks) + (SELECT count(*) FROM artists) \
                      + (SELECT count(*) FROM albums) + (SELECT count(*) FROM source_snapshots) \
                      + (SELECT count(*) FROM covers) + (SELECT count(*) FROM review_flags)";
    let opts = options(None, 1, mira_vale(10, Some(20)));

    let before = count(&lib.owner, everything).await?;
    let dry = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, true).await?;
    assert_eq!(dry.status, ReportStatus::Created);
    assert_eq!(count(&lib.owner, everything).await?, before, "a dry run changes nothing");
    let blobs = std::fs::read_dir(dir.path().join("blobs"))?.count();
    assert_eq!(blobs, 0, "and stores nothing");

    let real = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, false).await?;
    let after = count(&lib.owner, everything).await?;
    // The same file again (also under another name): a duplicate, and nothing new.
    for file in ["01 - Glass Harbor.flac", "glass harbor (copy).flac"] {
        let again = ingest_file(&lib.owner, &store, file, &opts, false).await?;
        assert_eq!(again.status, ReportStatus::Duplicate);
        assert_eq!(again.track_id, real.track_id);
    }
    assert_eq!(count(&lib.owner, everything).await?, after);
    Ok(())
}

#[sqlx::test]
async fn users_only_get_what_they_can_see(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // X is only credited on a track carol can't see: naming it by id is "not found".
    let hints = Hints {
        artists: vec![ArtistHint {
            name: "Mira Vale".into(),
            id: Some(lib.x),
            sources: Vec::new(),
        }],
        album: None,
    };
    let err = ingest_file(&lib.app, &store, "01 - Glass Harbor.flac", &options(Some(lib.carol), 1, hints), false)
        .await
        .unwrap_err();
    assert!(matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::NotFound("artist"))), "{err:#}");

    // A NetEase artist id that X has: carol's file is matched to X (the id says so), and X
    // becomes visible to her through her own track. With the track restricted away from
    // her, she'd get no id back.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::sources::add_snapshot(&mut tx, crate::model::sources::SourceOwner::Artist(lib.x), &netease(Kind::Artist, "artist_id", 99)).await?;
    tx.commit().await?;
    let mut opts = options(Some(lib.carol), 1, mira_vale(99, None));
    opts.restrict_to = Some(vec![lib.friends]);
    let report = ingest_file(&lib.app, &store, "01 - Glass Harbor.flac", &opts, false).await?;
    let found = artist(&report, "Mira Vale");
    assert_eq!(found.matched_by, MatchedBy::PlatformId);
    assert_eq!(found.id, None, "X stays hidden from carol");
    Ok(())
}

#[sqlx::test]
async fn original_filename_comes_only_from_the_tags(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Glass Harbor has ORIGINALFILENAME=glass_harbor_MASTER_v7.wav; Undertow has none, so
    // it has no original filename (not the name it was uploaded with).
    for file in ["01 - Glass Harbor.flac", "02 - Undertow.flac"] {
        ingest_file(&lib.owner, &store, file, &options(None, 1, Hints::default()), false).await?;
    }
    let names: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT uploaded_filename, original_filename FROM files ORDER BY uploaded_filename",
    )
    .fetch_all(&lib.owner)
    .await?;
    let find = |uploaded: &str| names.iter().find(|(u, _)| u == uploaded).unwrap().1.clone();
    assert_eq!(find("01 - Glass Harbor.flac").as_deref(), Some("glass_harbor_MASTER_v7.wav"));
    assert_eq!(find("02 - Undertow.flac"), None);
    Ok(())
}

async fn problems_seen_by(pool: &PgPool, user: Uuid) -> anyhow::Result<Vec<db::problems::Problem>> {
    let mut tx = db::begin(pool, Some(user)).await?;
    Ok(db::problems::list(&mut tx, false, None, 100).await?)
}

#[sqlx::test]
async fn files_that_dont_import_are_listed_with_their_audio(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // "no-artist.opus" has no ARTIST tag. A dry run doesn't list it...
    let opts = options(Some(lib.carol), 1, Hints::default());
    let err = ingest_file(&lib.app, &store, "no-artist.opus", &opts, true).await.unwrap_err();
    assert!(ingest::is_refusal(&err));
    assert!(problems_seen_by(&lib.app, lib.alice).await?.is_empty());

    // ...a real one does, with the audio kept and the tags. Trying again updates it.
    for _ in 0..2 {
        ingest_file(&lib.app, &store, "no-artist.opus", &opts, false).await.unwrap_err();
    }
    let listed = problems_seen_by(&lib.app, lib.carol).await?;
    assert_eq!(listed.len(), 1);
    let problem = &listed[0];
    assert_eq!((problem.kind.as_str(), problem.attempts), ("skipped", 2));
    assert!(problem.reason.contains("artist"), "{}", problem.reason);
    assert!(problem.embedded_tags.is_some());
    assert!(store.size(&problem.sha256).await.is_some(), "the audio is kept");

    // Carol's entry: she and admins see it, bob doesn't.
    assert_eq!(problems_seen_by(&lib.app, lib.alice).await?.len(), 1);
    assert!(problems_seen_by(&lib.app, lib.bob).await?.is_empty());
    Ok(())
}

#[sqlx::test]
async fn listed_files_resolve_when_they_import(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Refused first (an unknown status), then imported fine.
    let mut opts = options(None, 1, Hints::default());
    opts.status = "no such status".into();
    ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, false).await.unwrap_err();
    assert_eq!(problems_seen_by(&lib.app, lib.alice).await?.len(), 1);

    opts.status = "released".into();
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, false).await?;
    assert!(problems_seen_by(&lib.app, lib.alice).await?.is_empty());
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let resolved = db::problems::list(&mut tx, true, None, 10).await?;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].track_id, report.track_id);
    assert_eq!(resolved[0].resolution, "imported");
    Ok(())
}

/// Refuses "no-artist.opus" (no ARTIST tag) as `user`, and returns the kept file.
async fn refused_file(lib_pool: &PgPool, store: &BlobStore, opts: &IngestOptions) -> anyhow::Result<ingest::StagedFile> {
    ingest_file(lib_pool, store, "no-artist.opus", opts, false).await.unwrap_err();
    let (sha, size): (String, i64) = sqlx::query_as("SELECT sha256, size_bytes FROM ingest_problems")
        .fetch_one(lib_pool)
        .await?;
    Ok(ingest::StagedFile::stored(store, &sha, size as u64).await)
}

#[sqlx::test]
async fn listed_files_import_with_values_given_by_hand(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Refused while being imported restricted to "friends": the entry remembers that.
    let mut opts = options(None, 1, Hints::default());
    opts.restrict_to = Some(vec![lib.friends]);
    let file = refused_file(&lib.owner, &store, &opts).await?;
    let description: serde_json::Value = sqlx::query_scalar("SELECT description FROM ingest_problems")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(description["restrict_to"], json!([lib.friends]));

    // Imported again (what the Review page does), with the artist typed in.
    let description: crate::ingest::describe::NewDescription = serde_json::from_value(description)?;
    let description = description.validate(|_| None).map_err(anyhow::Error::msg)?;
    let again = IngestOptions {
        restrict_to: description.restrict_to,
        given: ingest::Given { title: None, artists: vec!["Typed Artist; Another".into()] },
        uploaded_filename: "no-artist.opus".into(),
        ..opts.clone()
    };
    let report = ingest::ingest_staged(&lib.owner, &store, &file, &again, false).await?;
    let track = report.track_id.unwrap();
    let names: Vec<&str> = report.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Typed Artist", "Another"]);

    // Still restricted, and the revision says what was typed in.
    let restricted: bool = sqlx::query_scalar("SELECT is_restricted FROM tracks WHERE id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert!(restricted);
    let reason: String = sqlx::query_scalar("SELECT reason FROM track_revisions WHERE track_id = $1 AND variant_id IS NOT NULL")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert!(reason.contains("given by hand") && reason.contains("artist"), "{reason}");
    let open: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_problems WHERE resolved_at IS NULL")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(open, 0);
    Ok(())
}

#[sqlx::test]
async fn listed_files_can_be_added_to_a_track(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let file = refused_file(&lib.owner, &store, &options(None, 1, Hints::default())).await?;
    let current = |pool: PgPool, track: Uuid| async move {
        sqlx::query_scalar::<_, Uuid>("SELECT file_id FROM tracks WHERE id = $1").bind(track).fetch_one(&pool).await
    };
    let replace = ingest::ReplaceOptions {
        user: Some(lib.alice),
        uploaded_filename: "no-artist.opus".into(),
        origin: netease(Kind::Track, "song_id", 1),
        reason: "untagged copy".into(),
        volume: None,
    };

    // As a previous file: the track keeps playing its file.
    let before = current(lib.owner.clone(), lib.v).await?;
    let added = ingest::add_file_to_track(&lib.app, &file, lib.v, ingest::AddAs::PreviousFile, &replace).await?;
    assert_eq!(current(lib.owner.clone(), lib.v).await?, before);
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let v = db::tracks::get_track(&mut tx, lib.v, true).await?.unwrap();
    assert!(v.file_history.as_ref().unwrap().iter().any(|h| h.old_file_id == added.new_file_id));
    drop(tx);
    assert!(problems_seen_by(&lib.app, lib.alice).await?.is_empty(), "resolved");

    // The same file can't be added twice.
    let err = ingest::add_file_to_track(&lib.app, &file, lib.o, ingest::AddAs::CurrentFile, &replace).await.unwrap_err();
    assert!(err.to_string().contains("already stored"), "{err}");
    Ok(())
}

#[sqlx::test]
async fn audio_goes_to_the_chosen_volume_and_covers_to_the_default(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let (ssd, hdd) = (tempfile::tempdir()?, tempfile::tempdir()?);
    let store = BlobStore::open_volumes(&[
        ("main".into(), ssd.path().to_path_buf()),
        ("archive".into(), hdd.path().to_path_buf()),
    ])?;

    // Glass Harbor has an embedded cover.
    let mut opts = options(None, 1, Hints::default());
    opts.volume = Some("archive".into());
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/01 - Glass Harbor.flac");
    let pending = ingest::receive_path(&store, &path, "archive").await?;
    let opts = IngestOptions { uploaded_filename: "01 - Glass Harbor.flac".into(), ..opts };
    ingest::ingest_with_report(&lib.owner, &store, pending, &opts, false).await?;

    let audio: (String, String) = sqlx::query_as(
        "SELECT b.volume, b.sha256 FROM blobs b JOIN files f ON f.sha256 = b.sha256",
    )
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(audio.0, "archive");
    assert!(store.find(&audio.1).await.unwrap().1.starts_with(hdd.path()));
    let covers: Vec<String> = sqlx::query_scalar(
        "SELECT b.volume FROM blobs b JOIN covers c ON c.blob_sha256 = b.sha256",
    )
    .fetch_all(&lib.owner)
    .await?;
    assert!(!covers.is_empty() && covers.iter().all(|v| v == "main"), "{covers:?}");
    Ok(())
}
