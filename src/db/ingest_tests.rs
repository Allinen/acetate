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
use crate::model::sources::{self, Kind, NewSource, SourceOwner, ValidSource};
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
        edited_after: false,
        tags_from: None,
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
            platform_name: None,
        }],
        lax_artist_names: false,
        album: album_id.map(|id| AlbumHint {
            sources: vec![netease(Kind::Album, "album_id", id)],
            ..Default::default()
        }),
        ..Default::default()
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
    let album = db::ingest::create_album(&mut tx, "Same Title", &db::ingest::album_roles_from_tags(&[a.clone(), b.clone()], true), "single", Some("2020")).await?;
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

    // An album with only H, which carol can't see: naming it by id is "not found".
    let mut tx = db::begin(&lib.owner, None).await?;
    let secret = db::revisions::create_album(
        &mut tx,
        &crate::model::AlbumData { title: "Secret".into(), ..Default::default() },
        db::revisions::Source::Import,
        "",
    )
    .await?;
    sqlx::query("INSERT INTO album_tracks (album_id, track_id) VALUES ($1, $2)")
        .bind(secret)
        .bind(lib.h)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let hints = Hints {
        album: Some(AlbumHint { id: Some(secret), ..Default::default() }),
        ..Default::default()
    };
    let err = ingest_file(&lib.app, &store, "01 - Glass Harbor.flac", &options(Some(lib.carol), 1, hints), false)
        .await
        .unwrap_err();
    assert!(matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::NotFound("album"))), "{err:#}");

    // X is only credited on H, but artists are visible to everyone logged in (migration
    // 0027): naming it by id works, and so does matching it by a platform id.
    let mut tx = db::begin(&lib.owner, None).await?;
    db::sources::add_snapshot(&mut tx, crate::model::sources::SourceOwner::Artist(lib.x), &netease(Kind::Artist, "artist_id", 99)).await?;
    tx.commit().await?;
    let report = ingest_file(&lib.app, &store, "01 - Glass Harbor.flac", &options(Some(lib.carol), 1, mira_vale(99, None)), false).await?;
    let found = artist(&report, "Mira Vale");
    assert_eq!((found.matched_by, found.id), (MatchedBy::PlatformId, Some(lib.x)));
    Ok(())
}

#[sqlx::test]
async fn original_filename_comes_from_the_tags_else_the_description(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // Glass Harbor has ORIGINALFILENAME=glass_harbor_MASTER_v7.wav; Undertow has none, so
    // it has no original filename (not the name it was uploaded with). A described name
    // is only used without one in the tags.
    let described = Hints { original_filename: Some("described.wav".into()), ..Default::default() };
    ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, described.clone()), false).await?;
    ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, Hints::default()), false).await?;
    ingest_file(&lib.owner, &store, "salt-and-static.m4a", &options(None, 3, described), false).await?;
    let names: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT uploaded_filename, original_filename FROM files ORDER BY uploaded_filename",
    )
    .fetch_all(&lib.owner)
    .await?;
    let find = |uploaded: &str| names.iter().find(|(u, _)| u == uploaded).unwrap().1.clone();
    assert_eq!(find("01 - Glass Harbor.flac").as_deref(), Some("glass_harbor_MASTER_v7.wav"));
    assert_eq!(find("02 - Undertow.flac"), None);
    assert_eq!(find("salt-and-static.m4a").as_deref(), Some("described.wav"));
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
async fn unknown_groups_are_refused_also_for_a_stored_file(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let opts = options(None, 1, Hints::default());
    ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, false).await?;

    // The same file again (a duplicate), dry run and for real: a wrong group id is refused.
    let wrong = IngestOptions { restrict_to: Some(vec![Uuid::now_v7()]), ..opts.clone() };
    for dry_run in [true, false] {
        let err = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &wrong, dry_run).await.unwrap_err();
        assert!(ingest::is_refusal(&err) && err.to_string().contains("unknown group"), "{err:#}");
    }
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
async fn listed_files_keep_their_covers_and_go_to_the_trash_when_resolved(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let hints = Hints { track_covers: vec![b"a cover sent along".to_vec()], ..Hints::default() };
    let file = refused_file(&lib.owner, &store, &options(Some(lib.carol), 1, hints)).await?;

    // The cover is kept, and named in the description by its hash.
    let (id, description, covers): (Uuid, serde_json::Value, Vec<String>) =
        sqlx::query_as("SELECT id, description, cover_blobs FROM ingest_problems").fetch_one(&lib.owner).await?;
    assert_eq!(covers.len(), 1);
    assert_eq!(description["track"]["covers"], json!([format!("sha256:{}", covers[0])]));
    assert!(store.size(&covers[0]).await.is_some());
    let trash = |pool: PgPool| async move {
        let mut tx = db::begin(&pool, Some(lib.carol)).await?;
        anyhow::Ok(db::trash::list(&mut tx).await?.into_iter().filter(|t| t.kind == "problem").map(|t| t.id).collect::<Vec<_>>())
    };
    assert!(trash(lib.app.clone()).await?.is_empty(), "open: not in the trash");

    // Resolved by hand ("junk"): in the trash; restoring opens it again.
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::problems::resolve(&mut tx, id, "junk").await?);
    tx.commit().await?;
    assert_eq!(trash(lib.app.clone()).await?, vec![id]);
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    assert!(db::trash::restore(&mut tx, db::trash::Kind::Problem, id).await?);
    assert!(db::problems::get_open(&mut tx, id).await?.is_some());
    assert!(db::problems::resolve(&mut tx, id, "junk").await?);
    tx.commit().await?;

    // Purged after the grace period: the audio and the cover go, the entry stays.
    sqlx::query("UPDATE ingest_problems SET resolved_at = now() - interval '8 days'").execute(&lib.owner).await?;
    let mut tx = db::begin(&lib.app, Some(lib.alice)).await?;
    let purged = db::trash::purge(&mut tx, &store, &[(db::trash::Kind::Problem, id)], &file.sha256[..8]).await?;
    tx.commit().await?;
    let mut deleted = purged.delete_blobs.clone();
    deleted.sort();
    let mut expected = vec![file.sha256.clone(), covers[0].clone()];
    expected.sort();
    assert_eq!(deleted, expected);
    assert!(trash(lib.app.clone()).await?.is_empty());
    Ok(())
}

#[sqlx::test]
async fn listed_files_import_again_keeping_their_restriction(
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

    // Imported again (what the Review page does), its artist to be given by an edit.
    let description: crate::ingest::describe::NewDescription = serde_json::from_value(description)?;
    let description = description.validate(|_| None).map_err(anyhow::Error::msg)?;
    let again = IngestOptions {
        restrict_to: description.restrict_to,
        edited_after: true,
        uploaded_filename: "no-artist.opus".into(),
        ..opts.clone()
    };
    let report = ingest::ingest_staged(&lib.owner, &store, &file, &again, false).await?;
    let track = report.track_id.unwrap();

    // Still restricted.
    let restricted: bool = sqlx::query_scalar("SELECT is_restricted FROM tracks WHERE id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert!(restricted);
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
        // Said to be an original file (its tags don't say).
        original_filename: Some("no-artist.opus".into()),
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
    let og: Option<String> = sqlx::query_scalar("SELECT original_filename FROM files WHERE id = $1").bind(added.new_file_id).fetch_one(&lib.owner).await?;
    assert_eq!(og.as_deref(), Some("no-artist.opus"));

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

/// An artist with another spelling of the name, `name`, as NetEase artist `netease_id`.
async fn existing_artist(lib: &super::rls_tests::Library, name: &str, netease_id: i64) -> anyhow::Result<Uuid> {
    let mut tx = db::begin(&lib.owner, None).await?;
    let data = crate::model::ArtistData { name: name.into(), ..Default::default() };
    let id = db::revisions::create_artist(&mut tx, &data, db::revisions::Source::Edit, "").await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Artist(id), &netease(Kind::Artist, "artist_id", netease_id)).await?;
    tx.commit().await?;
    Ok(id)
}

async fn netease_ids(pool: &PgPool, artist: Uuid) -> anyhow::Result<Vec<String>> {
    let mut tx = db::begin(pool, None).await?;
    Ok(db::ingest::platform_ids_of(&mut tx, SourceOwner::Artist(artist), "netease", false).await?)
}

async fn aliases_of(pool: &PgPool, artist: Uuid) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT alias FROM artist_aliases WHERE artist_id = $1 ORDER BY alias")
        .bind(artist)
        .fetch_all(pool)
        .await?)
}

#[sqlx::test]
async fn same_name_in_another_case_with_another_id_is_another_artist(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let upper = existing_artist(&lib, "MIRA VALE", 11).await?;

    // "Mira Vale" is NetEase artist 10: not the "MIRA VALE" that is 11.
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, mira_vale(10, None)), false).await?;
    let mira = artist(&report, "Mira Vale");
    assert_eq!(mira.matched_by, MatchedBy::Created);
    assert_ne!(mira.id, Some(upper));
    assert_eq!(netease_ids(&lib.owner, mira.id.unwrap()).await?, vec!["10".to_string()]);
    assert_eq!(netease_ids(&lib.owner, upper).await?, vec!["11".to_string()]);
    assert_eq!(aliases_of(&lib.owner, upper).await?, Vec::<String>::new(), "no \"Mira Vale\" alias on the other artist");
    Ok(())
}

#[sqlx::test]
async fn same_name_in_another_case_without_a_conflict_is_the_same_artist(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let upper = existing_artist(&lib, "MIRA VALE", 10).await?;

    // No ids at all for this file: the name decides, and the spelling becomes an alias.
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, Hints::default()), false).await?;
    assert_eq!(artist(&report, "Mira Vale").id, Some(upper));
    assert_eq!(aliases_of(&lib.owner, upper).await?, vec!["Mira Vale".to_string()]);
    Ok(())
}

#[sqlx::test]
async fn a_hint_matches_its_tag_name_ignoring_case(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // The folder says "mira vale - 10", the tag "Mira Vale": still NetEase artist 10.
    let hints = Hints {
        artists: vec![ArtistHint {
            name: "mira vale".into(),
            id: None,
            sources: vec![netease(Kind::Artist, "artist_id", 10)],
            platform_name: None,
        }],
        album: None,
        ..Default::default()
    };
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, hints), false).await?;
    assert_eq!(netease_ids(&lib.owner, artist(&report, "Mira Vale").id.unwrap()).await?, vec!["10".to_string()]);
    Ok(())
}

/// Whether `track`, if it were called "Glass Harbor" by `artists`, gets a possible
/// duplicate flag (the flags from before are cleared first).
async fn flagged_as_glass_harbor(pool: &PgPool, track: Uuid, artists: &[Uuid]) -> anyhow::Result<bool> {
    sqlx::query("DELETE FROM review_flags").execute(pool).await?;
    let mut tx = db::begin(pool, None).await?;
    db::review::flag_possible_duplicates(&mut tx, track, "Glass Harbor", artists, &[]).await?;
    tx.commit().await?;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM review_flags WHERE track_id = $1")
        .bind(track)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

async fn set_length(pool: &PgPool, track: Uuid, ms: Option<i64>) -> anyhow::Result<()> {
    sqlx::query("UPDATE files SET duration_ms = $2 WHERE id = (SELECT file_id FROM tracks WHERE id = $1)")
        .bind(track)
        .bind(ms)
        .execute(pool)
        .await?;
    Ok(())
}

#[sqlx::test]
async fn similar_titles_are_flagged_only_at_about_the_same_length(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // "Glass Harbor" by Mira Vale and "Undertow", both 4 s long.
    let glass = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, Hints::default()), false).await?;
    let other = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, Hints::default()), false).await?;
    let (glass, other) = (glass.track_id.unwrap(), other.track_id.unwrap());
    let mira: Vec<Uuid> = sqlx::query_scalar("SELECT artist_id FROM track_credits WHERE track_id = $1")
        .bind(glass)
        .fetch_all(&lib.owner)
        .await?;
    let pool = &lib.owner;

    assert!(flagged_as_glass_harbor(pool, other, &mira).await?, "same title, same length");
    set_length(pool, glass, Some(6_500)).await?;
    assert!(flagged_as_glass_harbor(pool, other, &mira).await?, "2.5 s apart: rips differ like that");
    set_length(pool, glass, Some(9_000)).await?;
    assert!(!flagged_as_glass_harbor(pool, other, &mira).await?, "5 s apart: another song, whatever the title says");
    set_length(pool, glass, None).await?;
    assert!(flagged_as_glass_harbor(pool, other, &mira).await?, "unknown length: not ruled out");
    sqlx::query("UPDATE tracks SET title = 'Glass Harbor (Instrumental)' WHERE id = $1").bind(glass).execute(pool).await?;
    assert!(!flagged_as_glass_harbor(pool, other, &mira).await?, "another version of the song");
    Ok(())
}

#[sqlx::test]
async fn lax_artist_names_give_a_lone_tag_artist_the_platform_artist(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // The folder says "Jay Wren - 10"; the file is tagged "Mira Vale" (another name).
    let kay_angel = |lax| Hints {
        artists: vec![ArtistHint {
            name: "Jay Wren".into(),
            id: None,
            sources: vec![netease(Kind::Artist, "artist_id", 10)],
            platform_name: None,
        }],
        lax_artist_names: lax,
        ..Default::default()
    };

    // Strict (the default): the id isn't used.
    let strict = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, kay_angel(false)), true).await?;
    assert_eq!(artist(&strict, "Mira Vale").matched_by, MatchedBy::Created);
    assert!(strict.warnings.iter().any(|w| w.contains("\"Jay Wren\" isn't in the file's")), "{:?}", strict.warnings);

    // Lax: it's Jay Wren (created under the platform's name), credited as "Mira Vale".
    let lax = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, kay_angel(true)), false).await?;
    assert!(lax.warnings.iter().any(|w| w.contains("went to the tag artist \"Mira Vale\"")), "{:?}", lax.warnings);
    let kay = artist(&lax, "Mira Vale").id.unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM artists WHERE id = $1").bind(kay).fetch_one(&lib.owner).await?;
    assert_eq!(name, "Jay Wren");
    assert_eq!(aliases_of(&lib.owner, kay).await?, vec!["Mira Vale".to_string()]);
    assert_eq!(netease_ids(&lib.owner, kay).await?, vec!["10".to_string()]);
    let alias_credited: bool = sqlx::query_scalar(
        "SELECT al.alias = 'Mira Vale' FROM track_credits c JOIN artist_aliases al ON al.id = c.alias_id WHERE c.artist_id = $1",
    )
    .bind(kay)
    .fetch_one(&lib.owner)
    .await?;
    assert!(alias_credited);

    // Two tag artists without ids ("Mira Vale feat. Oskar Lind", album artist "Mira Vale"):
    // not clear-cut, so nothing's moved even when lax.
    let unclear = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 2, kay_angel(true)), true).await?;
    assert!(unclear.warnings.iter().any(|w| w.contains("\"Jay Wren\" isn't in the file's")), "{:?}", unclear.warnings);
    Ok(())
}

#[sqlx::test]
async fn a_described_release_date_is_only_used_without_one_in_the_tags(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let dated = Hints { release_date: Some("2021-09-06".into()), ..Default::default() };
    let first_revision = async |track: Uuid| -> anyhow::Result<(Option<String>, String)> {
        Ok(sqlx::query_as(
            "SELECT t.release_date, r.reason FROM tracks t JOIN track_revisions r ON r.track_id = t.id WHERE t.id = $1",
        )
        .bind(track)
        .fetch_one(&lib.owner)
        .await?)
    };

    // The tags have a date: it's theirs.
    let tagged = ingest_file(&lib.owner, &store, "02 - Undertow.flac", &options(None, 1, dated.clone()), false).await?;
    let (date, reason) = first_revision(tagged.track_id.unwrap()).await?;
    assert_eq!(date.as_deref(), Some("2021-06-04"));
    assert_eq!(reason, "imported_from_file");

    // They don't: the described one, and the revision says so.
    let opts = IngestOptions {
        edited_after: true,
        ..options(None, 2, dated)
    };
    let undated = ingest_file(&lib.owner, &store, "no-artist.opus", &opts, false).await?;
    let (date, reason) = first_revision(undated.track_id.unwrap()).await?;
    assert_eq!(date.as_deref(), Some("2021-09-06"));
    assert!(reason.contains("release date from the description"), "{reason}");
    Ok(())
}

#[sqlx::test]
async fn an_alias_in_use_says_where_when_removed(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // Jay Wren, credited as "Mira Vale" (see the lax artist names test).
    let hints = Hints {
        artists: vec![ArtistHint {
            name: "Jay Wren".into(),
            id: None,
            sources: vec![netease(Kind::Artist, "artist_id", 10)],
            platform_name: None,
        }],
        lax_artist_names: true,
        ..Default::default()
    };
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, hints), false).await?;
    let kay = artist(&report, "Mira Vale").id.unwrap();

    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    let detail = db::catalog::get_artist(&mut tx, kay, false).await?.unwrap();
    let alias = detail.aliases[0].id;
    let uses = db::revisions::alias_uses_seen(&mut tx, alias).await?;
    let titles: Vec<&str> = uses.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Glass Harbor"]);
    assert!(!uses.not_visible);

    // Removing it is refused, naming the track.
    let data = crate::model::ArtistData { name: "Jay Wren".into(), ..Default::default() };
    let err = db::revisions::write_artist(&mut tx, kay, detail.revision_id, &data, db::revisions::Source::Edit, "")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("track \"Glass Harbor\""), "{err}");
    Ok(())
}

#[sqlx::test]
async fn composers_are_credited_as_producers_after_the_artists(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Accessor, ItemKey, TagExt};

    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // Undertow with COMPOSER "Mira Vale; Kit Harlow": Mira Vale (already the artist) and
    // someone new.
    let path = dir.path().join("undertow.flac");
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/02 - Undertow.flac"), &path)?;
    let mut file = lofty::read_from_path(&path)?;
    let tag = file.primary_tag_mut().unwrap();
    tag.insert_text(ItemKey::Composer, "Mira Vale; Kit Harlow".into());
    tag.set_title("Undertow (produced)".into()); // not a duplicate of the fixture
    tag.save_to_path(&path, WriteOptions::default())?;

    let pending = ingest::receive_path(&store, &path, "main").await?;
    let report = ingest::ingest_with_report(&lib.owner, &store, pending, &options(None, 1, Hints::default()), false).await?;
    let credits: Vec<(String, String)> = sqlx::query_as(
        "SELECT a.name, c.role FROM track_credits c JOIN artists a ON a.id = c.artist_id
         WHERE c.track_id = $1 ORDER BY c.position",
    )
    .bind(report.track_id.unwrap())
    .fetch_all(&lib.owner)
    .await?;
    let credits: Vec<(&str, &str)> = credits.iter().map(|(a, r)| (a.as_str(), r.as_str())).collect();
    assert_eq!(
        credits,
        [("Mira Vale feat. Oskar Lind", "primary"), ("Mira Vale", "producer"), ("Kit Harlow", "producer")]
    );

    // With `no_composers`, only the artists. (Another title, so it's not the same file.)
    let mut file = lofty::read_from_path(&path)?;
    let tag = file.primary_tag_mut().unwrap();
    tag.set_title("Undertow (no producers)".into());
    tag.save_to_path(&path, WriteOptions::default())?;
    let pending = ingest::receive_path(&store, &path, "main").await?;
    let hints = Hints { no_composers: true, ..Default::default() };
    let report = ingest::ingest_with_report(&lib.owner, &store, pending, &options(None, 2, hints), true).await?;
    // (Mira Vale is still there as the album artist.)
    let names: Vec<&str> = report.artists.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Mira Vale feat. Oskar Lind", "Mira Vale"]);
    Ok(())
}

/// A PNG header (1×1): enough to count as an image.
fn tiny_png(tag: u8) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R'];
    png.extend([0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0, 0, 0, tag]);
    png
}

#[sqlx::test]
async fn soundcloud_hints_date_uploader_and_covers(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let uploader = sources::validate(
        Kind::Artist,
        NewSource { platform: "soundcloud".into(), source_id: None, captured_at: None, data: json!({ "artist_id": 777 }) },
    )?;
    let hints = Hints {
        release_date: Some("2020-02-02T02:02:02Z".into()),
        release_date_over_tags: true,
        first_artist_sources: vec![uploader],
        track_covers: vec![tiny_png(1), tiny_png(2), tiny_png(1)],
        skip_embedded_covers: true,
        album: Some(AlbumHint { release_date: Some("2020-01-01".into()), ..Default::default() }),
        ..Default::default()
    };
    // Glass Harbor: ARTIST "Mira Vale", a date in the tags and an embedded PNG.
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, hints), false).await?;
    let track = report.track_id.unwrap();

    // The given date wins, and the revision says what the tags had.
    let (date, reason): (Option<String>, String) = sqlx::query_as(
        "SELECT t.release_date, r.reason FROM tracks t JOIN track_revisions r ON r.track_id = t.id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(date.as_deref(), Some("2020-02-02T02:02:02Z"));
    assert!(reason.contains("the tags say 2021-06-04T11:11:11Z"), "{reason}");
    let album_date: Option<String> = sqlx::query_scalar("SELECT release_date FROM albums WHERE id = $1")
        .bind(report.album.as_ref().unwrap().id.unwrap())
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(album_date.as_deref(), Some("2020-01-01"), "the album's own date");

    // The uploader's id went to the first ARTIST.
    let mira = artist(&report, "Mira Vale").id.unwrap();
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT external_id FROM sources WHERE artist_id = $1 AND platform = 'soundcloud'",
    )
    .bind(mira)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(ids, ["777"]);

    // Only the given covers (each once), not the embedded one.
    let covers: Vec<String> = sqlx::query_scalar("SELECT source FROM covers WHERE track_id = $1 ORDER BY position")
        .bind(track)
        .fetch_all(&lib.owner)
        .await?;
    assert_eq!(covers, ["description", "description"]);
    Ok(())
}

#[sqlx::test]
async fn the_path_a_file_came_from_is_kept_and_filled_in_once(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let imported_from = || async {
        sqlx::query_scalar::<_, Option<String>>("SELECT imported_from FROM files WHERE uploaded_filename = '02 - Undertow.flac'")
            .fetch_one(&lib.owner)
            .await
    };
    let from = |path: Option<&str>| IngestOptions { local_path: path.map(str::to_string), ..options(None, 1, Hints::default()) };

    // Uploaded without a path: unknown.
    ingest_file(&lib.owner, &store, "02 - Undertow.flac", &from(None), false).await?;
    assert_eq!(imported_from().await?, None);
    // Imported again from a folder: the path is filled in (it's a duplicate otherwise)...
    ingest_file(&lib.owner, &store, "02 - Undertow.flac", &from(Some("/music/a/Undertow.flac")), false).await?;
    assert_eq!(imported_from().await?.as_deref(), Some("/music/a/Undertow.flac"));
    // ...and not changed by another copy elsewhere.
    ingest_file(&lib.owner, &store, "02 - Undertow.flac", &from(Some("/music/b/Undertow.flac")), false).await?;
    assert_eq!(imported_from().await?.as_deref(), Some("/music/a/Undertow.flac"));

    let mut tx = lib.owner.begin().await?;
    assert_eq!(db::ingest::tracks_imported_from(&mut tx, "/music/a/Undertow.flac").await?.len(), 1);
    assert!(db::ingest::tracks_imported_from(&mut tx, "/music/b/Undertow.flac").await?.is_empty());
    Ok(())
}

#[sqlx::test]
async fn metadata_can_come_from_a_tagged_copy_of_an_untouched_file(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // The untouched file has no tags at all; its copy is Glass Harbor, fully tagged.
    let copy = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/01 - Glass Harbor.flac");
    let opts = IngestOptions {
        tags_from: Some(ingest::TagsFrom { path: copy, filename: "01 - Glass Harbor.flac".into() }),
        ..options(None, 1, Hints::default())
    };
    let report = ingest_file(&lib.owner, &store, "untitled_bounce_final_v3.flac", &opts, false).await?;
    let track = report.track_id.unwrap();

    let (title, artist, reason): (String, String, String) = sqlx::query_as(
        "SELECT t.title, t.artist_display, r.reason FROM tracks t JOIN track_revisions r ON r.track_id = t.id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!((title.as_str(), artist.as_str()), ("Glass Harbor", "Mira Vale"));
    assert!(reason.contains("metadata from the tags of a copy (01 - Glass Harbor.flac)"), "{reason}");

    // The stored file is the untouched one: its own (empty) tags, the copy's next to them,
    // and the copy's original filename and picture.
    let (uploaded, original, own, from_copy): (String, Option<String>, serde_json::Value, serde_json::Value) = sqlx::query_as(
        "SELECT f.uploaded_filename, f.original_filename, f.embedded_tags->'tags', f.embedded_tags->'tags_from_copy'->'filename'
         FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(uploaded, "untitled_bounce_final_v3.flac");
    assert_eq!(original.as_deref(), Some("glass_harbor_MASTER_v7.wav"));
    assert_eq!(own, json!([{ "format": "vorbis_comments", "items": [] }]), "the file's own tags: none");
    assert_eq!(from_copy, json!("01 - Glass Harbor.flac"));
    let covers: i64 = sqlx::query_scalar("SELECT count(*) FROM covers WHERE track_id = $1").bind(track).fetch_one(&lib.owner).await?;
    assert_eq!(covers, 1, "the copy's embedded picture");
    Ok(())
}

async fn albums_of(pool: &PgPool, track: Uuid) -> anyhow::Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT a.title, a.kind FROM album_tracks at JOIN albums a ON a.id = at.album_id WHERE at.track_id = $1 ORDER BY a.kind",
    )
    .bind(track)
    .fetch_all(pool)
    .await?)
}

#[sqlx::test]
async fn a_stored_file_imported_again_merges_what_is_new(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let on = |album_id: i64, single: bool| Hints {
        album: Some(AlbumHint {
            sources: vec![netease(Kind::Album, "album_id", album_id)],
            single: Some(single),
            ..Default::default()
        }),
        ..Default::default()
    };
    let merges = || async { count(&lib.owner, "SELECT count(*) FROM track_merges").await };

    // On the album first.
    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, on(100, false)), false).await?;
    let track = first.track_id.unwrap();

    // The same file again, same album: nothing new, a plain duplicate, no merge.
    let again = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, on(100, false)), false).await?;
    assert_eq!(again.status, ReportStatus::Duplicate);
    assert!(again.added.is_empty(), "{:?}", again.added);
    assert_eq!(merges().await?, 0);

    // As a single (another NetEase album): the track goes on the single too.
    let dry = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, on(200, true)), true).await?;
    assert!(dry.added.iter().any(|a| a == "on the single \"Tidelines\""), "{:?}", dry.added);
    assert_eq!(merges().await?, 0, "a dry run keeps nothing");
    let single = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, on(200, true)), false).await?;
    assert_eq!((single.status, single.track_id), (ReportStatus::Duplicate, Some(track)));
    assert!(single.added.iter().any(|a| a == "on the single \"Tidelines\""), "{:?}", single.added);
    assert_eq!(
        albums_of(&lib.owner, track).await?,
        [("Tidelines".to_string(), "album".to_string()), ("Tidelines".to_string(), "single".to_string())]
    );
    assert_eq!(merges().await?, 1);
    // Nothing doubled: same metadata (no new variant), same cover, same lyrics.
    let variants = count(&lib.owner, "SELECT count(*) FROM track_variants v JOIN tracks t ON t.id = v.track_id WHERE t.merged_into IS NULL AND t.title = 'Glass Harbor'").await?;
    assert_eq!(variants, 1);
    let (covers, lyrics): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM covers WHERE track_id = $1), (SELECT count(*) FROM lyrics WHERE track_id = $1)",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(covers, 1);
    assert!(lyrics <= 1);
    // The merged-away track plays the same file; the kept one still has it.
    let files = count(&lib.owner, "SELECT count(*) FROM files").await?;
    assert_eq!(files, 4, "the library's 3, plus Glass Harbor once");

    // Undone like any merge: off the single again.
    let merge: Uuid = sqlx::query_scalar("SELECT id FROM track_merges").fetch_one(&lib.owner).await?;
    let mut tx = db::begin(&lib.app, Some(lib.carol)).await?;
    db::revisions::undo_track_merge(&mut tx, merge, "").await?;
    tx.commit().await?;
    assert_eq!(albums_of(&lib.owner, track).await?, [("Tidelines".to_string(), "album".to_string())]);

    // Turned off: only the origin, as before.
    let off = Hints { no_duplicate_merge: true, ..on(300, true) };
    let plain = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, off), false).await?;
    assert!(plain.added.is_empty());
    assert_eq!(albums_of(&lib.owner, track).await?.len(), 1);
    Ok(())
}

#[sqlx::test]
async fn a_tagged_copy_with_other_metadata_becomes_another_variant(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // Stored with Glass Harbor's tags (from a copy); then again with Undertow's.
    let copy = |name: &str| IngestOptions {
        tags_from: Some(ingest::TagsFrom {
            path: Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(name),
            filename: name.into(),
        }),
        ..options(None, 1, Hints::default())
    };
    let first = ingest_file(&lib.owner, &store, "untitled_bounce_final_v3.flac", &copy("01 - Glass Harbor.flac"), false).await?;
    let track = first.track_id.unwrap();
    let again = ingest_file(&lib.owner, &store, "untitled_bounce_final_v3.flac", &copy("02 - Undertow.flac"), false).await?;
    assert_eq!(again.track_id, Some(track));
    assert!(again.added.iter().any(|a| a.contains("differs (as another variant)")), "{:?}", again.added);
    // The track's own metadata stays; Undertow's is a second variant.
    let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id = $1").bind(track).fetch_one(&lib.owner).await?;
    assert_eq!(title, "Glass Harbor");
    let variants: i64 = sqlx::query_scalar("SELECT count(*) FROM track_variants WHERE track_id = $1").bind(track).fetch_one(&lib.owner).await?;
    assert_eq!(variants, 2);
    Ok(())
}

#[sqlx::test]
async fn an_album_made_for_a_track_gets_its_covers(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::tag::TagExt;

    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let pool = &lib.owner;
    // A track's or an album's covers (ids are unique across both), in order.
    let covers_of = async |id: Uuid| -> anyhow::Result<Vec<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT blob_sha256 FROM covers WHERE (track_id = $1 OR album_id = $1) AND trashed_at IS NULL ORDER BY position",
        )
        .bind(id)
        .fetch_all(pool)
        .await?)
    };

    // On the album first (with Glass Harbor's embedded cover): the new album gets a copy.
    let on = |album_id: i64, single: bool| Hints {
        album: Some(AlbumHint { sources: vec![netease(Kind::Album, "album_id", album_id)], single: Some(single), ..Default::default() }),
        ..Default::default()
    };
    let first = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, on(100, false)), false).await?;
    let track = first.track_id.unwrap();
    let album_cover = covers_of(track).await?;
    assert_eq!(album_cover.len(), 1);
    assert_eq!(covers_of(first.album.as_ref().unwrap().id.unwrap()).await?, album_cover);

    // Again as a single, from a tagged copy whose cover is the single's own.
    let copy = dir.path().join("single copy.flac");
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/01 - Glass Harbor.flac"), &copy)?;
    let mut file = lofty::read_from_path(&copy)?;
    let tag = file.primary_tag_mut().unwrap();
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
    tag.push_picture(Picture::unchecked(tiny_png(7)).pic_type(PictureType::CoverFront).mime_type(MimeType::Png).build());
    tag.save_to_path(&copy, WriteOptions::default())?;
    let opts = IngestOptions {
        tags_from: Some(ingest::TagsFrom { path: copy, filename: "single copy.flac".into() }),
        ..options(None, 1, on(200, true))
    };
    let again = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &opts, false).await?;
    assert!(again.added.iter().any(|a| a == "on the single \"Tidelines\""), "{:?}", again.added);

    // The single shows its own cover; the track keeps the album's first, then the single's.
    let single_cover = crate::storage::sha256_hex(&tiny_png(7));
    let single: Uuid = sqlx::query_scalar("SELECT id FROM albums WHERE kind = 'single' AND title = 'Tidelines'")
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(covers_of(single).await?, [single_cover.clone()]);
    assert_eq!(covers_of(track).await?, [album_cover[0].clone(), single_cover.clone()]);

    // Singles made that way before this: no covers. `fix-merged-covers` gives them the
    // import's (simulated here by taking the single's away, in this test database only).
    sqlx::query("DELETE FROM covers WHERE album_id = $1").bind(single).execute(pool).await?;
    let merge: Uuid = sqlx::query_scalar("SELECT id FROM track_merges").fetch_one(pool).await?;
    let mut tx = pool.begin().await?;
    let copied = db::revisions::copy_reimport_covers_to_albums(&mut tx, merge).await?;
    tx.commit().await?;
    assert_eq!(copied, [("Tidelines".to_string(), 1)]);
    assert_eq!(covers_of(single).await?, [single_cover]);
    // The album the track was on before isn't touched, and running it again does nothing.
    assert_eq!(covers_of(first.album.as_ref().unwrap().id.unwrap()).await?, album_cover);
    let mut tx = pool.begin().await?;
    assert!(db::revisions::copy_reimport_covers_to_albums(&mut tx, merge).await?.is_empty());
    Ok(())
}

#[sqlx::test]
async fn given_values_and_the_fix_later_flag(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    // Glass Harbor has a title and ARTIST "Mira Vale": the producer folder wins for the
    // artist; the given title isn't used, since the tags have one.
    let hints = Hints {
        given: crate::ingest::describe::GivenValues {
            title: Some("from the file name".into()),
            title_from: "the file name".into(),
            artists: vec!["Some Producer".into()],
            artists_from: "the producer folder \"Some Producer\"".into(),
            artists_over_tags: true,
        },
        flag: Some("imported quickly".into()),
        ..Default::default()
    };
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, hints), false).await?;
    let track = report.track_id.unwrap();
    let (title, artist, reason): (String, String, String) = sqlx::query_as(
        "SELECT t.title, t.artist_display, r.reason FROM tracks t JOIN track_revisions r ON r.track_id = t.id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!((title.as_str(), artist.as_str()), ("Glass Harbor", "Some Producer"));
    assert!(reason.contains("given with the description: artist from the producer folder \"Some Producer\", over the tags"), "{reason}");
    assert!(!reason.contains("title from"), "{reason}");
    let note: serde_json::Value = sqlx::query_scalar("SELECT related FROM review_flags WHERE kind = 'fix_later' AND track_id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(note, json!({ "note": "imported quickly" }));
    Ok(())
}


// ---- All of the metadata in the description (`metadata`): the tags aren't used ----

/// A description as a client sends it, validated as the API does.
fn described(description: serde_json::Value) -> anyhow::Result<IngestOptions> {
    let description: crate::ingest::describe::NewDescription = serde_json::from_value(description)?;
    let valid = description.validate(|_| None).map_err(anyhow::Error::msg)?;
    Ok(IngestOptions {
        origin: valid.origin,
        sources: valid.track_sources,
        hints: valid.hints,
        ..options(None, 0, Hints::default())
    })
}

fn full_metadata() -> serde_json::Value {
    json!({
        "origin": {"platform": "netease", "data": {"song_id": 501}},
        "metadata": {
            "title": "Real Title",
            "credits": [
                {"name": "Real Artist", "sources": [{"platform": "netease", "data": {"artist_id": 77}}]},
                {"name": "Guest", "role": "featured"},
                {"name": "Beatmaker", "role": "producer"},
                {"name": "Real Artist", "role": "producer"}
            ],
            "release_date": "2020-01-01",
            "genres": ["Rap"],
            "comment": "from the platform",
            "album": {
                "title": "Real EP", "kind": "ep", "track_number": 3, "disc_number": 1,
                "sources": [{"platform": "netease", "data": {"album_id": 88}}]
            },
            "lyrics": [{"body": "[00:01.00] line", "official": true, "language": "en"}]
        }
    })
}

#[sqlx::test]
async fn described_metadata_is_the_track_and_the_tags_are_only_kept(
    _: PgPoolOptions,
    connect: PgConnectOptions,
) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;

    // A tagged file (Mira Vale, "Glass Harbor", on Tidelines): none of it is used.
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &described(full_metadata())?, false).await?;
    assert_eq!(report.status, ReportStatus::Created);
    let track = report.track_id.unwrap();
    let names: Vec<&str> = report.artists.iter().map(|a| a.name.as_str()).collect();
    assert!(!names.contains(&"Mira Vale"), "{names:?}");
    assert_eq!(report.album.as_ref().unwrap().name, "Real EP");

    let mut tx = db::begin(&lib.owner, None).await?;
    let metadata = db::revisions::primary_metadata(&mut tx, track).await?.unwrap();
    drop(tx);
    assert_eq!(
        (metadata.title.as_str(), metadata.release_date.as_deref(), metadata.genres.clone(), metadata.comment.as_deref()),
        ("Real Title", Some("2020-01-01"), vec!["Rap".to_string()], Some("from the platform"))
    );
    let credits: Vec<(String, String)> = sqlx::query_as(
        "SELECT a.name, c.role FROM track_credits c JOIN artists a ON a.id = c.artist_id WHERE c.track_id = $1 ORDER BY c.position",
    )
    .bind(track)
    .fetch_all(&lib.owner)
    .await?;
    let credits: Vec<(&str, &str)> = credits.iter().map(|(n, r)| (n.as_str(), r.as_str())).collect();
    assert_eq!(credits, [("Real Artist", "primary"), ("Guest", "featured"), ("Beatmaker", "producer"), ("Real Artist", "producer")]);
    let album: (String, String, Option<i16>, Option<i16>) = sqlx::query_as(
        "SELECT a.kind, a.title, at.track_number, at.disc_number FROM album_tracks at JOIN albums a ON a.id = at.album_id WHERE at.track_id = $1",
    )
    .bind(track)
    .fetch_one(&lib.owner)
    .await?;
    assert_eq!(album, ("ep".into(), "Real EP".into(), Some(3), Some(1)));
    let album_credits: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM album_credits c JOIN artists a ON a.id = c.artist_id JOIN album_tracks at ON at.album_id = c.album_id WHERE at.track_id = $1",
    )
    .bind(track)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(album_credits, ["Real Artist"], "the album's artists: the track's primary ones");
    let label: String = sqlx::query_scalar("SELECT v.label FROM track_variants v JOIN tracks t ON t.primary_variant_id = v.id WHERE t.id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(label, "From the description");
    let lyrics: (String, bool, Option<String>) = sqlx::query_as("SELECT source, is_official, language FROM lyrics WHERE track_id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert_eq!(lyrics, ("description".into(), true, Some("en".into())));
    // The file's own tags: kept, verbatim, with the file.
    let tags: serde_json::Value = sqlx::query_scalar("SELECT f.embedded_tags FROM files f JOIN tracks t ON t.file_id = f.id WHERE t.id = $1")
        .bind(track)
        .fetch_one(&lib.owner)
        .await?;
    assert!(tags.to_string().contains("Tidelines"), "{tags}");

    // A file with no artist in its tags at all: fine, the description has it. The artist
    // is found by its NetEase id, credited under the name given.
    let mut other = full_metadata();
    other["origin"]["data"]["song_id"] = json!(502);
    other["metadata"]["credits"] = json!([{"name": "R. Artist", "sources": [{"platform": "netease", "data": {"artist_id": 77}}]}]);
    other["metadata"]["album"] = json!(null);
    let report = ingest_file(&lib.owner, &store, "no-artist.opus", &described(other)?, false).await?;
    assert_eq!(report.status, ReportStatus::Created);
    assert_eq!(artist(&report, "R. Artist").matched_by, MatchedBy::PlatformId);
    Ok(())
}

#[sqlx::test]
async fn described_metadata_is_checked(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let refused = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut description = full_metadata();
        change(&mut description);
        described(description).err().map(|err| err.to_string()).unwrap_or_default()
    };
    assert!(refused(&|d| d["metadata"]["credits"] = json!([{"name": "A", "role": "featured"}])).contains("primary"));
    assert!(refused(&|d| d["artists"] = json!([{"name": "A"}])).contains("`artists` isn't used with `metadata`"));
    assert!(refused(&|d| d["track"] = json!({"release_date_over_tags": true})).contains("isn't used"));
    assert!(refused(&|d| d["metadata"]["credits"][1] = json!({"name": "Real Artist"})).contains("credited twice as primary"));
    assert!(refused(&|d| d["metadata"]["credits"][1] = json!({"name": "Real Artist", "role": "featured", "id": Uuid::now_v7()})).is_empty(), "one id: fine");

    // Roles and kinds the server doesn't have: refused before anything is made.
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let mut description = full_metadata();
    description["metadata"]["credits"][1]["role"] = json!("vibes");
    let err = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &described(description)?, true).await.unwrap_err();
    assert!(err.to_string().contains("no credit role \"vibes\""), "{err}");
    // Also for a file that's stored already (it's tried as a new track, then merged).
    ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &options(None, 1, Hints::default()), false).await?;
    let mut description = full_metadata();
    description["metadata"]["credits"][1]["role"] = json!("vibes");
    let err = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &described(description)?, false).await.unwrap_err();
    assert!(err.to_string().contains("no credit role \"vibes\""), "{err}");
    let mut description = full_metadata();
    description["metadata"]["album"]["kind"] = json!("boxset");
    let err = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &described(description)?, true).await.unwrap_err();
    assert!(err.to_string().contains("no album kind"), "{err}");
    Ok(())
}

/// A file that didn't import keeps its description (with `metadata`), to import again from
/// the Review page: it reads back the same.
#[test]
fn described_metadata_goes_round_the_review_list() -> anyhow::Result<()> {
    let options = described(full_metadata())?;
    let json = crate::ingest::describe::description_json(&options.origin, "released", &options.sources, &options.hints, None);
    let again = described(json)?;
    let (a, b) = (options.hints.described.unwrap(), again.hints.described.unwrap());
    assert_eq!((a.title, a.credits, a.lyrics.len()), (b.title, b.credits, b.lyrics.len()));
    assert_eq!(a.album.unwrap().kind, "ep");
    assert_eq!(again.hints.artists.iter().find(|h| h.name == "Real Artist").unwrap().sources.len(), 1);
    assert_eq!(again.hints.album.unwrap().track_number, Some(3));
    Ok(())
}


/// Two artists with one name on one track, told apart by their ids: two artists, each with
/// its own ids. The same name without ids that differ is one artist.
#[sqlx::test]
async fn namesakes_are_told_apart_by_their_ids(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let lib = setup(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let mut description = full_metadata();
    description["metadata"]["credits"] = json!([
        {"name": "Lil X", "sources": [{"platform": "netease", "data": {"artist_id": 1}}]},
        {"name": "Lil X", "role": "featured", "sources": [{"platform": "netease", "data": {"artist_id": 2}}]},
        {"name": "Lil X", "role": "producer"}
    ]);
    description["metadata"]["album"] = json!(null);
    let report = ingest_file(&lib.owner, &store, "01 - Glass Harbor.flac", &described(description)?, false).await?;
    let track = report.track_id.unwrap();
    let credits: Vec<(Uuid, String, Vec<String>)> = sqlx::query_as(
        "SELECT c.artist_id, c.role,
                ARRAY(SELECT s.external_id FROM sources s WHERE s.artist_id = c.artist_id AND s.platform = 'netease' ORDER BY 1)
         FROM track_credits c WHERE c.track_id = $1 ORDER BY c.position",
    )
    .bind(track)
    .fetch_all(&lib.owner)
    .await?;
    assert_eq!(credits.len(), 3, "{credits:?}");
    let (primary, featured, producer) = (&credits[0], &credits[1], &credits[2]);
    assert_ne!(primary.0, featured.0, "two artists");
    assert_eq!((primary.2.clone(), featured.2.clone()), (vec!["1".to_string()], vec!["2".to_string()]), "each with its own id");
    assert_eq!(producer.0, primary.0, "the one without ids is the first");

    // The same twice (different Acetate ids): no longer refused.
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    let mut description = full_metadata();
    description["metadata"]["credits"] = json!([{"name": "Y", "id": a}, {"name": "Y", "id": b, "role": "featured"}]);
    let options = described(description)?;
    let credits = &options.hints.described.as_ref().unwrap().credits;
    assert_ne!(credits[0].artist, credits[1].artist);
    Ok(())
}
