//! Who gets the original and who a transcoded copy: streaming, share links, and downloads
//! with the library's tags. With "01 - Glass Harbor.flac" from `test-audio/`. (Copies need
//! ffmpeg; without it those checks are skipped.)

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use crate::api::{self, AppState};
use crate::db;
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource};
use crate::storage::BlobStore;

struct Setup {
    owner: PgPool,
    app: axum::Router,
    /// Has every permission.
    admin: String,
    /// Has `stream` and `share`, not `download_original`.
    listener: String,
    track: Uuid,
    file: Uuid,
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

async fn setup(connect: PgConnectOptions) -> anyhow::Result<Setup> {
    let owner = PgPoolOptions::new().connect_with(connect.clone()).await?;
    let pool = db::app_pool_options().connect_with(connect).await?;
    sqlx::query("INSERT INTO group_permissions SELECT id, 'share' FROM groups WHERE name = 'user'").execute(&owner).await?;
    let admin = user(&owner, "alice", "admin").await?;
    let listener = user(&owner, "bob", "user").await?;

    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let file_name = "01 - Glass Harbor.flac";
    let options = IngestOptions {
        user: None,
        uploaded_filename: file_name.into(),
        origin: sources::validate(Kind::Track, NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": 7 }) })?,
        status: "released".into(),
        sources: Vec::new(),
        restrict_to: None,
        hints: Default::default(),
        local_path: None,
        edited_after: false,
        tags_from: None,
        volume: None,
    };
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file_name);
    let pending = ingest::receive_path(&store, &path, "main").await?;
    let report = ingest::ingest_with_report(&pool, &store, pending, &options, false).await?;
    let track = report.track_id.unwrap();
    let file = sqlx::query_scalar("SELECT file_id FROM tracks WHERE id = $1").bind(track).fetch_one(&owner).await?;
    let app = api::router(AppState {
        uploads: Default::default(),
        pool,
        store,
        web_dir: dir.path().to_path_buf(),
        secure_cookies: false,
        trust_proxy: false,
        rate_limits: std::sync::Arc::new(api::rate_limit::RateLimits::new()),
        images: std::sync::Arc::new(crate::storage::images::ImageCache::new(dir.path().join("cache"))),
        transcodes: std::sync::Arc::new(crate::storage::transcode::TranscodeCache::new(dir.path().join("cache"), 128, 0)),
        upload_limits: std::sync::Arc::new(api::upload_limits::UploadLimits::new(0, 0)),
    });
    Ok(Setup { owner, app, admin, listener, track, file, original: std::fs::read(path)?, _dir: dir })
}

struct Answer {
    status: StatusCode,
    disposition: Option<String>,
    body: Vec<u8>,
}

impl Setup {
    async fn go(&self, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Answer {
        let mut request = Request::builder().method(method).uri(uri);
        request = request.extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 9))));
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request.header("content-type", "application/json").body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let disposition = response.headers().get("content-disposition").map(|v| v.to_str().unwrap().to_string());
        let body = axum::body::to_bytes(response.into_body(), 100_000_000).await.unwrap().to_vec();
        Answer { status, disposition, body }
    }

    async fn stream(&self, token: &str, query: &str) -> Answer {
        self.go("GET", &format!("/api/files/{}/stream{query}", self.file), Some(token), None).await
    }

    async fn share(&self, token: &str, original: bool) -> Answer {
        self.go("POST", "/api/shares", Some(token), Some(json!({ "track_id": self.track, "original": original }))).await
    }

    /// The share's stream, opened without logging in.
    async fn open_share(&self, link: &Answer) -> Answer {
        let token = serde_json::from_slice::<Value>(&link.body).unwrap()["token"].as_str().unwrap().to_string();
        self.go("GET", &format!("/api/share/{token}/tracks/{}/stream", self.track), None, None).await
    }
}

fn is_opus(answer: &Answer) -> bool {
    answer.status == StatusCode::OK && answer.body.starts_with(b"OggS") && answer.body.windows(8).any(|w| w == b"OpusHead")
}

/// Whether the server could make copies (ffmpeg installed): a 503 says it couldn't.
fn can_copy(answer: &Answer) -> bool {
    answer.status != StatusCode::SERVICE_UNAVAILABLE
}

#[sqlx::test]
async fn streaming_gives_the_original_only_to_those_allowed_it(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;

    let admin = s.stream(&s.admin, "").await;
    assert_eq!((admin.status, admin.body == s.original), (StatusCode::OK, true), "download_original: the stored bytes");

    let listener = s.stream(&s.listener, "").await;
    if !can_copy(&listener) {
        return Ok(()); // no ffmpeg here
    }
    assert!(is_opus(&listener), "without it: a transcoded copy");
    // The original is not available by asking for it, however it's asked.
    assert!(is_opus(&s.stream(&s.listener, "?transcode=false").await));

    assert!(is_opus(&s.stream(&s.admin, "?transcode=true").await), "asked for a copy");
    Ok(())
}

#[sqlx::test]
async fn share_links_play_copies_unless_the_original_is_given(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;

    // Giving the original needs the permission to download it.
    assert_eq!(s.share(&s.listener, true).await.status, StatusCode::FORBIDDEN);

    let copy_link = s.share(&s.listener, false).await;
    assert_eq!(copy_link.status, StatusCode::OK);
    let opened = s.open_share(&copy_link).await;
    if can_copy(&opened) {
        assert!(is_opus(&opened), "a share plays a copy by default: {} {}", opened.status, String::from_utf8_lossy(&opened.body[..opened.body.len().min(300)]));
    }

    let original_link = s.share(&s.admin, true).await;
    assert_eq!(original_link.status, StatusCode::OK);
    assert!(s.open_share(&original_link).await.body == s.original, "or the original, if its creator gave it");

    // A share made without the original doesn't become one for an admin.
    let by_admin = s.share(&s.admin, false).await;
    let opened = s.open_share(&by_admin).await;
    if can_copy(&opened) {
        assert!(is_opus(&opened));
    }

    // The creator losing the permission to give it turns the link back to copies.
    sqlx::query("INSERT INTO groups (name) VALUES ('curator')").execute(&s.owner).await?;
    sqlx::query("INSERT INTO group_permissions SELECT id, p FROM groups, unnest(ARRAY['stream', 'share', 'download_original']) p WHERE name = 'curator'")
        .execute(&s.owner)
        .await?;
    let curator = user(&s.owner, "dave", "curator").await?;
    let link = s.share(&curator, true).await;
    assert_eq!(link.status, StatusCode::OK);
    assert!(s.open_share(&link).await.body == s.original);
    sqlx::query("DELETE FROM group_permissions WHERE permission = 'download_original' AND group_id = (SELECT id FROM groups WHERE name = 'curator')")
        .execute(&s.owner)
        .await?;
    let opened = s.open_share(&link).await;
    if can_copy(&opened) {
        assert!(is_opus(&opened));
    }
    Ok(())
}

#[sqlx::test]
async fn downloads_carry_the_current_tags(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    use lofty::file::TaggedFileExt;
    use lofty::tag::Accessor;

    let s = setup(connect).await?;
    let uri = format!("/api/tracks/{}/download", s.track);
    assert_eq!(s.go("GET", &uri, Some(&s.listener), None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(s.go("GET", &uri, None, None).await.status, StatusCode::UNAUTHORIZED);

    let tagged = s.go("GET", &uri, Some(&s.admin), None).await;
    assert_eq!(tagged.status, StatusCode::OK, "{}", String::from_utf8_lossy(&tagged.body));
    assert!(tagged.disposition.as_deref().is_some_and(|d| d.starts_with("attachment; filename*=UTF-8''") && d.ends_with(".flac")), "{:?}", tagged.disposition);
    let file = tempfile::Builder::new().suffix(".flac").tempfile()?;
    std::fs::write(file.path(), &tagged.body)?;
    let read = lofty::read_from_path(file.path())?;
    let title: String = sqlx::query_scalar("SELECT title FROM tracks WHERE id = $1").bind(s.track).fetch_one(&s.owner).await?;
    let artist: String = sqlx::query_scalar("SELECT artist_display FROM tracks WHERE id = $1").bind(s.track).fetch_one(&s.owner).await?;
    let tag = read.primary_tag().unwrap();
    assert_eq!((tag.title().as_deref(), tag.artist().as_deref()), (Some(title.as_str()), Some(artist.as_str())));
    assert_eq!(tagged.disposition.as_deref().map(|d| d.contains("%20-%20")), Some(true));

    let untouched = s.go("GET", &format!("{uri}?tags=false"), Some(&s.admin), None).await;
    assert_eq!(untouched.body, s.original, "?tags=false: the stored bytes");
    Ok(())
}
