//! Mirrors: the blob list and its deltas, waiting for changes, and the bytes. With
//! "01 - Glass Harbor.flac" from `test-audio/`.

use std::path::Path;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower::ServiceExt;

use crate::api::{self, AppState};
use crate::db;
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource};
use crate::storage::BlobStore;

struct Setup {
    owner: PgPool,
    app: axum::Router,
    admin: String,
    /// Has `download_original`, isn't an admin.
    listener: String,
    /// Plays "01 - Glass Harbor.flac" (`sha256`).
    track: uuid::Uuid,
    sha256: String,
    /// Plays "02 - Undertow.flac".
    other_track: uuid::Uuid,
    other_sha256: String,
    original: Vec<u8>,
    _dir: tempfile::TempDir,
}

async fn user(owner: &PgPool, name: &str, group: &str) -> anyhow::Result<String> {
    let mut tx = db::begin(owner, None).await?;
    let id = db::users::create_user(&mut tx, name, "not a real hash").await?;
    db::users::set_groups(&mut tx, id, &[group.to_string()]).await?.map_err(anyhow::Error::msg)?;
    tx.commit().await?;
    let token = crate::auth::new_token();
    db::users::create_session(owner, id, &crate::auth::token_hash(&token), None, "test").await?;
    Ok(token)
}

/// Imports a file from `test-audio/` as a track: its id and the file's hash.
async fn ingest_file(pool: &PgPool, store: &BlobStore, file_name: &str, song_id: u32) -> anyhow::Result<(uuid::Uuid, String)> {
    let options = IngestOptions {
        user: None,
        uploaded_filename: file_name.into(),
        origin: sources::validate(Kind::Track, NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": song_id }) })?,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: Default::default(),
        local_path: Some(format!("/srv/music/{file_name}")),
        edited_after: false,
        tags_from: None,
        volume: None,
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file_name);
    let pending = ingest::receive_path(store, &path, "main").await?;
    let sha256 = pending.sha256.clone();
    let report = ingest::ingest_with_report(pool, store, pending, &options, false).await?;
    Ok((report.track_id.unwrap(), sha256))
}

async fn setup(connect: PgConnectOptions) -> anyhow::Result<Setup> {
    let owner = PgPoolOptions::new().connect_with(connect.clone()).await?;
    let pool = db::test_app_pool(connect).await?;
    sqlx::query("INSERT INTO group_permissions SELECT id, 'download_original' FROM groups WHERE name = 'user'")
        .execute(&owner)
        .await?;
    let admin = user(&owner, "alice", "admin").await?;
    let listener = user(&owner, "bob", "user").await?;

    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let (track, sha256) = ingest_file(&pool, &store, "01 - Glass Harbor.flac", 7).await?;
    let (other_track, other_sha256) = ingest_file(&pool, &store, "02 - Undertow.flac", 8).await?;

    let blob_changes = std::sync::Arc::new(api::mirror::BlobChanges::default());
    tokio::spawn(api::mirror::listen(pool.clone(), blob_changes.clone()));
    let app = api::router(AppState {
        uploads: Default::default(),
        blob_changes,
        health: Default::default(),
        together: Default::default(),
        pool,
        store,
        web_dir: dir.path().to_path_buf(),
            docs_dir: dir.path().join("docs"),
        secure_cookies: false,
        trust_proxy: false,
        rate_limits: std::sync::Arc::new(api::rate_limit::RateLimits::new()),
        images: std::sync::Arc::new(crate::storage::images::ImageCache::new(dir.path().join("cache"))),
        transcodes: std::sync::Arc::new(crate::storage::transcode::TranscodeCache::new(dir.path().join("cache"), vec![128], 0)),
        upload_limits: std::sync::Arc::new(api::upload_limits::UploadLimits::new(0, 0)),
    });
    // Let the listener connect before anything changes.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let original = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join("01 - Glass Harbor.flac"))?;
    Ok(Setup { owner, app, admin, listener, track, sha256, other_track, other_sha256, original, _dir: dir })
}

struct Answer {
    status: StatusCode,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

impl Setup {
    async fn get(&self, uri: &str, token: &str, range: Option<&str>) -> Answer {
        let mut request = Request::builder().uri(uri).header("authorization", format!("Bearer {token}"));
        request = request.extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
        if let Some(range) = range {
            request = request.header("range", range);
        }
        let response = self.app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 100_000_000).await.unwrap().to_vec();
        Answer { status, body }
    }

    async fn post(&self, uri: &str, body: Value) -> StatusCode {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", format!("Bearer {}", self.admin))
            .header("content-type", "application/json")
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))))
            .body(Body::from(body.to_string()))
            .unwrap();
        self.app.clone().oneshot(request).await.unwrap().status()
    }

    async fn list(&self, query: &str) -> Value {
        let answer = self.get(&format!("/api/mirror{query}"), &self.admin, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", String::from_utf8_lossy(&answer.body));
        answer.json()
    }

    /// The state of `sha256` in a listing, if it's in it.
    fn state_in(listing: &Value, sha256: &str) -> Option<String> {
        listing["blobs"].as_array().unwrap().iter().find(|b| b["sha256"] == sha256).map(|b| b["state"].as_str().unwrap().to_string())
    }
}

fn since(listing: &Value) -> String {
    format!("?since={}", listing["version"].as_str().unwrap())
}

#[sqlx::test]
async fn only_admins_mirror(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    assert_eq!(s.get("/api/mirror", &s.listener, None).await.status, StatusCode::FORBIDDEN);
    let blob = format!("/api/mirror/blobs/{}", s.sha256);
    assert_eq!(s.get(&blob, &s.listener, None).await.status, StatusCode::FORBIDDEN);
    Ok(())
}

#[sqlx::test]
async fn the_list_says_where_each_blob_is_and_what_its_for(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let listing = s.list("").await;
    assert_eq!(listing["full"], true);
    assert_eq!(listing["volumes"], json!([{ "name": "main", "default": true }]));
    let entry = listing["blobs"].as_array().unwrap().iter().find(|b| b["sha256"] == s.sha256).unwrap();
    assert_eq!(entry["volume"], "main");
    assert_eq!(entry["size"], s.original.len());
    assert_eq!(entry["state"], "audio");
    assert_eq!(entry["path"], "/srv/music/01 - Glass Harbor.flac", "where it was imported from");
    // Its embedded cover is stored too.
    assert!(listing["blobs"].as_array().unwrap().iter().any(|b| b["state"] == "cover"), "{listing}");
    Ok(())
}

#[sqlx::test]
async fn a_delta_has_what_changed_and_how_it_is_now(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let first = s.list("").await;

    let nothing = s.list(&since(&first)).await;
    assert_eq!((nothing["full"].clone(), nothing["blobs"].clone()), (json!(false), json!([])));

    // A stray blob (nothing refers to it), then moved to another volume.
    let stray = crate::storage::sha256_hex(b"stray");
    sqlx::query("INSERT INTO blobs (sha256, volume, size_bytes) VALUES ($1, 'main', 5)").bind(&stray).execute(&s.owner).await?;
    let second = s.list(&since(&first)).await;
    assert_eq!(second["blobs"], json!([{ "sha256": stray, "volume": "main", "size": 5, "path": null, "state": "other" }]));
    sqlx::query("UPDATE blobs SET volume = 'archive' WHERE sha256 = $1").bind(&stray).execute(&s.owner).await?;
    let third = s.list(&since(&second)).await;
    assert_eq!(third["blobs"][0]["volume"], "archive");

    // The track's file in the trash, then purged and its blob deleted: each a change.
    sqlx::query("UPDATE files SET trashed_at = now() WHERE sha256 = $1").bind(&s.sha256).execute(&s.owner).await?;
    let fourth = s.list(&since(&third)).await;
    assert_eq!(Setup::state_in(&fourth, &s.sha256).as_deref(), Some("trashed"));
    assert_eq!(fourth["blobs"].as_array().unwrap().len(), 1, "only what changed: {fourth}");
    sqlx::query("UPDATE files SET purged_at = now() WHERE sha256 = $1").bind(&s.sha256).execute(&s.owner).await?;
    sqlx::query("DELETE FROM blobs WHERE sha256 = $1").bind(&s.sha256).execute(&s.owner).await?;
    let fifth = s.list(&since(&fourth)).await;
    assert_eq!(fifth["blobs"], json!([{ "sha256": s.sha256, "volume": null, "size": null, "path": null, "state": "gone" }]));

    // Gone isn't in the whole list.
    assert_eq!(Setup::state_in(&s.list("").await, &s.sha256), None);

    let bad = s.get("/api/mirror?since=nope", &s.admin, None).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[sqlx::test]
async fn a_file_a_track_gave_up_is_a_previous_one(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let first = s.list("").await;
    let status = s.post(&format!("/api/tracks/{}/use-file", s.track), json!({ "file_of": s.other_track })).await;
    assert_eq!(status, StatusCode::OK);
    let delta = s.list(&since(&first)).await;
    assert_eq!(Setup::state_in(&delta, &s.sha256).as_deref(), Some("previous"));
    assert_eq!(Setup::state_in(&delta, &s.other_sha256).as_deref(), Some("audio"), "still played (by both tracks now)");
    Ok(())
}

#[sqlx::test]
async fn a_version_older_than_the_log_gets_everything(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let first = s.list("").await;
    sqlx::query("INSERT INTO blobs (sha256, volume) VALUES ($1, 'main')").bind(crate::storage::sha256_hex(b"x")).execute(&s.owner).await?;
    sqlx::query("UPDATE blob_changes SET at = now() - interval '100 days'").execute(&s.owner).await?;
    let mut conn = s.owner.acquire().await?;
    assert!(db::blobs::trim_changes(&mut conn, 90).await? > 0);
    let again = s.list(&since(&first)).await;
    assert_eq!(again["full"], true);
    assert_eq!(Setup::state_in(&again, &s.sha256).as_deref(), Some("audio"));
    Ok(())
}

#[sqlx::test]
async fn waiting_answers_as_soon_as_something_changes(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let first = s.list("").await;

    // Nothing changes: an empty delta once the time is up.
    let started = Instant::now();
    let quiet = s.list(&format!("{}&wait=1", since(&first))).await;
    assert_eq!(quiet["blobs"], json!([]));
    assert!(started.elapsed() >= Duration::from_millis(900));

    let owner = s.owner.clone();
    let stray = crate::storage::sha256_hex(b"new");
    let added = stray.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sqlx::query("INSERT INTO blobs (sha256, volume) VALUES ($1, 'main')").bind(&added).execute(&owner).await.unwrap();
    });
    let started = Instant::now();
    let woken = s.list(&format!("{}&wait=30", since(&first))).await;
    assert!(started.elapsed() < Duration::from_secs(10), "answered when it changed, not after 30 s");
    assert_eq!(Setup::state_in(&woken, &stray).as_deref(), Some("other"));
    Ok(())
}

#[sqlx::test]
async fn the_bytes_come_as_stored_also_in_parts(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let uri = format!("/api/mirror/blobs/{}", s.sha256);
    let whole = s.get(&uri, &s.admin, None).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(crate::storage::sha256_hex(&whole.body), s.sha256);

    let rest = s.get(&uri, &s.admin, Some("bytes=1000-")).await;
    assert_eq!(rest.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(rest.body, s.original[1000..]);

    assert_eq!(s.get("/api/mirror/blobs/abc", &s.admin, None).await.status, StatusCode::BAD_REQUEST);
    let missing = format!("/api/mirror/blobs/{}", crate::storage::sha256_hex(b"never stored"));
    assert_eq!(s.get(&missing, &s.admin, None).await.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[sqlx::test]
async fn the_catalog_answers_its_own_etag_with_304_until_something_changes(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let ask = |etag: Option<String>| {
        let mut request = Request::builder().uri("/api/catalog").header("authorization", format!("Bearer {}", s.admin));
        request = request.extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
        if let Some(etag) = etag {
            request = request.header("if-none-match", etag);
        }
        let app = s.app.clone();
        async move {
            let response = app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
            let etag = response.headers().get("etag").map(|v| v.to_str().unwrap().to_string());
            (response.status(), etag)
        }
    };
    let (status, etag) = ask(None).await;
    assert_eq!(status, StatusCode::OK);
    let etag = etag.expect("an ETag");
    assert_eq!(ask(Some(etag.clone())).await.0, StatusCode::NOT_MODIFIED);

    sqlx::query("UPDATE tracks SET title = 'Renamed' WHERE id = $1").bind(s.track).execute(&s.owner).await?;
    let (status, newer) = ask(Some(etag.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(newer.as_deref(), Some(etag.as_str()));
    assert_eq!(ask(newer).await.0, StatusCode::NOT_MODIFIED);
    // Something that isn't a version is no ETag of ours: the catalog comes.
    assert_eq!(ask(Some("\"abc\"".into())).await.0, StatusCode::OK);
    Ok(())
}
