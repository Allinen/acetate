//! Importing a file that didn't import with the track editor's changes
//! (`POST /api/ingest-problems/{id}/import`): through the API, with "no-artist.opus" from
//! `test-audio/` (a title, no ARTIST tag), refused once and kept for review.

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

use crate::api::{self, AppState};
use crate::db;
use crate::ingest::{self, IngestOptions, describe};
use crate::model::sources::{self, Kind, NewSource};
use crate::storage::BlobStore;

struct Setup {
    owner: PgPool,
    pool: PgPool,
    store: BlobStore,
    options: IngestOptions,
    app: axum::Router,
    token: String,
    problem: Uuid,
    _dir: tempfile::TempDir,
}

/// An admin with a session, and "no-artist.opus" uploaded by them from NetEase song 7,
/// refused (no artist).
async fn setup(connect: PgConnectOptions) -> anyhow::Result<Setup> {
    setup_with(connect, "no-artist.opus").await
}

/// The same with `file`. One that would import is listed as if it had been refused.
async fn setup_with(connect: PgConnectOptions, file: &str) -> anyhow::Result<Setup> {
    let owner = PgPoolOptions::new().connect_with(connect.clone()).await?;
    let pool = db::test_app_pool(connect).await?;
    let mut tx = db::begin(&owner, None).await?;
    let user = db::users::create_user(&mut tx, "alice", "not a real hash").await?;
    db::users::set_groups(&mut tx, user, &["admin".to_string()]).await?.map_err(anyhow::Error::msg)?;
    tx.commit().await?;
    let token = crate::auth::new_token();
    db::users::create_session(&owner, user, &crate::auth::token_hash(&token), None, "test").await?;

    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let origin = NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": 7 }) };
    let options = IngestOptions {
        user: Some(user),
        uploaded_filename: file.into(),
        origin: sources::validate(Kind::Track, origin)?,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: Default::default(),
        local_path: None,
        edited_after: false,
        tags_from: None,
        volume: None,
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file);
    let pending = ingest::receive_path(&store, &path, "main").await?;
    if ingest::ingest_with_report(&pool, &store, pending, &options, true).await.is_err() {
        let pending = ingest::receive_path(&store, &path, "main").await?;
        assert!(ingest::ingest_with_report(&pool, &store, pending, &options, false).await.is_err());
    } else {
        let pending = ingest::receive_path(&store, &path, "main").await?;
        let staged = ingest::StagedFile::new(&store, pending, false, "main").await?;
        let problem = db::problems::NewProblem {
            kind: "skipped",
            sha256: &staged.sha256,
            size_bytes: staged.size as i64,
            uploaded_filename: file,
            path: None,
            reason: "refused once",
            description: describe::description_json(&options.origin, "released", &[], &Default::default(), None),
            embedded_tags: None,
            tag_title: None,
            tag_artists: &[],
        };
        let mut tx = db::begin(&owner, None).await?;
        db::blobs::register(&mut tx, &staged.sha256, "main", Some(staged.size as i64)).await?;
        db::problems::record(&mut tx, &problem, &[]).await?;
        tx.commit().await?;
    }
    let problem = sqlx::query_scalar("SELECT id FROM ingest_problems").fetch_one(&owner).await?;

    let app = api::router(AppState {
        uploads: Default::default(),
        blob_changes: Default::default(),
        pool: pool.clone(),
        store: BlobStore::open(dir.path())?,
        web_dir: dir.path().to_path_buf(),
            docs_dir: dir.path().join("docs"),
        secure_cookies: false,
        trust_proxy: false,
        rate_limits: std::sync::Arc::new(api::rate_limit::RateLimits::new()),
        images: std::sync::Arc::new(crate::storage::images::ImageCache::new(dir.path().join("cache"))),
        transcodes: std::sync::Arc::new(crate::storage::transcode::TranscodeCache::new(dir.path().join("cache"), vec![128, 64], 0)),
        upload_limits: std::sync::Arc::new(api::upload_limits::UploadLimits::new(0, 0)),
    });
    Ok(Setup { owner, pool, store, options, app, token, problem, _dir: dir })
}

impl Setup {
    async fn import(&self, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(format!("/api/ingest-problems/{}/import", self.problem))
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 10_000_000).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// What's left of imports: tracks, and whether the file is still listed.
    async fn left(&self) -> anyhow::Result<(i64, bool)> {
        let tracks = sqlx::query_scalar("SELECT count(*) FROM tracks").fetch_one(&self.owner).await?;
        let open = sqlx::query_scalar("SELECT resolved_at IS NULL FROM ingest_problems WHERE id = $1")
            .bind(self.problem)
            .fetch_one(&self.owner)
            .await?;
        Ok((tracks, open))
    }
}

/// The editor's changes on the draft: titled, credited to a new artist "Typed Artist",
/// unreleased, a single of its own.
fn changes(draft: &Value, title: &str) -> Value {
    let track = &draft["track"];
    let variant = track["variants"].as_array().unwrap().iter().find(|v| v["id"] == track["primary_variant_id"]).unwrap();
    let artist = Uuid::now_v7();
    json!({
        "track": {
            "track_id": track["id"],
            "meta": {
                "variant_id": variant["id"],
                "base_revision_id": variant["revision_id"],
                "data": { "title": title, "credits": [{ "artist_id": artist, "role": "primary" }] },
            },
            "facts": { "base_revision_id": track["external_revision_id"], "status": "unreleased" },
            "single": true,
        },
        "new_artists": [{ "id": artist, "name": "Typed Artist" }],
        "draft": draft["draft"],
    })
}

fn previewed(mut body: Value) -> Value {
    body["preview"] = json!(true);
    body
}

#[sqlx::test]
async fn a_file_imports_with_the_editors_changes(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;

    // A preview with no changes: the draft, what it lacks, and nothing kept.
    let (status, draft) = s.import(json!({ "preview": true })).await;
    assert_eq!(status, StatusCode::OK, "{draft}");
    assert_eq!(draft["missing"], json!(["artist"]));
    assert!(draft["edited"]["flags"].as_array().unwrap().contains(&json!("missing_artists")), "{draft}");
    let tag_title = draft["track"]["title"].as_str().unwrap().to_string();
    assert!(!tag_title.is_empty());
    // With the source it was imported with.
    assert_eq!(draft["track"]["sources"][0]["platform"], "netease");
    assert_eq!(draft["track"]["file"]["origin"]["source_id"], draft["track"]["sources"][0]["id"]);
    assert_eq!(s.left().await?, (0, true));

    // The changes previewed: nothing lacking, still nothing kept.
    let (status, previewed) = s.import(previewed(changes(&draft, "Typed Title"))).await;
    assert_eq!(status, StatusCode::OK, "{previewed}");
    assert_eq!(previewed["missing"], json!([]));
    assert!(previewed["edited"]["error"].is_null(), "{previewed}");
    assert!(!previewed["edited"]["flags"].as_array().unwrap().contains(&json!("missing_artists")));
    assert!(!previewed["edited"]["flags"].as_array().unwrap().contains(&json!("missing_album")), "a single of its own");
    assert_eq!(s.left().await?, (0, true));
    let artists: i64 = sqlx::query_scalar("SELECT count(*) FROM artists").fetch_one(&s.owner).await?;
    assert_eq!(artists, 0);

    // Done: one track (and its single), the file resolved with it.
    let body = changes(&draft, "Typed Title");
    let (status, done) = s.import(body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let track: Uuid = serde_json::from_value(done["track_id"].clone())?;
    // What it made, for the next file's changes (so it's made once).
    let stand_in = body["new_artists"][0]["id"].as_str().unwrap();
    let made: Uuid = serde_json::from_value(done["made"]["artists"][stand_in].clone())?;
    let named: String = sqlx::query_scalar("SELECT name FROM artists WHERE id = $1").bind(made).fetch_one(&s.owner).await?;
    assert_eq!(named, "Typed Artist");
    assert_eq!(s.left().await?, (1, false));
    let resolved: Option<Uuid> = sqlx::query_scalar("SELECT track_id FROM ingest_problems").fetch_one(&s.owner).await?;
    assert_eq!(resolved, Some(track));
    let (title, status, artist): (String, String, String) = sqlx::query_as(
        "SELECT t.title, t.status, a.name FROM tracks t JOIN track_credits c ON c.track_id = t.id JOIN artists a ON a.id = c.artist_id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&s.owner)
    .await?;
    assert_eq!((title.as_str(), status.as_str(), artist.as_str()), ("Typed Title", "unreleased", "Typed Artist"));
    let singles: i64 = sqlx::query_scalar("SELECT count(*) FROM album_tracks n JOIN albums a ON a.id = n.album_id WHERE n.track_id = $1 AND a.kind = 'single'")
        .bind(track)
        .fetch_one(&s.owner)
        .await?;
    assert_eq!(singles, 1);
    // Revision 0 is what the tags said; the changes come after it.
    let revisions: Vec<(Value, String)> = sqlx::query_as("SELECT data, reason FROM track_revisions WHERE track_id = $1 AND kind = 'internal' ORDER BY id")
        .bind(track)
        .fetch_all(&s.owner)
        .await?;
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].0, json!({ "title": tag_title }));
    assert_eq!(revisions[1].1, "while importing \"no-artist.opus\"");
    // Its source is the one it came with, and its file's origin.
    let (platform, is_origin): (String, bool) = sqlx::query_as(
        "SELECT s.platform, f.origin_snapshot_id IN (SELECT id FROM source_snapshots WHERE source_id = s.id)
         FROM sources s JOIN tracks t ON t.id = s.track_id JOIN files f ON f.id = t.file_id WHERE t.id = $1",
    )
    .bind(track)
    .fetch_one(&s.owner)
    .await?;
    assert_eq!((platform.as_str(), is_origin), ("netease", true));
    Ok(())
}

#[sqlx::test]
async fn refused_without_an_artist(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let (status, refused) = s.import(json!({})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("artist"), "{refused}");
    assert_eq!(s.left().await?, (0, true));

    // A title taken away by the changes: refused too.
    let (_, draft) = s.import(json!({ "preview": true })).await;
    let mut untitled = changes(&draft, "x");
    untitled["track"]["meta"]["data"]["title"] = json!("");
    let (status, refused) = s.import(untitled).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(s.left().await?, (0, true));
    Ok(())
}

#[sqlx::test]
async fn changes_fit_only_their_draft(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let (_, draft) = s.import(json!({ "preview": true })).await;

    // Without the draft, the changes' ids are a preview's that's gone: nothing is kept.
    let mut without = changes(&draft, "Typed Title");
    without.as_object_mut().unwrap().remove("draft");
    let (status, refused) = s.import(without).await;
    assert!(status.is_client_error(), "{status} {refused}");
    assert_eq!(s.left().await?, (0, true));

    // A draft of an import that came out differently: preview again.
    let mut other = changes(&draft, "Typed Title");
    other["draft"]["shape"] = json!("0000");
    let (status, refused) = s.import(other).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["message"].as_str().unwrap().contains("preview it again"), "{refused}");
    assert_eq!(s.left().await?, (0, true));
    Ok(())
}

#[sqlx::test]
async fn what_the_import_made_is_carried_over(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    // Tagged: the import makes its artist (Mira Vale) and album (Tidelines), under new ids
    // each time.
    let s = setup_with(connect, "01 - Glass Harbor.flac").await?;
    let (_, draft) = s.import(json!({ "preview": true })).await;
    assert_eq!(draft["missing"], json!([]));
    let credit = draft["track"]["credits"][0].clone();
    let mut body = changes(&draft, "Typed Title");
    // Kept as the draft has it (and a producer added), on its album still, its source the
    // file's origin.
    body["track"]["meta"]["data"]["credits"] = json!([
        { "artist_id": credit["artist_id"], "role": "primary" },
        { "artist_id": body["new_artists"][0]["id"], "role": "producer" },
    ]);
    body["track"]["facts"]["albums"] = json!([{ "album_id": draft["track"]["albums"][0]["album_id"] }]);
    body["track"]["sources"] = json!({ "origin": draft["track"]["sources"][0]["id"] });
    // (It's alone on its album: not renamed along.)
    body["track"]["along"] = json!(false);
    let (status, done) = s.import(body).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let track = Uuid::parse_str(done["track_id"].as_str().unwrap())?;
    let credits: Vec<(String, String)> = sqlx::query_as(
        "SELECT a.name, c.role FROM track_credits c JOIN artists a ON a.id = c.artist_id WHERE c.track_id = $1 ORDER BY c.position",
    )
    .bind(track)
    .fetch_all(&s.owner)
    .await?;
    let credits: Vec<(&str, &str)> = credits.iter().map(|(a, r)| (a.as_str(), r.as_str())).collect();
    assert_eq!(credits, [("Mira Vale", "primary"), ("Typed Artist", "producer")]);
    let albums: Vec<String> = sqlx::query_scalar("SELECT a.title FROM album_tracks n JOIN albums a ON a.id = n.album_id WHERE n.track_id = $1 ORDER BY a.kind")
        .bind(track)
        .fetch_all(&s.owner)
        .await?;
    assert_eq!(albums, ["Tidelines", "Typed Title"], "its album, and the single");
    // The draft's were the preview's only: one of each.
    let made: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artists WHERE name = 'Mira Vale'), (SELECT count(*) FROM albums WHERE title = 'Tidelines')")
        .fetch_one(&s.owner)
        .await?;
    assert_eq!(made, (1, 1));
    Ok(())
}

#[sqlx::test]
async fn a_drafts_pictures_can_be_seen(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup_with(connect, "01 - Glass Harbor.flac").await?;
    let (_, draft) = s.import(json!({ "preview": true })).await;
    let sha = draft["track"]["covers"][0]["blob_sha256"].as_str().expect("an embedded picture").to_string();
    let request = Request::builder()
        .uri(format!("/api/images/{sha}"))
        .header("authorization", format!("Bearer {}", s.token))
        .body(Body::empty())
        .unwrap();
    let response = s.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[sqlx::test]
async fn possible_duplicates_are_of_the_changed_track(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    // "Glass Harbor" by Mira Vale, already there.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio/01 - Glass Harbor.flac");
    let pending = ingest::receive_path(&s.store, &path, "main").await?;
    let options = IngestOptions { uploaded_filename: "01 - Glass Harbor.flac".into(), ..s.options.clone() };
    let there = ingest::ingest_with_report(&s.pool, &s.store, pending, &options, false).await?;
    let mira = there.artists[0].id.unwrap();

    let (_, draft) = s.import(json!({ "preview": true })).await;
    let mut body = previewed(changes(&draft, "Glass Harbor"));
    body["track"]["meta"]["data"]["credits"] = json!([{ "artist_id": mira, "role": "primary" }]);
    let (status, previewed) = s.import(body).await;
    assert_eq!(status, StatusCode::OK, "{previewed}");
    assert!(previewed["edited"]["flags"].as_array().unwrap().contains(&json!("possible_duplicate")), "{previewed}");
    Ok(())
}
