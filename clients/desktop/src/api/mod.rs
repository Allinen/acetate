//! Talking to an Acetate server: logging in (the session is kept in the app's data
//! folder, so the next start is already logged in), the catalog, covers and file bytes.
//!
//! `Api` is a cheap handle (clone it freely): the HTTP client and the session are shared
//! by all clones, so logging in or out is seen everywhere at once.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where to find the server, and the session token for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub server: String,
    pub token: String,
    pub username: String,
}

/// The server's error body: `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// The user's own plays, rating and star on something, as the server answers a change.
#[derive(Debug, Clone, Deserialize)]
pub struct Annotation {
    pub play_count: i32,
    pub rating: i16,
    pub starred: bool,
}

/// What `GET /api/catalog` answered.
pub enum CatalogResponse {
    /// The same as the copy we have (its ETag matched).
    Unchanged,
    Changed { body: Vec<u8>, etag: Option<String> },
}

#[derive(Clone)]
pub struct Api {
    client: reqwest::Client,
    session_file: Arc<PathBuf>,
    session: Arc<RwLock<Option<Session>>>,
}

impl Api {
    /// `data_dir`: the app's own folder, where the session is kept between runs.
    pub fn new(data_dir: PathBuf) -> Api {
        let session_file = data_dir.join("session.json");
        let session = std::fs::read(&session_file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let client = reqwest::Client::builder()
            // The catalog comes compressed (tens of MB otherwise).
            .zstd(true)
            .gzip(true)
            .build()
            .expect("an HTTP client");
        Api { client, session_file: Arc::new(session_file), session: Arc::new(RwLock::new(session)) }
    }

    pub fn session(&self) -> Option<Session> {
        self.session.read().unwrap().clone()
    }

    fn current(&self) -> anyhow::Result<Session> {
        self.session().context("not logged in")
    }

    pub async fn login(&self, server: &str, username: &str, password: &str) -> anyhow::Result<Session> {
        let server = server.trim().trim_end_matches('/').to_string();
        let response = self
            .client
            .post(format!("{server}/api/auth/login"))
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .with_context(|| format!("can't reach {server}"))?;
        let response = check(response).await.context("login failed")?;
        #[derive(Deserialize)]
        struct Login {
            token: String,
        }
        let login: Login = response.json().await?;
        let session = Session { server, token: login.token, username: username.to_string() };
        if let Some(dir) = self.session_file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&*self.session_file, serde_json::to_vec(&session)?)?;
        *self.session.write().unwrap() = Some(session.clone());
        Ok(session)
    }

    pub fn logout(&self) {
        *self.session.write().unwrap() = None;
        let _ = std::fs::remove_file(&*self.session_file);
    }

    /// A request to the server with the session's token.
    fn request(&self, method: reqwest::Method, path: &str) -> anyhow::Result<reqwest::RequestBuilder> {
        let session = self.current()?;
        Ok(self.client.request(method, format!("{}{path}", session.server)).bearer_auth(&session.token))
    }

    /// A request with an optional JSON body, answered with JSON.
    pub async fn send_json<T: DeserializeOwned>(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> anyhow::Result<T> {
        let mut request = self.request(method, path)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        Ok(check(response).await?.json().await?)
    }

    /// A request with an optional JSON body, for endpoints that answer without one.
    pub async fn send(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> anyhow::Result<()> {
        let mut request = self.request(method, path)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        check(request.send().await?).await?;
        Ok(())
    }

    /// Stars or unstars a track, album or artist (`kind`: "tracks", "albums", "artists").
    pub async fn star(&self, kind: &str, id: Uuid, starred: bool) -> anyhow::Result<Annotation> {
        let method = if starred { reqwest::Method::POST } else { reqwest::Method::DELETE };
        self.send_json(method, &format!("/api/{kind}/{id}/star"), None).await
    }

    /// Rates it, 0 (not rated) to 10 (five stars).
    pub async fn rate(&self, kind: &str, id: Uuid, rating: i16) -> anyhow::Result<Annotation> {
        self.send_json(reqwest::Method::PUT, &format!("/api/{kind}/{id}/rating"), Some(serde_json::json!({ "rating": rating }))).await
    }

    /// Reports a listen (after half the track or 4 minutes).
    pub async fn played(&self, track: Uuid) -> anyhow::Result<Annotation> {
        self.send_json(reqwest::Method::POST, &format!("/api/tracks/{track}/played"), None).await
    }

    /// The whole library (`GET /api/catalog`). With the ETag of the copy we have, an
    /// unchanged library costs the server no body to send.
    pub async fn catalog(&self, etag: Option<&str>) -> anyhow::Result<CatalogResponse> {
        let mut request = self.request(reqwest::Method::GET, "/api/catalog")?;
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(CatalogResponse::Unchanged);
        }
        let response = check(response).await?;
        let etag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok(CatalogResponse::Changed { body: response.bytes().await?.to_vec(), etag })
    }

    /// A cover's image bytes.
    pub async fn cover(&self, cover_id: Uuid) -> anyhow::Result<Vec<u8>> {
        let response = self.request(reqwest::Method::GET, &format!("/api/covers/{cover_id}"))?.send().await?;
        Ok(check(response).await?.bytes().await?.to_vec())
    }

    /// A file's bytes from `from` on (a Range request), as a response to read in chunks.
    pub async fn file_from(&self, file_id: Uuid, from: u64) -> anyhow::Result<reqwest::Response> {
        let request = self
            .request(reqwest::Method::GET, &format!("/api/files/{file_id}/stream"))?
            .header(reqwest::header::RANGE, format!("bytes={from}-"));
        check(request.send().await?).await
    }
}

/// Turns an error status into an error with the server's message.
async fn check(response: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let message = response.json::<ErrorBody>().await.map(|b| b.error).unwrap_or_default();
    anyhow::bail!("{status}: {message}")
}
