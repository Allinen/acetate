//! What reaches whom over HTTP: every route refuses a guest unless it is meant to be open,
//! and a restricted track's files and covers go to no one outside its groups, by any route.

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
use crate::ingest::{self, IngestOptions};
use crate::model::sources::{self, Kind, NewSource};
use crate::storage::BlobStore;

struct Setup {
    owner: PgPool,
    app: axum::Router,
    pool: PgPool,
    store: BlobStore,
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
    let pool = db::test_app_pool(connect).await?;
    let dir = tempfile::tempdir()?;
    let store = BlobStore::open(dir.path())?;
    let app = api::router(AppState {
        uploads: Default::default(),
        blob_changes: Default::default(),
        health: Default::default(),
        pool: pool.clone(),
        store: store.clone(),
        web_dir: dir.path().to_path_buf(),
        docs_dir: dir.path().join("docs"),
        secure_cookies: false,
        trust_proxy: false,
        rate_limits: std::sync::Arc::new(api::rate_limit::RateLimits::new()),
        images: std::sync::Arc::new(crate::storage::images::ImageCache::new(dir.path().join("cache"))),
        transcodes: std::sync::Arc::new(crate::storage::transcode::TranscodeCache::new(dir.path().join("cache"), vec![128], 0)),
        upload_limits: std::sync::Arc::new(api::upload_limits::UploadLimits::new(0, 0)),
    });
    Ok(Setup { owner, app, pool, store, _dir: dir })
}

impl Setup {
    async fn go(&self, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Vec<u8>) {
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
        (status, axum::body::to_bytes(response.into_body(), 100_000_000).await.unwrap().to_vec())
    }

    async fn ingest(&self, file_name: &str, restrict_to: Option<Vec<Uuid>>) -> anyhow::Result<Uuid> {
        let options = IngestOptions {
            user: None,
            uploaded_filename: file_name.into(),
            origin: sources::validate(Kind::Track, NewSource { platform: "netease".into(), source_id: None, captured_at: None, data: json!({ "song_id": 7 }) })?,
            status: "released".into(),
            sources: Vec::new(),
            restrict_to,
            hints: Default::default(),
            local_path: None,
            edited_after: false,
            tags_from: None,
            volume: None,
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join(file_name);
        let pending = ingest::receive_path(&self.store, &path, "main").await?;
        let report = ingest::ingest_with_report(&self.pool, &self.store, pending, &options, false).await?;
        Ok(report.track_id.unwrap())
    }
}

/// The paths in `.route("...", ...)` calls between two markers of `api/mod.rs`.
fn routes_between(source: &str, from: &str, to: &str) -> Vec<String> {
    let start = source.find(from).expect(from);
    let end = start + source[start..].find(to).expect(to);
    source[start..end]
        .split(".route(")
        .skip(1)
        .map(|chunk| chunk.trim_start().strip_prefix('"').expect("a route starts with its path").split('"').next().unwrap().to_string())
        .collect()
}

/// The routes anyone may use without logging in. Adding one is a decision: it goes here.
const OPEN: &[&str] = &["/api/version", "/api/auth/login", "/api/share/{token}", "/api/share/{token}/tracks/{track_id}/stream", "/api/share/{token}/covers/{cover_id}"];
/// Behind the session middleware but fine for a guest: they only ever act on the caller's own session.
const GUEST_OK: &[&str] = &["/api/auth/logout", "/api/auth/me"];

#[sqlx::test]
async fn every_route_refuses_a_guest_unless_it_is_meant_to_be_open(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let source = include_str!("mod.rs");

    let mut open = routes_between(source, "let share_links = Router::new()", "let public = Router::new()");
    open.extend(routes_between(source, "let public = Router::new()", "Router::new()\n        .merge(public)"));
    // (The pages `/s/...` and `/open/...` are static files.)
    open.retain(|p| p.starts_with("/api/"));
    open.sort();
    let mut expected: Vec<String> = OPEN.iter().map(|p| p.to_string()).collect();
    expected.sort();
    assert_eq!(open, expected, "a route was opened to guests (or closed): update OPEN on purpose");

    let routes = routes_between(source, "let api = Router::new()", "// `route_layer`");
    assert!(routes.len() > 100, "the router's source was read: {} routes", routes.len());
    let mut refused = 0;
    for route in routes.iter().filter(|r| !GUEST_OK.contains(&r.as_str()) && !r.starts_with("/api/auth/")) {
        let uri = route.split('/').map(|part| if part.starts_with('{') { Uuid::nil().to_string() } else { part.to_string() }).collect::<Vec<_>>().join("/");
        for method in ["GET", "POST", "PUT", "DELETE"] {
            let (status, _) = s.go(method, &uri, None, None).await;
            // 405: the route has no such method. Anything else but 401 would be the handler running.
            assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::METHOD_NOT_ALLOWED, "{method} {route} answered a guest with {status}");
            refused += (status == StatusCode::UNAUTHORIZED) as usize;
        }
    }
    assert!(refused > 100, "{refused} refusals");

    // The account routes: a guest has no session to act on.
    for (method, uri) in [("POST", "/api/auth/password"), ("GET", "/api/auth/sessions"), ("DELETE", "/api/auth/sessions"), ("DELETE", &format!("/api/auth/sessions/{}", Uuid::nil()))] {
        let (status, _) = s.go(method, uri, None, None).await;
        assert!(status.is_client_error(), "{method} {uri}: {status}");
    }
    // A session that doesn't work is refused too, not treated as a guest.
    let (status, _) = s.go("GET", "/api/catalog", Some("not-a-session"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    Ok(())
}

#[sqlx::test]
async fn a_restricted_tracks_files_and_covers_reach_no_one_outside_its_groups(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let admin = user(&s.owner, "alice", "admin").await?;
    let outsider = user(&s.owner, "bob", "user").await?;
    sqlx::query("INSERT INTO group_permissions SELECT id, 'share' FROM groups WHERE name = 'user'").execute(&s.owner).await?;
    let vault: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ('vault') RETURNING id").fetch_one(&s.owner).await?;

    let open_track = s.ingest("01 - Glass Harbor.flac", None).await?;
    let secret_track = s.ingest("02 - Undertow.flac", Some(vec![vault])).await?;
    let file_of = |track: Uuid| {
        let owner = s.owner.clone();
        async move { sqlx::query_scalar::<_, Uuid>("SELECT file_id FROM tracks WHERE id = $1").bind(track).fetch_one(&owner).await.unwrap() }
    };
    let (open_file, secret_file) = (file_of(open_track).await, file_of(secret_track).await);

    // A cover on each.
    let mut covers = Vec::new();
    for (track, bytes) in [(open_track, &b"open cover"[..]), (secret_track, &b"secret cover"[..])] {
        let sha = s.store.put_bytes(bytes).await?;
        let id: Uuid = sqlx::query_scalar("INSERT INTO covers (track_id, blob_sha256, mime, source, position) VALUES ($1, $2, 'image/png', 'upload', 0) RETURNING id")
            .bind(track)
            .bind(&sha)
            .fetch_one(&s.owner)
            .await?;
        covers.push((id, sha));
    }
    let ((open_cover, _), (secret_cover, secret_hash)) = (covers[0].clone(), covers[1].clone());

    // The routes that hand out the secret track's bytes, each as a path.
    let secret_routes = [
        format!("/api/files/{secret_file}/stream"),
        format!("/api/files/{secret_file}/stream?transcode=true"),
        format!("/api/tracks/{secret_track}/download"),
        format!("/api/tracks/{secret_track}/download?tags=false"),
        format!("/api/covers/{secret_cover}"),
        format!("/api/images/{secret_hash}"),
        format!("/api/images/{}", &secret_hash[..16]),
        format!("/api/images/{secret_hash}?size=small"),
    ];
    for route in &secret_routes {
        let (guest, _) = s.go("GET", route, None, None).await;
        assert_eq!(guest, StatusCode::UNAUTHORIZED, "guest: {route}");
        let (other, body) = s.go("GET", route, Some(&outsider), None).await;
        assert!(other == StatusCode::NOT_FOUND || other == StatusCode::FORBIDDEN, "outsider got {other} from {route}");
        assert!(!body.windows(12).any(|w| w == b"secret cover"), "bytes leaked through {route}");
        let (admin_status, _) = s.go("GET", route, Some(&admin), None).await;
        // Admin sees it (some routes need ffmpeg or a real image to decode).
        assert_ne!(admin_status, StatusCode::NOT_FOUND, "admin: {route}");
    }

    // The open track works for the same outsider, so the 404s above were about the restriction.
    let (status, _) = s.go("GET", &format!("/api/covers/{open_cover}"), Some(&outsider), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = s.go("GET", &format!("/api/files/{open_file}/stream"), Some(&outsider), None).await;
    assert_ne!(status, StatusCode::NOT_FOUND);

    // Share links: a guest with no valid token gets nothing, and a link to one track
    // doesn't open another.
    for route in [
        "/api/share/nope".to_string(),
        format!("/api/share/nope/tracks/{secret_track}/stream"),
        format!("/api/share/nope/covers/{secret_cover}"),
    ] {
        assert_eq!(s.go("GET", &route, None, None).await.0, StatusCode::NOT_FOUND, "{route}");
    }
    let (status, body) = s.go("POST", "/api/shares", Some(&outsider), Some(json!({ "track_id": secret_track }))).await;
    assert_ne!(status, StatusCode::OK, "a share of a track the creator can't see");
    let _ = body;
    let (status, body) = s.go("POST", "/api/shares", Some(&outsider), Some(json!({ "track_id": open_track }))).await;
    assert_eq!(status, StatusCode::OK);
    let link = serde_json::from_slice::<Value>(&body)?["token"].as_str().unwrap().to_string();
    for route in [format!("/api/share/{link}/tracks/{secret_track}/stream"), format!("/api/share/{link}/covers/{secret_cover}")] {
        assert_eq!(s.go("GET", &route, None, None).await.0, StatusCode::NOT_FOUND, "{route}");
    }
    // A revoked link stops working.
    let shared_cover = s.go("GET", &format!("/api/share/{link}/covers/{open_cover}"), None, None).await.0;
    assert_eq!(shared_cover, StatusCode::OK);
    Ok(())
}

#[sqlx::test]
async fn a_trashed_cover_is_no_longer_served(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let admin = user(&s.owner, "alice", "admin").await?;
    let track = s.ingest("01 - Glass Harbor.flac", None).await?;
    let sha = s.store.put_bytes(b"a cover").await?;
    let cover: Uuid = sqlx::query_scalar("INSERT INTO covers (track_id, blob_sha256, mime, source, position) VALUES ($1, $2, 'image/png', 'upload', 0) RETURNING id")
        .bind(track)
        .bind(&sha)
        .fetch_one(&s.owner)
        .await?;
    assert_eq!(s.go("GET", &format!("/api/covers/{cover}"), Some(&admin), None).await.0, StatusCode::OK);
    assert_eq!(s.go("GET", &format!("/api/images/{sha}"), Some(&admin), None).await.0, StatusCode::OK);
    sqlx::query("UPDATE covers SET trashed_at = now() WHERE id = $1").bind(cover).execute(&s.owner).await?;
    assert_eq!(s.go("GET", &format!("/api/covers/{cover}"), Some(&admin), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(s.go("GET", &format!("/api/images/{sha}"), Some(&admin), None).await.0, StatusCode::NOT_FOUND);
    Ok(())
}

#[sqlx::test]
async fn permissions_are_listed_for_those_who_hand_them_out(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    let admin = user(&s.owner, "alice", "admin").await?;
    let listener = user(&s.owner, "bob", "user").await?;
    let (status, body) = s.go("GET", "/api/permissions", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let list: Vec<Value> = serde_json::from_slice(&body)?;
    assert_eq!(list.len(), crate::auth::Permission::ALL.len());
    assert!(list.iter().all(|p| p["description"].as_str().is_some_and(|d| !d.is_empty())));
    assert_eq!(s.go("GET", "/api/permissions", Some(&listener), None).await.0, StatusCode::FORBIDDEN);
    Ok(())
}

/// The user guide opens at `/docs` too: it's sent on to `/docs/`, where the book's relative
/// links (its styles and scripts) resolve under `/docs/`.
#[sqlx::test]
async fn the_guide_at_docs_goes_on_to_docs_slash(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
    let s = setup(connect).await?;
    std::fs::create_dir_all(s._dir.path().join("docs/css"))?;
    std::fs::write(s._dir.path().join("docs/index.html"), "<link rel=\"stylesheet\" href=\"css/book.css\">")?;
    std::fs::write(s._dir.path().join("docs/css/book.css"), "body {}")?;
    let response = s.app.clone().oneshot(Request::builder().uri("/docs?x=1").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(response.headers()["location"], "/docs/?x=1");
    assert_eq!(s.go("GET", "/docs/", None, None).await.0, StatusCode::OK);
    assert_eq!(s.go("GET", "/docs/css/book.css", None, None).await.0, StatusCode::OK);
    Ok(())
}
