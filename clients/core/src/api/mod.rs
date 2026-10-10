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
    /// Which server it is, whatever its address (the server's `instance`): what's kept on
    /// disk is kept under this, so the address can change. None in a session made before
    /// servers had one (`Api::learn_instance` fills it in).
    #[serde(default)]
    pub instance: Option<Uuid>,
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

/// What `Api::check_address` found at an address.
#[derive(Debug, Clone, PartialEq)]
pub enum AddressCheck {
    /// This session's server: the same server, and the login works there.
    Same,
    /// Another server.
    Different,
    /// This server, but the login isn't accepted there (a backup restored from before it,
    /// say).
    TokenRefused,
    /// Nothing answered; why.
    NotReached(String),
    /// Something answered (with this status), but not like an Acetate server.
    NotAcetate(u16),
    /// Which server this session is of isn't known yet, and the current address doesn't
    /// answer to say.
    UnknownYet,
}

impl std::fmt::Display for AddressCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressCheck::Same => f.write_str("It's the same server."),
            AddressCheck::Different => f.write_str("That is a different server. To use it, log out and log in to it."),
            AddressCheck::TokenRefused => f.write_str("That is the same server, but it doesn't accept this login (a backup from before it, perhaps). Log out and log in again there."),
            AddressCheck::NotReached(why) => write!(f, "Nothing answered there: {why}"),
            AddressCheck::NotAcetate(status) => write!(f, "Something answered there ({status}), but it isn't an Acetate server."),
            AddressCheck::UnknownYet => f.write_str("Can't tell yet which server this is: the current address doesn't answer. Try again when it does."),
        }
    }
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

    /// Whether a server answers at `address` (no login): the HTTP status it gave, or why
    /// the connection failed (a certificate the phone doesn't trust, say), with the whole
    /// chain of causes. For finding out what's wrong with an address.
    pub async fn reach(&self, address: &str) -> Result<u16, String> {
        match self.client.get(address.trim()).send().await {
            Ok(response) => Ok(response.status().as_u16()),
            Err(err) => Err(chain(&err)),
        }
    }

    /// Whether `address` is where this session's server is now: asks it who the token
    /// belongs to, and which server it is. Changes nothing, and a refusal doesn't end the
    /// session.
    pub async fn check_address(&self, address: &str) -> AddressCheck {
        if self.session().is_some_and(|s| s.instance.is_none()) {
            self.learn_instance().await;
        }
        let Some(Session { token, username: mine, instance: Some(instance), .. }) = self.session() else {
            return AddressCheck::UnknownYet;
        };
        let address = address.trim().trim_end_matches('/');
        let response = match self.client.get(format!("{address}/api/auth/me")).bearer_auth(token).send().await {
            Ok(response) => response,
            Err(err) => return AddressCheck::NotReached(chain(&err)),
        };
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return AddressCheck::TokenRefused;
        }
        #[derive(Deserialize)]
        struct Me {
            instance: Option<Uuid>,
            username: Option<String>,
        }
        let me = if status.is_success() { response.json::<Me>().await.ok() } else { None };
        match me {
            Some(Me { instance: Some(there), .. }) if there != instance => AddressCheck::Different,
            Some(Me { instance: Some(_), username }) if username.as_deref() == Some(mine.as_str()) => AddressCheck::Same,
            Some(Me { instance: Some(_), .. }) => AddressCheck::TokenRefused,
            _ => AddressCheck::NotAcetate(status.as_u16()),
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
            instance: Option<Uuid>,
        }
        let login: Login = read_json(self.go(request, &server).await?).await?;
        let session = Session { server, token: login.token, username: username.to_string(), instance: login.instance };
        if session.instance.is_some() {
            // What an earlier version kept under the address is this server's too.
            self.adopt_kept(&Session { instance: None, ..session.clone() }, &session);
        }
        self.keep(&session);
        *self.session.write().unwrap() = Some(session.clone());
        self.expired.store(false, Relaxed);
        Ok(session)
    }

    /// Writes the session to its file (best effort: logged in all the same; only the next
    /// start asks again).
    fn keep(&self, session: &Session) {
        let saved = self
            .session_file
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&*self.session_file, serde_json::to_vec(session).unwrap_or_default()));
        if let Err(err) = saved {
            eprintln!("couldn't keep the session: {err}");
        }
    }

    /// What's kept in the app's folder under `from`'s key is now kept under `to`'s.
    fn adopt_kept(&self, from: &Session, to: &Session) {
        if let Some(dir) = self.session_file.parent() {
            crate::catalog::store::move_kept(dir, &crate::catalog::store::key(from), &crate::catalog::store::key(to));
        }
    }

    /// For a session made before servers had an identity: asks the server which it is, and
    /// from then on what's kept is kept under that, not the address (what is already kept
    /// under the address moves). Without an answer (the server's away) nothing changes and
    /// the next call tries again; call it before opening what's kept.
    pub async fn learn_instance(&self) {
        let Some(session) = self.session().filter(|s| s.instance.is_none()) else { return };
        #[derive(Deserialize)]
        struct Me {
            instance: Option<Uuid>,
        }
        let Ok(me) = self.send_json::<Me>(reqwest::Method::GET, "/api/auth/me", None).await else { return };
        let Some(instance) = me.instance else { return };
        let learned = Session { instance: Some(instance), ..session.clone() };
        self.adopt_kept(&session, &learned);
        self.keep(&learned);
        *self.session.write().unwrap() = Some(learned);
    }

    /// Moves the session to `address`, if the server there is this one (`check_address`
    /// again, so a stale check changes nothing). Nothing kept is touched: it's kept under
    /// the server's identity.
    pub async fn change_address(&self, address: &str) -> Result<(), String> {
        match self.check_address(address).await {
            AddressCheck::Same => {}
            other => return Err(other.to_string()),
        }
        let Some(session) = self.session() else { return Err("You're not logged in.".into()) };
        let moved = Session { server: address.trim().trim_end_matches('/').to_string(), ..session };
        self.keep(&moved);
        *self.session.write().unwrap() = Some(moved);
        Ok(())
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

    /// Makes a link anyone can listen with (`POST /api/shares`) to a track or album, which
    /// stops working after `days` (None: never). The link's address.
    pub async fn share(&self, target: crate::links::Target, days: Option<u32>) -> Result<String, ApiError> {
        use crate::links::Target;
        let mut body = match target {
            Target::Track(id) => serde_json::json!({ "track_id": id }),
            Target::Album(id) => serde_json::json!({ "album_id": id }),
            Target::Artist(_) => {
                return Err(ApiError { status: None, code: "unexpected".into(), message: "an artist can't be shared this way".into(), extra_id: None });
            }
        };
        if let Some(days) = days {
            body["expires_at"] = crate::links::expires_at(days).into();
        }
        #[derive(Deserialize)]
        struct Created {
            token: String,
        }
        let created: Created = self.send_json(reqwest::Method::POST, "/api/shares", Some(body)).await?;
        let server = self.session().map(|s| s.server).unwrap_or_default();
        Ok(format!("{}/s/{}", server.trim_end_matches('/'), created.token))
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
    #[cfg(any(test, feature = "test-support"))]
    pub async fn file_from(&self, file_id: Uuid, from: u64) -> Result<reqwest::Response, ApiError> {
        self.stream_from(&format!("/api/files/{file_id}/stream"), from).await
    }

    /// The server's blobs for a mirror (admins): all, or what changed since `since`;
    /// `wait`: seconds to wait for a change if nothing changed yet (see `mirror`).
    pub async fn mirror_list(&self, since: Option<&str>, wait: Option<u64>) -> Result<MirrorListing, ApiError> {
        let (mut request, server) = self.request(reqwest::Method::GET, "/api/mirror")?;
        if let Some(since) = since {
            request = request.query(&[("since", since)]);
        }
        if let Some(wait) = wait {
            request = request.query(&[("wait", wait)]);
        }
        read_json(self.go(request, &server).await?).await
    }

    /// Audio at `path` (a file's stream, a kept file that didn't import) from `from` on (a
    /// Range request), as a response to read in chunks.
    pub async fn stream_from(&self, path: &str, from: u64) -> Result<reqwest::Response, ApiError> {
        let (request, server) = self.request(reqwest::Method::GET, path)?;
        self.go(request.header(reqwest::header::RANGE, format!("bytes={from}-")), &server).await
    }
}

/// What `GET /api/mirror` answers.
#[derive(Debug, Deserialize)]
pub struct MirrorListing {
    pub version: String,
    /// The whole list (else what changed).
    pub full: bool,
    pub volumes: Vec<ServerVolume>,
    pub blobs: Vec<Listed>,
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

/// An error with the whole chain of its causes, as one line.
fn chain(err: &reqwest::Error) -> String {
    let mut text = err.to_string();
    let mut cause = std::error::Error::source(err);
    while let Some(c) = cause {
        text.push_str(&format!(": {c}"));
        cause = c.source();
    }
    text
}

/// A request that didn't get an answer, in words.
fn unreachable(err: &reqwest::Error, server: &str) -> ApiError {
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

/// A blob as a server lists it (`GET /api/mirror`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Listed {
    pub sha256: String,
    pub volume: Option<String>,
    pub size: Option<i64>,
    /// Where the server imported it from, if known.
    #[serde(default)]
    pub path: Option<String>,
    pub state: String,
}

/// One of the server's volumes, as listed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerVolume {
    pub name: String,
    pub default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that answers every request with `status` and `body`.
    fn answering(status: &'static str, body: String) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                    line.clear();
                }
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        address
    }

    fn api_of(instance: Uuid, server: &str) -> (tempfile::TempDir, Api) {
        let dir = tempfile::tempdir().unwrap();
        let session = Session { server: server.into(), token: "t".into(), username: "ann".into(), instance: Some(instance) };
        std::fs::write(dir.path().join("session.json"), serde_json::to_vec(&session).unwrap()).unwrap();
        let api = Api::new(dir.path().to_path_buf());
        (dir, api)
    }

    fn check(instance: Uuid, status: &'static str, body: serde_json::Value) -> (AddressCheck, Api, tempfile::TempDir) {
        let (dir, api) = api_of(instance, "http://old.example");
        let there = answering(status, body.to_string());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let found = runtime.block_on(api.check_address(&there));
        (found, api, dir)
    }

    #[test]
    fn an_address_is_only_taken_if_the_same_server_accepts_the_login() {
        let id = Uuid::new_v4();
        let me = |instance: Uuid, username: serde_json::Value| serde_json::json!({ "instance": instance, "username": username, "permissions": [] });
        assert_eq!(check(id, "200 OK", me(id, "ann".into())).0, AddressCheck::Same);
        assert_eq!(check(id, "200 OK", me(Uuid::new_v4(), "ann".into())).0, AddressCheck::Different);
        // The same server, but it knows the login as nobody (a guest) or as someone else.
        assert_eq!(check(id, "200 OK", me(id, serde_json::Value::Null)).0, AddressCheck::TokenRefused);
        assert_eq!(check(id, "200 OK", me(id, "bo".into())).0, AddressCheck::TokenRefused);
        assert_eq!(check(id, "401 Unauthorized", serde_json::json!({})).0, AddressCheck::TokenRefused);
        assert_eq!(check(id, "404 Not Found", serde_json::json!({})).0, AddressCheck::NotAcetate(404));
        assert_eq!(check(id, "200 OK", serde_json::json!({ "hello": 1 })).0, AddressCheck::NotAcetate(200));
        // A refusal isn't the session ending.
        let (_, api, _dir) = check(id, "401 Unauthorized", serde_json::json!({}));
        assert!(!api.take_expired());
    }

    #[test]
    fn changing_the_address_changes_only_the_address() {
        let id = Uuid::new_v4();
        let (dir, api) = api_of(id, "http://old.example");
        let there = answering("200 OK", serde_json::json!({ "instance": id, "username": "ann", "permissions": [] }).to_string());
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(api.change_address(&format!("{there}/"))).unwrap();
        let session = api.session().unwrap();
        assert_eq!((session.server.as_str(), session.token.as_str(), session.instance), (there.as_str(), "t", Some(id)));
        let kept: Session = serde_json::from_slice(&std::fs::read(dir.path().join("session.json")).unwrap()).unwrap();
        assert_eq!(kept.server, there);
        // Another server: refused, nothing changed.
        let other = answering("200 OK", serde_json::json!({ "instance": Uuid::new_v4(), "username": "ann", "permissions": [] }).to_string());
        assert!(runtime.block_on(api.change_address(&other)).is_err());
        assert_eq!(api.session().unwrap().server, there);
    }
}
