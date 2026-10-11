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
//!
//! The connection to the session's server is shared too (`Connection`): a request that
//! can't connect makes it `Offline`, and from then on requests fail at once (`offline`)
//! instead of each waiting to time out, while a probe asks the server now and then (and
//! at once on `try_now`). Any answer makes it `Online` again.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

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
    /// `offline` (not sent: the server couldn't be reached a moment ago), `unexpected` (an
    /// answer this app can't read). For acting on (edit mode: offer to
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

/// A star or rating as the server answers its change: the annotation, and what it was
/// before the change.
#[derive(Debug, Clone, Deserialize)]
pub struct Changed {
    #[serde(flatten)]
    pub annotation: Annotation,
    pub was_starred: bool,
    pub was_rating: i16,
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

/// Whether the session's server can be reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Connection {
    Online,
    /// A request couldn't connect, at `since`; nothing has answered since.
    Offline { since: SystemTime },
}

impl Connection {
    pub fn is_offline(&self) -> bool {
        matches!(self, Connection::Offline { .. })
    }
}

/// How long the probe waits before asking the server, the first time and at most.
const PROBE_FIRST: Duration = Duration::from_secs(5);
const PROBE_MOST: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Api {
    client: reqwest::Client,
    session_file: Arc<PathBuf>,
    session: Arc<RwLock<Option<Session>>>,
    /// A request was refused as not logged in: the session ended.
    expired: Arc<AtomicBool>,
    incompatible: Arc<AtomicBool>,
    connection: Arc<tokio::sync::watch::Sender<Connection>>,
    /// Wakes the probe (`try_now`).
    probe_now: Arc<tokio::sync::Notify>,
    /// The probe's first wait (shorter in tests).
    probe_first: Duration,
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
            // A server on the LAN answers well inside this; one that doesn't is away.
            .connect_timeout(Duration::from_secs(4))
            .build()
            .expect("an HTTP client");
        Api {
            client,
            session_file: Arc::new(session_file),
            session: Arc::new(RwLock::new(session)),
            expired: Arc::default(),
            incompatible: Arc::default(),
            connection: Arc::new(tokio::sync::watch::Sender::new(Connection::Online)),
            probe_now: Arc::default(),
            probe_first: PROBE_FIRST,
        }
    }

    /// Whether the server can be reached, as of the last request.
    pub fn connection(&self) -> Connection {
        *self.connection.borrow()
    }

    /// Changes of `connection`, for waking whoever shows it or acts on its coming back.
    pub fn watch_connection(&self) -> tokio::sync::watch::Receiver<Connection> {
        self.connection.subscribe()
    }

    /// While offline: asks the server now instead of at the probe's next turn (the user
    /// asked to, or the system says the network changed).
    pub fn try_now(&self) {
        if self.connection().is_offline() {
            self.probe_now.notify_one();
        }
    }

    fn set_connection(&self, to: Connection) {
        let changed = self.connection.send_if_modified(|now| {
            if now.is_offline() == to.is_offline() {
                return false;
            }
            *now = to;
            true
        });
        if changed && to.is_offline() {
            tokio::spawn(self.clone().probe());
        }
    }

    /// While offline: asks the server who's logged in, after 5 s, then less and less often
    /// (up to a minute), until it answers.
    async fn probe(self) {
        let mut wait = self.probe_first;
        loop {
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.probe_now.notified() => {}
            }
            let Some(session) = self.session() else {
                self.set_connection(Connection::Online);
                return;
            };
            if !self.connection().is_offline() {
                return;
            }
            let request = self.client.get(format!("{}/api/auth/me", session.server)).bearer_auth(&session.token);
            let _ = self.go(request, &session.server).await;
            if !self.connection().is_offline() {
                return;
            }
            wait = (wait * 2).min(PROBE_MOST);
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

    /// The request, saying which API version and catalog format this app is built with: a
    /// server of another gets it turned away (426), see `take_incompatible`.
    fn versioned(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header("x-acetate-api", crate::account::api_version()).header("x-acetate-catalog", crate::catalog::model::FORMAT)
    }

    /// Whether the server turned a request away since last asked, for being another version
    /// than this app (it was updated, or the app was).
    pub fn take_incompatible(&self) -> bool {
        self.incompatible.swap(false, Relaxed)
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
            .header("x-acetate-api", crate::account::api_version())
            .header("x-acetate-catalog", crate::catalog::model::FORMAT)
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
        // It answered there: whatever happened at the old address, it's there now.
        self.set_connection(Connection::Online);
        Ok(())
    }

    pub fn logout(&self) {
        *self.session.write().unwrap() = None;
        self.connection.send_replace(Connection::Online);
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
        if let Connection::Offline { .. } = self.connection() {
            return Err(ApiError {
                status: None,
                code: "offline".into(),
                message: "Offline: the server can't be reached (it's tried again now and then).".into(),
                extra_id: None,
            });
        }
        self.request_anyway(method, path)
    }

    /// A request even while offline (the catalog refresh: asked for, or every few minutes).
    fn request_anyway(&self, method: reqwest::Method, path: &str) -> Result<(reqwest::RequestBuilder, String), ApiError> {
        let session = self.session().ok_or_else(|| ApiError {
            status: None,
            code: "unauthorized".into(),
            message: "You're not logged in.".into(),
            extra_id: None,
        })?;
        let request = self.client.request(method, format!("{}{path}", session.server)).bearer_auth(&session.token);
        Ok((self.versioned(request), session.server))
    }

    /// Sends a request: its response if it succeeded, else why not, in words.
    /// A request to the session's server tells whether it can be reached: an answer (any)
    /// that it can, failing to connect that it can't.
    async fn go(&self, request: reqwest::RequestBuilder, server: &str) -> Result<reqwest::Response, ApiError> {
        let sent = request.send().await;
        if self.session().is_some_and(|s| s.server == server) {
            match &sent {
                Ok(_) => self.set_connection(Connection::Online),
                Err(err) if err.is_connect() || err.is_timeout() => self.set_connection(Connection::Offline { since: SystemTime::now() }),
                Err(_) => {}
            }
        }
        let response = sent.map_err(|err| unreachable(&err, server))?;
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
        if status == reqwest::StatusCode::UPGRADE_REQUIRED {
            self.incompatible.store(true, Relaxed);
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
    pub async fn star(&self, kind: &str, id: Uuid, starred: bool) -> Result<Changed, ApiError> {
        let method = if starred { reqwest::Method::POST } else { reqwest::Method::DELETE };
        self.send_json(method, &format!("/api/{kind}/{id}/star"), None).await
    }

    /// Rates it, 0 (not rated) to 10 (five stars).
    pub async fn rate(&self, kind: &str, id: Uuid, rating: i16) -> Result<Changed, ApiError> {
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

    /// Reports a listen (after half the track or 4 minutes). `when`: when it was (seconds
    /// since 1970), and the key that makes sending it again count once.
    pub async fn played(&self, track: Uuid, when: Option<(u64, Uuid)>) -> Result<Annotation, ApiError> {
        let body = when.map(|(at, key)| serde_json::json!({ "played_at": crate::links::rfc3339(at), "key": key }));
        self.send_json(reqwest::Method::POST, &format!("/api/tracks/{track}/played"), body).await
    }

    /// The library (`GET /api/catalog`). With the version of the copy we have (`since`),
    /// only what changed since (a delta); with its ETag, an unchanged library costs the
    /// server no body to send.
    pub async fn catalog(&self, etag: Option<&str>, since: Option<&str>) -> Result<CatalogResponse, ApiError> {
        let (mut request, server) = self.request_anyway(reqwest::Method::GET, "/api/catalog")?;
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
    let message = if err.is_builder() {
        format!("\"{server}\" isn't a valid server address. Check it for typos, and start it with https:// (or http://), like https://music.example.com.")
    } else if err.is_connect() {
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

    /// An address where nothing answers (connecting is refused).
    fn refused() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    #[test]
    fn a_server_that_cant_be_reached_makes_it_offline_until_it_answers() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let away = refused();
        let (_dir, mut api) = api_of(Uuid::new_v4(), &away);
        api.probe_first = Duration::from_millis(50);
        runtime.block_on(async {
            let err = api.send(reqwest::Method::GET, "/api/vocab", None).await.unwrap_err();
            assert_eq!(err.code, "unreachable");
            assert!(api.connection().is_offline());
            // While offline, requests fail at once, without trying.
            let started = std::time::Instant::now();
            let err = api.send(reqwest::Method::GET, "/api/vocab", None).await.unwrap_err();
            assert_eq!(err.code, "offline");
            assert!(started.elapsed() < Duration::from_millis(20));
            // The catalog is asked all the same.
            assert_eq!(api.catalog(None, None).await.err().unwrap().code, "unreachable");
            // The probe keeps asking, and the server is back (at the same address, here
            // another one): online again.
            let mut changes = api.watch_connection();
            let back = answering("200 OK", "{}".into());
            api.session.write().unwrap().as_mut().unwrap().server = back;
            tokio::time::timeout(Duration::from_secs(5), changes.wait_for(|c| !c.is_offline())).await.unwrap().unwrap();
            api.send(reqwest::Method::GET, "/api/vocab", None).await.unwrap();
        });
    }

    #[test]
    fn trying_now_asks_without_waiting_for_the_probe() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (_dir, mut api) = api_of(Uuid::new_v4(), &refused());
        api.probe_first = Duration::from_secs(3600);
        runtime.block_on(async {
            let _ = api.send(reqwest::Method::GET, "/api/vocab", None).await;
            assert!(api.connection().is_offline());
            api.session.write().unwrap().as_mut().unwrap().server = answering("404 Not Found", "{}".into());
            api.try_now();
            // Any answer, an error too, is the server being there.
            let mut changes = api.watch_connection();
            tokio::time::timeout(Duration::from_secs(5), changes.wait_for(|c| !c.is_offline())).await.unwrap().unwrap();
        });
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
