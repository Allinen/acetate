//! Talking to an Acetate server: logging in (the session is kept in the app's data
//! folder, so the next start is already logged in), the catalog, covers and file bytes.
//!
//! `Api` is a cheap handle (clone it freely): the HTTP client and the session are shared
//! by all clones, so logging in or out is seen everywhere at once.
//!
//! Every request goes through `go`, so every failure is an `ApiError` whose text is a
//! sentence for a person: the server's own `message` when it answered, else ours (it
//! couldn't be reached, the connection dropped). A 401 on any request also marks the
//! session as ended, for the app to go back to the login screen.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, RwLock};

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

/// A request that failed. Shown as `message`, a sentence for a person.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// The HTTP status, if the server answered.
    pub status: Option<u16>,
    /// The server's error code (`not_found`, `stale`…), or ours: `unreachable`,
    /// `unexpected` (an answer this app can't read). For acting on (edit mode: offer to
    /// reload on `stale`), not for showing.
    pub code: String,
    pub message: String,
    /// `became_extra` (a track made into an extra): the extra.
    pub extra_id: Option<Uuid>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// The server's error body (`message` since the server writes them for people; older
/// servers only had `error`).
#[derive(Deserialize)]
struct ErrorBody {
    code: Option<String>,
    message: Option<String>,
    extra_id: Option<Uuid>,
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
    /// A request was refused as not logged in: the session ended.
    expired: Arc<AtomicBool>,
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
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("an HTTP client");
        Api {
            client,
            session_file: Arc::new(session_file),
            session: Arc::new(RwLock::new(session)),
            expired: Arc::default(),
        }
    }

    pub fn session(&self) -> Option<Session> {
        self.session.read().unwrap().clone()
    }

    /// Whether the session ended since last asked (a request got a 401).
    pub fn take_expired(&self) -> bool {
        self.expired.swap(false, Relaxed)
    }

    pub async fn login(&self, server: &str, username: &str, password: &str) -> Result<Session, ApiError> {
        let server = server.trim().trim_end_matches('/').to_string();
        let request = self
            .client
            .post(format!("{server}/api/auth/login"))
            .json(&serde_json::json!({ "username": username, "password": password }));
        #[derive(Deserialize)]
        struct Login {
            token: String,
        }
        let login: Login = read_json(self.go(request, &server).await?).await?;
        let session = Session { server, token: login.token, username: username.to_string() };
        let saved = self
            .session_file
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&*self.session_file, serde_json::to_vec(&session).unwrap_or_default()));
        if let Err(err) = saved {
            // Logged in all the same; only the next start asks again.
            eprintln!("couldn't keep the session: {err}");
        }
        *self.session.write().unwrap() = Some(session.clone());
        self.expired.store(false, Relaxed);
        Ok(session)
    }

    pub fn logout(&self) {
        *self.session.write().unwrap() = None;
        let _ = std::fs::remove_file(&*self.session_file);
    }

    /// A request to the server with the session's token, and the server's address.
    /// A path's full address and the header that logs in to it, for something that fetches
    /// it itself (the video player).
    pub fn address_and_auth(&self, path: &str) -> Option<(String, String)> {
        let session = self.session()?;
        Some((format!("{}{path}", session.server), format!("Authorization: Bearer {}", session.token)))
    }

    fn request(&self, method: reqwest::Method, path: &str) -> Result<(reqwest::RequestBuilder, String), ApiError> {
        let session = self.session().ok_or_else(|| ApiError {
            status: None,
            code: "unauthorized".into(),
            message: "You're not logged in.".into(),
            extra_id: None,
        })?;
        let request = self.client.request(method, format!("{}{path}", session.server)).bearer_auth(&session.token);
        Ok((request, session.server))
    }

    /// Sends a request: its response if it succeeded, else why not, in words.
    async fn go(&self, request: reqwest::RequestBuilder, server: &str) -> Result<reqwest::Response, ApiError> {
        let response = request.send().await.map_err(|err| unreachable(&err, server))?;
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(response);
        }
        let body: Option<ErrorBody> = response.json().await.ok();
        let (code, message, extra_id) = match body {
            Some(ErrorBody { code, message, extra_id }) => (code, message, extra_id),
            None => (None, None, None),
        };
        let code = code.unwrap_or_else(|| status.as_str().to_string());
        if status == reqwest::StatusCode::UNAUTHORIZED && code != "wrong_login" {
            self.expired.store(true, Relaxed);
        }
        let message = message.unwrap_or_else(|| format!("The server answered {status}."));
        Err(ApiError { status: Some(status.as_u16()), code, message, extra_id })
    }

    /// A request with an optional JSON body, answered with JSON.
    pub async fn send_json<T: DeserializeOwned>(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> Result<T, ApiError> {
        let (mut request, server) = self.request(method, path)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        read_json(self.go(request, &server).await?).await
    }

    /// A request with an optional JSON body, for endpoints that answer without one.
    pub async fn send(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> Result<(), ApiError> {
        let (mut request, server) = self.request(method, path)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        self.go(request, &server).await?;
        Ok(())
    }

    /// Uploads a file (one multipart field named "file"), answered with JSON: covers,
    /// extras' files. A big one is sent ahead in chunks (`/api/uploads`), under proxies'
    /// caps on a request's body (Cloudflare: 100 MB), and the form names it.
    pub async fn upload<T: DeserializeOwned>(&self, path: &str, bytes: Vec<u8>, file_name: &str) -> Result<T, ApiError> {
        const CHUNK_BYTES: usize = 50_000_000;
        let part = if bytes.len() > CHUNK_BYTES {
            #[derive(Deserialize)]
            struct Started {
                id: Uuid,
            }
            let upload = self.send_json::<Started>(reqwest::Method::POST, "/api/uploads", None).await?.id;
            for (n, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
                let (request, server) = self.request(reqwest::Method::PUT, &format!("/api/uploads/{upload}?offset={}", n * CHUNK_BYTES))?;
                self.go(request.body(chunk.to_vec()), &server).await?;
            }
            reqwest::multipart::Part::text(upload.to_string()).mime_str("application/vnd.acetate.upload").expect("a valid type")
        } else {
            reqwest::multipart::Part::bytes(bytes)
        };
        let (request, server) = self.request(reqwest::Method::POST, path)?;
        let request = request.multipart(reqwest::multipart::Form::new().part("file", part.file_name(file_name.to_string())));
        read_json(self.go(request, &server).await?).await
    }

    /// Stars or unstars a track, album or artist (`kind`: "tracks", "albums", "artists").
    pub async fn star(&self, kind: &str, id: Uuid, starred: bool) -> Result<Annotation, ApiError> {
        let method = if starred { reqwest::Method::POST } else { reqwest::Method::DELETE };
        self.send_json(method, &format!("/api/{kind}/{id}/star"), None).await
    }

    /// Rates it, 0 (not rated) to 10 (five stars).
    pub async fn rate(&self, kind: &str, id: Uuid, rating: i16) -> Result<Annotation, ApiError> {
        self.send_json(reqwest::Method::PUT, &format!("/api/{kind}/{id}/rating"), Some(serde_json::json!({ "rating": rating }))).await
    }

    /// Reports a listen (after half the track or 4 minutes).
    pub async fn played(&self, track: Uuid) -> Result<Annotation, ApiError> {
        self.send_json(reqwest::Method::POST, &format!("/api/tracks/{track}/played"), None).await
    }

    /// The library (`GET /api/catalog`). With the version of the copy we have (`since`),
    /// only what changed since (a delta); with its ETag, an unchanged library costs the
    /// server no body to send.
    pub async fn catalog(&self, etag: Option<&str>, since: Option<&str>) -> Result<CatalogResponse, ApiError> {
        let (mut request, server) = self.request(reqwest::Method::GET, "/api/catalog")?;
        if let Some(since) = since {
            request = request.query(&[("since", since)]);
        }
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }
        let response = self.go(request, &server).await?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(CatalogResponse::Unchanged);
        }
        let etag = response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = response.bytes().await.map_err(|err| unreachable(&err, &server))?;
        Ok(CatalogResponse::Changed { body: body.to_vec(), etag })
    }

    /// A cover image's bytes, by its content (`hash`: its SHA-256 or the start of it): at
    /// `size` ("small", "standard"), or the original (None).
    pub async fn image(&self, hash: &str, size: Option<&str>) -> Result<Vec<u8>, ApiError> {
        let path = match size {
            Some(size) => format!("/api/images/{hash}?size={size}"),
            None => format!("/api/images/{hash}"),
        };
        let (request, server) = self.request(reqwest::Method::GET, &path)?;
        let response = self.go(request, &server).await?;
        Ok(response.bytes().await.map_err(|err| unreachable(&err, &server))?.to_vec())
    }

    /// A file's bytes from `from` on (a Range request), as a response to read in chunks.
    #[cfg(test)]
    pub async fn file_from(&self, file_id: Uuid, from: u64) -> Result<reqwest::Response, ApiError> {
        self.stream_from(&format!("/api/files/{file_id}/stream"), from).await
    }

    /// Audio at `path` (a file's stream, a kept file that didn't import) from `from` on (a
    /// Range request), as a response to read in chunks.
    pub async fn stream_from(&self, path: &str, from: u64) -> Result<reqwest::Response, ApiError> {
        let (request, server) = self.request(reqwest::Method::GET, path)?;
        self.go(request.header(reqwest::header::RANGE, format!("bytes={from}-")), &server).await
    }
}

/// A successful answer's JSON; an answer this app can't read says so.
async fn read_json<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ApiError> {
    response.json().await.map_err(|err| ApiError {
        status: None,
        code: "unexpected".into(),
        message: format!("The server's answer isn't what this app expected. Is the app up to date? ({err})"),
        extra_id: None,
    })
}

/// A request that didn't get an answer, in words.
pub fn unreachable(err: &reqwest::Error, server: &str) -> ApiError {
    let message = if err.is_connect() {
        format!("Can't reach the server at {server}. Is it running, and is the address right?")
    } else if err.is_timeout() {
        format!("The server at {server} didn't answer in time.")
    } else if err.is_body() || err.is_decode() {
        "The connection to the server dropped. Try again.".to_string()
    } else {
        format!("Couldn't talk to the server at {server} ({err}).")
    };
    ApiError { status: None, code: "unreachable".into(), message, extra_id: None }
}
