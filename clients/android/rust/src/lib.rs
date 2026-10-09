//! The phone's door into `acetate-core`: the only FFI code. UniFFI turns what's marked here
//! into Kotlin (and Swift) bindings; nothing else in the repo knows about them.
//!
//! One `Core` per process. Lists never cross whole: Kotlin asks for a *view* (albums sorted
//! by X, filtered by Y), gets a handle with a count, and reads the rows on screen with
//! `rows(start, n)`. A view keeps the catalog it was built from, so its rows stay
//! consistent; when the catalog is replaced the listener says so and Kotlin asks again.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use acetate_core::api::{Api, ApiError};
use acetate_core::playback::player::{Engine, Load, Player};
use acetate_core::playback::{QueueItem, queue, queue_item};
use acetate_core::details::{Details, TrackDetail};
use acetate_core::links;
use acetate_core::offline::{Offline, Pin};
use acetate_core::playlists::Playlists;
use acetate_core::resume::Resume;
use acetate_core::catalog::index::{AlbumSort, ArtistSort, TrackRows, TrackSort};
use acetate_core::catalog::store::{self, Store, Update};
use acetate_core::catalog::marks::{Kind, Mark, Marks};
use acetate_core::catalog::{Album, Catalog, Track};
use uuid::Uuid;

uniffi::setup_scaffolding!();

mod editor;
mod forms;
mod media;

/// Android's certificate checking (so a server's HTTPS works with the certificates the
/// phone trusts, including ones the user installed) needs the app's `Context` once, before
/// the first request: `Tls.init(context)` in Kotlin.
#[cfg(target_os = "android")]
mod tls {
    use jni::EnvUnowned;
    use jni::errors::ThrowRuntimeExAndDefault;
    use jni::jni_mangle;
    use jni::objects::{JClass, JObject};

    #[jni_mangle("io.github.allinen.acetate.Tls")]
    pub fn init<'caller>(mut env: EnvUnowned<'caller>, _class: JClass<'caller>, context: JObject<'caller>) {
        // What the Rust side (and the verifier) log goes to logcat, tagged "acetate".
        android_logger::init_once(android_logger::Config::default().with_tag("acetate").with_max_level(log::LevelFilter::Info));
        env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context)).resolve::<ThrowRuntimeExAndDefault>();
    }
}

// ---- Errors, events ----

/// A request that failed: `message` is a sentence for a person, `code` is for acting on.
#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    Failed { code: String, reason: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let CoreError::Failed { reason, .. } = self;
        f.write_str(reason)
    }
}

impl From<ApiError> for CoreError {
    fn from(err: ApiError) -> Self {
        CoreError::Failed { code: err.code, reason: err.message }
    }
}

/// What changed, for Kotlin to read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Change {
    /// Logged in or out, or the session ended.
    Session,
    /// A new catalog is there: views built before are of the old one.
    Catalog,
    /// `sync_state` changed (refreshing, failed, done).
    Sync,
    /// What plays, or the queue, changed (not the position: that's the player's).
    Player,
    /// A star or rating changed (or its answer came): read the marks again.
    Marks,
    /// The playlists, or one playlist's entries, changed: read them again.
    Playlists,
    /// A track's details (its lyrics…) arrived or were refreshed.
    Details,
    /// Downloads: something was kept, taken away, or is coming (`offline_status`).
    Downloads,
    /// What the account may do (edit rights) arrived.
    Account,
    /// An edit's state moved (loaded, previewed, applied): read it again.
    Editor,
}

/// Kotlin code the core calls, from any thread.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn changed(&self, what: Change);
    /// Something to tell the user, in words: a change that failed, or one that went through.
    fn notice(&self, message: String);
}

// ---- Plain data ----

#[derive(Debug, Clone, uniffi::Record)]
pub struct SessionInfo {
    pub server: String,
    pub username: String,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SyncState {
    pub have_catalog: bool,
    /// The server is being asked (the catalog shown, if any, is the kept copy).
    pub refreshing: bool,
    /// The last refresh failed: why, in words. The kept copy is still there.
    pub error: Option<String>,
    pub tracks: u64,
    pub albums: u64,
    pub artists: u64,
}

/// What a star or rating is on.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum MarkOn {
    Track,
    Album,
    Artist,
}

#[derive(Debug, Clone, Copy, Default, uniffi::Record)]
pub struct MarkInfo {
    pub play_count: i32,
    /// 0 to 10 (half stars); 0: not rated.
    pub rating: i16,
    pub starred: bool,
}

/// One line of lyrics: when it's sung (None: the lyrics aren't synced), and the heading of
/// the part of the song it starts, if any.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LyricLine {
    pub at_ms: Option<u64>,
    pub text: String,
    pub heading: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LyricsView {
    pub synced: bool,
    pub language: Option<String>,
    pub lines: Vec<LyricLine>,
}

fn lyrics_view(lyrics: &acetate_core::details::Lyrics) -> LyricsView {
    use acetate_core::lyrics::{self, Block};
    let mut lines = Vec::new();
    if lyrics.is_synced {
        let (parsed, sections) = lyrics::parse_with_sections(&lyrics.body);
        let mut last = None;
        for line in parsed {
            let heading = (line.section != last).then_some(line.section).flatten().map(|s| heading_text(&sections[s]));
            last = line.section;
            lines.push(LyricLine { at_ms: Some(line.at.as_millis() as u64), text: line.text, heading });
        }
    } else {
        let mut heading = None;
        for block in lyrics::blocks(&lyrics.body) {
            match block {
                Block::Heading(section) => heading = Some(heading_text(&section)),
                Block::Text(text) => {
                    for line in text.lines() {
                        lines.push(LyricLine { at_ms: None, text: line.to_string(), heading: heading.take() });
                    }
                }
            }
        }
    }
    LyricsView { synced: lyrics.is_synced, language: lyrics.language.clone(), lines }
}

/// "Verse 1" or "Verse 1 · Wrenfield, Someone".
fn heading_text(section: &acetate_core::lyrics::Section) -> String {
    if section.artists.is_empty() { section.label.clone() } else { format!("{} · {}", section.label, section.artists.join(", ")) }
}

/// Where a link leads.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LinkTarget {
    Album { id: String },
    Artist { id: String },
    /// A link to another server than the one logged in to.
    OtherServer { server: String },
    /// Not in this library (yet).
    NotThere,
    /// Not a link this app opens.
    Invalid,
}

fn link_target(on: MarkOn, id: Uuid) -> links::Target {
    match on {
        MarkOn::Track => links::Target::Track(id),
        MarkOn::Album => links::Target::Album(id),
        MarkOn::Artist => links::Target::Artist(id),
    }
}

/// An extra: something that isn't a track (a stem, a vocal take, a video…).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ExtraRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub duration_ms: Option<i64>,
    /// The file to play, when it's audio.
    pub audio_file: Option<String>,
    /// The file to watch, when it's a video.
    pub video_file: Option<String>,
    /// A video's thumbnail (a cover key).
    pub thumb: Option<String>,
}

/// What can be kept on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PinKind {
    Track,
    Album,
    Playlist,
    /// The starred tracks (as they change).
    Liked,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PinRow {
    pub kind: PinKind,
    pub id: String,
    pub name: String,
}

/// One thing kept on the device, and how much of it is here.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PinInfo {
    pub kind: PinKind,
    pub id: String,
    pub name: String,
    /// Files here, of `total`, and their size.
    pub have: u64,
    pub total: u64,
    pub bytes: u64,
    /// Whether what it covers is known (a playlist still being fetched isn't).
    pub known: bool,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct DownloadsInfo {
    pub pins: Vec<PinRow>,
    /// Files here, and their size.
    pub files: u64,
    pub bytes: u64,
    /// Still to come (the one coming now too), of this run's `asked`, of which `done`.
    pub queued: u64,
    pub downloading: bool,
    pub asked: u64,
    pub done: u64,
    pub failed: u64,
    /// Stopped by the user.
    pub paused: bool,
    /// Every byte fetched since the app started, and of the file coming now.
    pub received: u64,
    pub current_bytes: u64,
}

/// An artist as credited somewhere, to go to their page.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ArtistLink {
    pub id: String,
    /// The name credited (an alias, if that's what's credited).
    pub name: String,
    /// "primary", "featured", "producer"…
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrackPage {
    pub row: TrackRow,
    pub artists: Vec<ArtistLink>,
    /// Every album it's on, its main one first.
    pub albums: Vec<AlbumRow>,
    pub genres: Vec<String>,
    pub release_date: Option<String>,
    pub status: String,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
}

fn artist_links(catalog: &Catalog, credits: &[acetate_core::catalog::model::Credit]) -> Vec<ArtistLink> {
    use acetate_core::catalog::model::Role;
    let mut seen = std::collections::HashSet::new();
    credits
        .iter()
        .filter(|c| seen.insert((c.0, std::mem::discriminant(&c.1))))
        .filter_map(|c| {
            let artist = catalog.artist(c.0)?;
            let role = match c.1 {
                Role::Primary => "primary",
                Role::Featured => "featured",
                Role::Producer => "producer",
                Role::Other => "other",
            };
            Some(ArtistLink { id: c.0.to_string(), name: c.2.as_deref().map(str::to_string).unwrap_or_else(|| artist.name.clone()), role: role.into() })
        })
        .collect()
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchResult {
    pub artists: Vec<ArtistRow>,
    pub albums: Vec<AlbumRow>,
    pub tracks: Vec<TrackRow>,
    pub playlists: Vec<PlaylistRow>,
}

/// A suggestion: what it says, and the whole search line once it's chosen.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Chip {
    pub label: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PlaylistRow {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub mine: bool,
    pub public: bool,
    pub entries: i64,
    pub duration_ms: i64,
}

/// One place in a playlist (`entry` identifies it: the same track can be there twice).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EntryRow {
    pub entry: String,
    pub track: TrackRow,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct PlaylistPage {
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner: String,
    pub mine: bool,
    pub public: bool,
    /// Being fetched from the server (what's shown is the kept copy, if any).
    pub loading: bool,
    pub error: Option<String>,
    pub entries: Vec<EntryRow>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AlbumRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub kind: String,
    pub year: Option<i32>,
    pub track_count: i64,
    pub duration_ms: i64,
    /// The cover's key, for `image_request`.
    pub cover: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ArtistRow {
    pub id: String,
    pub name: String,
    pub track_count: i64,
    pub album_count: i64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TrackRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// The album it belongs to first.
    pub album: Option<String>,
    pub year: Option<i16>,
    pub duration_ms: Option<i64>,
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum AlbumOrder {
    Added,
    Title,
    Artist,
    Year,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ArtistOrder {
    Name,
    Tracks,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum TrackOrder {
    Added,
    Title,
    Artist,
    Album,
    Year,
    Length,
    Plays,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ImageSize {
    Small,
    Standard,
    Original,
}

/// What an image loader (Coil) needs to fetch a cover: the address and the login header.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImageRequest {
    pub url: String,
    pub authorization: String,
    /// The same image kept on the device (downloads keep the covers of what they keep): a
    /// file to read instead.
    pub local: Option<String>,
}

fn album_row(a: Album) -> AlbumRow {
    AlbumRow {
        id: a.id().to_string(),
        title: a.title().to_string(),
        artist: a.artist_display().to_string(),
        kind: a.kind().to_string(),
        year: a.year(),
        track_count: a.track_count(),
        duration_ms: a.duration_ms(),
        cover: a.cover().map(|c| cover_key(c.key.0)),
    }
}

fn track_row(catalog: &Catalog, t: Track) -> TrackRow {
    TrackRow {
        id: t.id().to_string(),
        title: t.title().to_string(),
        artist: t.artist_display().to_string(),
        album: catalog.main_album(t).map(|a| a.title().to_string()),
        year: t.year(),
        duration_ms: t.duration_ms(),
        cover: t.cover().map(|c| cover_key(c.key.0)),
    }
}

fn cover_key(key: u64) -> String {
    format!("{key:016x}")
}

fn parse_id(id: &str) -> Option<Uuid> {
    id.parse().ok()
}

// ---- Views ----

/// Albums, filtered and sorted. `count` is cheap; `rows` copies only the rows asked for.
#[derive(uniffi::Object)]
pub struct AlbumView {
    catalog: Option<Arc<Catalog>>,
    list: Vec<u32>,
}

#[uniffi::export]
impl AlbumView {
    pub fn count(&self) -> u64 {
        self.list.len() as u64
    }

    pub fn rows(&self, start: u64, n: u64) -> Vec<AlbumRow> {
        let Some(catalog) = &self.catalog else { return vec![] };
        window(&self.list, start, n).iter().map(|&i| album_row(catalog.album_at(i))).collect()
    }
}

#[derive(uniffi::Object)]
pub struct ArtistView {
    catalog: Option<Arc<Catalog>>,
    list: Vec<u32>,
}

#[uniffi::export]
impl ArtistView {
    pub fn count(&self) -> u64 {
        self.list.len() as u64
    }

    pub fn rows(&self, start: u64, n: u64) -> Vec<ArtistRow> {
        let Some(catalog) = &self.catalog else { return vec![] };
        window(&self.list, start, n)
            .iter()
            .map(|&i| {
                let a = &catalog.artists[i as usize];
                ArtistRow { id: a.id.to_string(), name: a.label(), track_count: a.track_count, album_count: a.album_count }
            })
            .collect()
    }
}

/// Tracks of the whole library, or of one album or artist.
#[derive(uniffi::Object)]
pub struct TrackView {
    catalog: Option<Arc<Catalog>>,
    list: Vec<u32>,
}

#[uniffi::export]
impl TrackView {
    pub fn count(&self) -> u64 {
        self.list.len() as u64
    }

    pub fn rows(&self, start: u64, n: u64) -> Vec<TrackRow> {
        let Some(catalog) = &self.catalog else { return vec![] };
        window(&self.list, start, n).iter().map(|&i| track_row(catalog, catalog.track_at(i))).collect()
    }
}

fn window(list: &[u32], start: u64, n: u64) -> &[u32] {
    let start = (start as usize).min(list.len());
    let end = start.saturating_add(n as usize).min(list.len());
    &list[start..end]
}

// ---- The core ----

struct Shared {
    api: Api,
    player: Arc<Player>,
    /// Whose queue is being kept (the key of the server and user).
    keeping: Mutex<Option<String>>,
    store: Mutex<Option<Arc<Store>>>,
    catalog: Mutex<Option<Arc<Catalog>>>,
    sync: Mutex<SyncState>,
    /// Stars and ratings changed in this session, shown until the server's catalog has them.
    marks: Mutex<Marks>,
    playlists: Arc<Mutex<Playlists>>,
    track_details: Arc<Mutex<Details<TrackDetail>>>,
    /// Who's logged in, what they may do, and the server's fixed lists.
    account: Arc<Mutex<acetate_core::account::Account>>,
    account_wake: acetate_core::Waker,
    /// The downloads of whoever is logged in (set when their sync starts).
    offline: Mutex<Option<Arc<Offline>>>,
    /// What the app said about them, for when they're opened.
    download_quality: Mutex<Option<u32>>,
    downloads_allowed: std::sync::atomic::AtomicBool,
    /// When the catalog was last asked of the server.
    asked: Mutex<Option<Instant>>,
    listener: Arc<dyn Listener>,
}

impl Shared {
    fn catalog(&self) -> Option<Arc<Catalog>> {
        self.catalog.lock().unwrap().clone()
    }
}

/// The app's one core: the session, the catalog and everything asked of them.
#[derive(uniffi::Object)]
pub struct Core {
    data_dir: std::path::PathBuf,
    shared: Arc<Shared>,
    runtime: tokio::runtime::Runtime,
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// `data_dir`: the app's own folder (the session and the kept catalog live there).
    #[uniffi::constructor]
    pub fn new(data_dir: String, listener: Arc<dyn Listener>) -> Arc<Core> {
        let data_dir = std::path::PathBuf::from(data_dir);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("acetate-core")
            .build()
            .expect("a tokio runtime");
        let api = Api::new(data_dir.clone());
        let player = Arc::new(Player::new(Some(api.clone()), runtime.handle().clone(), Arc::new(ChangeWake(listener.clone()))));
        let playlist_wake = Arc::new(PlaylistWake { playlists: std::sync::OnceLock::new(), listener: listener.clone() });
        let playlists = Arc::new(Mutex::new(Playlists::new(api.clone(), runtime.handle().clone(), playlist_wake.clone())));
        let _ = playlist_wake.playlists.set(Arc::downgrade(&playlists));
        let detail_wake = Arc::new(DetailWake { details: std::sync::OnceLock::new(), listener: listener.clone() });
        let track_details = Arc::new(Mutex::new(Details::new("tracks", api.clone(), runtime.handle().clone(), detail_wake.clone())));
        let _ = detail_wake.details.set(Arc::downgrade(&track_details));
        let account_wake = Arc::new(AccountWake { account: std::sync::OnceLock::new(), listener: listener.clone() });
        let account = Arc::new(Mutex::new(acetate_core::account::Account::default()));
        let _ = account_wake.account.set(Arc::downgrade(&account));
        let account_wake: acetate_core::Waker = account_wake;
        let shared = Arc::new(Shared {
            api,
            player,
            account,
            account_wake,
            keeping: Mutex::new(None),
            store: Mutex::new(None),
            catalog: Mutex::new(None),
            sync: Mutex::new(SyncState::default()),
            marks: Mutex::new(Marks::default()),
            playlists,
            track_details,
            offline: Mutex::new(None),
            download_quality: Mutex::new(None),
            downloads_allowed: std::sync::atomic::AtomicBool::new(true),
            asked: Mutex::new(None),
            listener,
        });
        // What's downloaded plays from the device.
        let weak = Arc::downgrade(&shared);
        shared.player.set_local(Arc::new(move |sha| weak.upgrade()?.offline.lock().unwrap().as_ref()?.local_path(sha)));
        Arc::new(Core { data_dir, shared, runtime })
    }

    /// Who's logged in (kept between runs), if anyone.
    pub fn session(&self) -> Option<SessionInfo> {
        self.shared.api.session().map(|s| SessionInfo { server: s.server, username: s.username })
    }

    /// Asks an address (no login) and says what came back: the HTTP status, or why the
    /// connection failed, causes and all.
    pub async fn reach(&self, address: String) -> String {
        let api = self.shared.api.clone();
        match self.runtime.spawn(async move { api.reach(&address).await }).await {
            Ok(Ok(status)) => format!("answered {status}"),
            Ok(Err(why)) => format!("failed: {why}"),
            Err(e) => format!("failed: {e}"),
        }
    }

    /// Whether the session ended since last asked (a request was refused as not logged in).
    pub fn take_expired(&self) -> bool {
        self.shared.api.take_expired()
    }

    pub async fn login(&self, server: String, username: String, password: String) -> Result<SessionInfo, CoreError> {
        // On the core's runtime, not the caller's thread: a request may call into Android
        // (checking a certificate), which it won't allow on the main thread.
        let api = self.shared.api.clone();
        let session = self
            .runtime
            .spawn(async move { api.login(&server, &username, &password).await })
            .await
            .map_err(|e| CoreError::Failed { code: "unexpected".into(), reason: format!("the login stopped: {e}") })??;
        self.shared.listener.changed(Change::Session);
        Ok(SessionInfo { server: session.server, username: session.username })
    }

    /// Logs out; the kept catalog stays on disk for the next login of the same user.
    pub fn logout(&self) {
        self.shared.player.stop();
        *self.shared.keeping.lock().unwrap() = None;
        self.shared.api.logout();
        *self.shared.catalog.lock().unwrap() = None;
        *self.shared.store.lock().unwrap() = None;
        *self.shared.sync.lock().unwrap() = SyncState::default();
        self.shared.marks.lock().unwrap().clear();
        self.shared.playlists.lock().unwrap().reset(None);
        self.shared.track_details.lock().unwrap().reset(None);
        *self.shared.offline.lock().unwrap() = None;
        *self.shared.account.lock().unwrap() = Default::default();
        self.shared.listener.changed(Change::Session);
    }

    /// Shows the kept catalog and asks the server what changed (once per call: call it at
    /// start and when the app comes back to the front).
    pub fn start_sync(&self) {
        let Some(session) = self.shared.api.session() else { return };
        let store = Arc::new(Store::new(&self.data_dir, &session));
        *self.shared.store.lock().unwrap() = Some(store.clone());
        // The queue is kept per server and user, and brought back once the catalog is there.
        let key = acetate_core::catalog::store::key(&session);
        let mut keeping = self.shared.keeping.lock().unwrap();
        if keeping.as_deref() != Some(&key) {
            self.shared.player.keep_with(Resume::new(&self.data_dir, &key));
            self.shared.playlists.lock().unwrap().reset(Some(self.data_dir.join(format!("playlists-{key}"))));
            self.shared.track_details.lock().unwrap().reset(Some(self.data_dir.join(format!("details-{key}"))));
            match Offline::open(&self.data_dir.join(format!("offline-{key}")), self.shared.api.clone(), self.runtime.handle().clone(), Arc::new(DownloadWake(self.shared.listener.clone()))) {
                Ok(offline) => {
                    offline.set_quality(*self.shared.download_quality.lock().unwrap());
                    offline.set_allowed(self.shared.downloads_allowed.load(std::sync::atomic::Ordering::SeqCst));
                    *self.shared.offline.lock().unwrap() = Some(offline);
                }
                Err(err) => log::warn!("couldn't open the downloads: {err:#}"),
            }
            self.shared.player.keep_listens(self.data_dir.join(format!("unsent-{key}.json")));
            *keeping = Some(key);
        } else {
            // Back to the front: the connection may be back.
            self.shared.player.send_unsent();
        }
        drop(keeping);
        {
            let mut account = self.shared.account.lock().unwrap();
            if account.me.is_none() {
                account.fetch(&self.shared.api, self.runtime.handle(), &self.shared.account_wake);
            }
        }
        let shared = self.shared.clone();
        *shared.asked.lock().unwrap() = Some(Instant::now());
        set_sync(&shared, |s| {
            s.refreshing = true;
            s.error = None;
        });
        self.runtime.spawn(async move {
            let report = |update: Update| match update {
                Update::Loaded { catalog, from_server } => {
                    if from_server {
                        if let Some(asked) = *shared.asked.lock().unwrap() {
                            shared.marks.lock().unwrap().catalog_arrived(asked);
                            shared.playlists.lock().unwrap().catalog_arrived(asked);
                        }
                    }
                    set_catalog(&shared, catalog);
                }
                Update::Unchanged => {}
                Update::Failed(message) => set_sync(&shared, |s| s.error = Some(message)),
            };
            store::load_and_refresh(store, shared.api.clone(), report).await;
            set_sync(&shared, |s| s.refreshing = false);
        });
    }

    pub fn sync_state(&self) -> SyncState {
        self.shared.sync.lock().unwrap().clone()
    }

    // ---- Stars and ratings ----

    /// The user's star, rating and play count on a track, album or artist, with this
    /// session's changes.
    pub fn mark(&self, id: String) -> MarkInfo {
        let Some(id) = parse_id(&id) else { return MarkInfo::default() };
        let catalog = self.shared.catalog();
        let mark = self.shared.marks.lock().unwrap().get(catalog.as_deref(), id);
        MarkInfo { play_count: mark.play_count, rating: mark.rating, starred: mark.starred }
    }

    /// Stars or unstars; shown at once, sent to the server.
    pub fn set_starred(&self, on: MarkOn, id: String, starred: bool) {
        self.change_mark(on, &id, |m| m.starred = starred, true);
    }

    /// Rates, 0 (not rated) to 10 (five stars).
    pub fn set_rating(&self, on: MarkOn, id: String, rating: i16) {
        self.change_mark(on, &id, |m| m.rating = rating.clamp(0, 10), false);
    }

    // ---- Playlists ----

    /// The playlists: the user's own first, then public ones. (Liked songs aren't one: see
    /// `liked`.)
    pub fn playlists(&self) -> Vec<PlaylistRow> {
        let catalog = self.shared.catalog();
        let playlists = self.shared.playlists.lock().unwrap();
        playlists
            .list(catalog.as_deref())
            .iter()
            .map(|p| PlaylistRow {
                id: p.id.to_string(),
                name: p.name.clone(),
                owner: p.owner.clone(),
                mine: p.mine,
                public: p.public,
                entries: p.entries,
                duration_ms: p.duration_ms,
            })
            .collect()
    }

    /// A playlist with its entries, as kept; fetched in the background the first time in a
    /// run (`Change::Playlists` says when it came).
    pub fn playlist(&self, id: String) -> Option<PlaylistPage> {
        let id = parse_id(&id)?;
        let catalog = self.shared.catalog()?;
        let mut playlists = self.shared.playlists.lock().unwrap();
        let cached = playlists.detail(id);
        let (loading, error) = (cached.refreshing, cached.error.clone());
        let Some(detail) = cached.value.clone() else {
            return Some(PlaylistPage { id: id.to_string(), loading, error, ..PlaylistPage::default() });
        };
        let entries = detail
            .entries
            .iter()
            .filter_map(|e| {
                let t = catalog.track_index(e.track_id)?;
                Some(EntryRow { entry: e.id.to_string(), track: track_row(&catalog, catalog.track_at(t)) })
            })
            .collect();
        Some(PlaylistPage {
            id: id.to_string(),
            name: detail.name.clone(),
            description: detail.description.clone(),
            owner: detail.owner.clone(),
            mine: detail.mine,
            public: detail.public,
            loading,
            error,
            entries,
        })
    }

    /// The starred tracks, newest first (what "Liked songs" shows).
    pub fn liked(&self) -> Vec<TrackRow> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        let liked = self.shared.marks.lock().unwrap().liked(&catalog);
        liked.into_iter().map(|t| track_row(&catalog, catalog.track_at(t))).collect()
    }

    pub fn create_playlist(&self, name: String, tracks: Vec<String>) {
        let tracks = tracks.iter().filter_map(|t| parse_id(t)).collect();
        self.shared.playlists.lock().unwrap().create(name, tracks);
    }

    pub fn add_to_playlist(&self, id: String, name: String, tracks: Vec<String>) {
        let Some(id) = parse_id(&id) else { return };
        let tracks = tracks.iter().filter_map(|t| parse_id(t)).collect();
        self.shared.playlists.lock().unwrap().add(id, name, tracks);
    }

    pub fn remove_from_playlist(&self, id: String, entries: Vec<String>) {
        let Some(id) = parse_id(&id) else { return };
        let entries = entries.iter().filter_map(|e| parse_id(e)).collect();
        self.shared.playlists.lock().unwrap().remove(id, entries);
        self.shared.listener.changed(Change::Playlists);
    }

    /// Moves an entry up (`by` -1) or down (1).
    pub fn move_in_playlist(&self, id: String, entry: String, by: i32) {
        let (Some(id), Some(entry)) = (parse_id(&id), parse_id(&entry)) else { return };
        self.shared.playlists.lock().unwrap().move_entry(id, entry, by as isize);
        self.shared.listener.changed(Change::Playlists);
    }

    pub fn rename_playlist(&self, id: String, name: String) {
        let Some(id) = parse_id(&id) else { return };
        self.shared.playlists.lock().unwrap().update(id, Some(name), None, None);
        self.shared.listener.changed(Change::Playlists);
    }

    /// Moves it to the trash (the server keeps it there).
    pub fn delete_playlist(&self, id: String, name: String) {
        let Some(id) = parse_id(&id) else { return };
        self.shared.playlists.lock().unwrap().trash(id, name);
    }

    // ---- Lists ----

    pub async fn albums(&self, filter: String, order: AlbumOrder) -> Arc<AlbumView> {
        let catalog = self.shared.catalog();
        blocking(move || {
            let sort = match order {
                AlbumOrder::Added => AlbumSort::Added,
                AlbumOrder::Title => AlbumSort::Title,
                AlbumOrder::Artist => AlbumSort::Artist,
                AlbumOrder::Year => AlbumSort::Year,
            };
            let list = catalog.as_deref().map(|catalog| catalog.album_list(&filter, sort)).unwrap_or_default();
            Arc::new(AlbumView { catalog, list })
        })
        .await
    }

    pub async fn artists(&self, filter: String, order: ArtistOrder) -> Arc<ArtistView> {
        let catalog = self.shared.catalog();
        blocking(move || {
            let sort = match order {
                ArtistOrder::Name => ArtistSort::Name,
                ArtistOrder::Tracks => ArtistSort::Tracks,
            };
            let list = catalog.as_deref().map(|catalog| catalog.artist_list(&filter, sort)).unwrap_or_default();
            Arc::new(ArtistView { catalog, list })
        })
        .await
    }

    pub async fn tracks(&self, filter: String, order: TrackOrder, descending: bool) -> Arc<TrackView> {
        let catalog = self.shared.catalog();
        blocking(move || {
            let sort = match order {
                TrackOrder::Added => TrackSort::Added,
                TrackOrder::Title => TrackSort::Title,
                TrackOrder::Artist => TrackSort::Artist,
                TrackOrder::Album => TrackSort::Album,
                TrackOrder::Year => TrackSort::Year,
                TrackOrder::Length => TrackSort::Length,
                TrackOrder::Plays => TrackSort::Plays,
            };
            let rows = TrackRows { tracks: None, added: None };
            let list = catalog
                .as_deref()
                .map(|catalog| catalog.order_tracks(&rows, &filter, sort, descending).into_iter().map(|i| i as u32).collect())
                .unwrap_or_default();
            Arc::new(TrackView { catalog, list })
        })
        .await
    }

    // ---- Downloads ----

    /// Keeps (or stops keeping) a track, album, playlist or the liked songs on the device.
    pub fn download(&self, what: PinKind, id: String, on: bool) {
        let Some(offline) = self.shared.offline.lock().unwrap().clone() else { return };
        let (kind, id, name) = match what {
            PinKind::Track => ("track", id.clone(), self.shared.catalog().and_then(|c| Some(c.track(parse_id(&id)?)?.title().to_string()))),
            PinKind::Album => ("album", id.clone(), self.shared.catalog().and_then(|c| Some(c.album(parse_id(&id)?)?.title().to_string()))),
            PinKind::Playlist => ("playlist", id.clone(), self.playlists().into_iter().find(|p| p.id == id).map(|p| p.name)),
            PinKind::Liked => ("liked", String::new(), Some("Liked songs".to_string())),
        };
        offline.pin(Pin { kind: kind.into(), id, name: name.unwrap_or_default() }, on);
        refresh_offline(&self.shared);
    }

    pub fn is_downloading(&self, what: PinKind, id: String) -> bool {
        let kind = match what {
            PinKind::Track => "track",
            PinKind::Album => "album",
            PinKind::Playlist => "playlist",
            PinKind::Liked => "liked",
        };
        let id = if what == PinKind::Liked { String::new() } else { id };
        self.shared.offline.lock().unwrap().as_ref().is_some_and(|o| o.is_pinned(kind, &id))
    }

    /// Whether a track's file is on the device.
    pub fn is_downloaded(&self, track: String) -> bool {
        let (Some(offline), Some(catalog)) = (self.shared.offline.lock().unwrap().clone(), self.shared.catalog()) else { return false };
        parse_id(&track).and_then(|id| catalog.track(id)?.sha256()).is_some_and(|sha| offline.local_path(&sha).is_some())
    }

    /// What's kept: the pins, and how much of them is here.
    pub fn downloads(&self) -> DownloadsInfo {
        let Some(offline) = self.shared.offline.lock().unwrap().clone() else { return DownloadsInfo::default() };
        let status = offline.status();
        DownloadsInfo {
            pins: offline
                .pins()
                .into_iter()
                .map(|p| PinRow {
                    kind: match p.kind.as_str() {
                        "track" => PinKind::Track,
                        "album" => PinKind::Album,
                        "playlist" => PinKind::Playlist,
                        _ => PinKind::Liked,
                    },
                    id: p.id,
                    name: p.name,
                })
                .collect(),
            files: status.files as u64,
            bytes: status.bytes,
            queued: status.queued as u64,
            downloading: status.downloading,
            asked: status.asked as u64,
            done: status.done as u64,
            failed: status.failed as u64,
            paused: status.paused,
            received: status.received,
            current_bytes: status.current_bytes,
        }
    }

    /// What's kept, each with how much of it is here: for the downloads list.
    pub fn download_pins(&self) -> Vec<PinInfo> {
        let (Some(offline), Some(catalog)) = (self.shared.offline.lock().unwrap().clone(), self.shared.catalog()) else { return vec![] };
        let shared = &self.shared;
        offline
            .pins()
            .into_iter()
            .map(|pin| {
                let tracks = acetate_core::offline::pin_tracks(
                    &pin,
                    &catalog,
                    &mut |id| shared.playlists.lock().unwrap().detail(id).value.as_ref().map(|d| d.entries.iter().map(|e| e.track_id).collect()),
                    &|| shared.marks.lock().unwrap().liked(&catalog),
                );
                let sources: Vec<String> = tracks.iter().flatten().filter_map(|&t| catalog.track_at(t).sha256()).collect();
                let sizes: Vec<u64> = sources.iter().filter_map(|s| offline.kept_size(s)).collect();
                PinInfo {
                    kind: match pin.kind.as_str() {
                        "track" => PinKind::Track,
                        "album" => PinKind::Album,
                        "playlist" => PinKind::Playlist,
                        _ => PinKind::Liked,
                    },
                    id: pin.id,
                    name: pin.name,
                    have: sizes.len() as u64,
                    total: sources.len() as u64,
                    bytes: sizes.iter().sum(),
                    known: tracks.is_some(),
                }
            })
            .collect()
    }

    /// The quality to download at (None: the original if the server allows it).
    pub fn set_download_quality(&self, kbps: Option<u32>) {
        if let Some(offline) = self.shared.offline.lock().unwrap().as_ref() {
            offline.set_quality(kbps);
        }
        *self.shared.download_quality.lock().unwrap() = kbps;
    }

    /// Whether downloads may run now (the connection the user allows them on).
    pub fn set_downloads_allowed(&self, allowed: bool) {
        self.shared.downloads_allowed.store(allowed, std::sync::atomic::Ordering::SeqCst);
        if let Some(offline) = self.shared.offline.lock().unwrap().clone() {
            offline.set_allowed(allowed);
        }
    }

    /// Stops the downloads (what's here stays, what's coming waits).
    pub fn pause_downloads(&self) {
        if let Some(offline) = self.shared.offline.lock().unwrap().as_ref() {
            offline.pause();
        }
    }

    pub fn resume_downloads(&self) {
        if let Some(offline) = self.shared.offline.lock().unwrap().clone() {
            offline.resume();
        }
    }

    /// Looks at the pins again (a playlist or the liked songs changed).
    pub fn refresh_downloads(&self) {
        refresh_offline(&self.shared);
    }

    /// Forgets every download and takes the files away.
    pub fn remove_downloads(&self) {
        if let Some(offline) = self.shared.offline.lock().unwrap().clone() {
            offline.remove_all();
        }
    }

    // ---- Links and sharing ----

    /// The link to give someone who has an account (opens in their app or the browser).
    pub fn open_link(&self, on: MarkOn, id: String) -> Option<String> {
        let session = self.shared.api.session()?;
        Some(links::web(&session.server, link_target(on, parse_id(&id)?)))
    }

    /// A link anyone can listen with (`POST /api/shares`), to a track or album, that stops
    /// working after `days` (None: never).
    pub async fn make_share(&self, on: MarkOn, id: String, days: Option<u32>) -> Result<String, CoreError> {
        let Some(id) = parse_id(&id) else {
            return Err(CoreError::Failed { code: "unexpected".into(), reason: "that isn't an id".into() });
        };
        let api = self.shared.api.clone();
        let target = link_target(on, id);
        self.runtime
            .spawn(async move { api.share(target, days).await })
            .await
            .map_err(|e| CoreError::Failed { code: "unexpected".into(), reason: format!("the request stopped: {e}") })?
            .map_err(CoreError::from)
    }

    /// What a link (`acetate://open/…` or a web link to the server) points to, here.
    pub fn resolve_link(&self, text: String) -> LinkTarget {
        let Some(link) = links::parse(&text) else { return LinkTarget::Invalid };
        if let (Some(server), Some(session)) = (link.server.as_deref(), self.shared.api.session())
            && !links::same_server(server, &session.server)
        {
            return LinkTarget::OtherServer { server: server.to_string() };
        }
        let Some(catalog) = self.shared.catalog() else { return LinkTarget::NotThere };
        match link.target {
            links::Target::Album(id) if catalog.album_index(id).is_some() => LinkTarget::Album { id: id.to_string() },
            links::Target::Artist(id) if catalog.artist(id).is_some() => LinkTarget::Artist { id: id.to_string() },
            // A track opens on its album.
            links::Target::Track(id) => match catalog.track(id).and_then(|t| catalog.main_album(t)) {
                Some(album) => LinkTarget::Album { id: album.id().to_string() },
                None => LinkTarget::NotThere,
            },
            _ => LinkTarget::NotThere,
        }
    }

    // ---- Extras (vocals, stems, videos…) ----

    /// The extras linked to a track, album or artist.
    pub fn extras_of(&self, on: MarkOn, id: String) -> Vec<ExtraRow> {
        let (Some(id), Some(catalog)) = (parse_id(&id), self.shared.catalog()) else { return vec![] };
        let kind = match on {
            MarkOn::Track => "track",
            MarkOn::Album => "album",
            MarkOn::Artist => "artist",
        };
        catalog
            .extras
            .iter()
            .filter(|e| e.links.iter().any(|(k, i)| k == kind && *i == id))
            .map(|e| ExtraRow {
                id: e.id.to_string(),
                name: e.name.clone(),
                kind: e.kind.clone(),
                duration_ms: e.duration_ms,
                audio_file: e.first_file.filter(|_| e.video.is_none()).map(|f| f.to_string()),
                video_file: e.video.map(|v| v.to_string()),
                thumb: e.thumb_key.clone(),
            })
            .collect()
    }

    /// How to fetch an extra's file (a video's stream): the address and the login header.
    pub fn extra_stream(&self, file: String) -> Option<ImageRequest> {
        let session = self.shared.api.session()?;
        Some(ImageRequest {
            url: format!("{}/api/extra-files/{file}/stream", session.server),
            authorization: format!("Bearer {}", session.token),
            local: None,
        })
    }

    // ---- Lyrics ----

    /// A track's lyrics (the synced ones if it has them): None until they're known or if it
    /// has none; `Change::Details` says when they came.
    pub fn lyrics(&self, track: String) -> Option<LyricsView> {
        let id = parse_id(&track)?;
        let detail = {
            let mut details = self.shared.track_details.lock().unwrap();
            let cached = details.get(id);
            if cached.value.is_none() {
                log::info!("details of {id}: not there yet (refreshing {}, error {:?})", cached.refreshing, cached.error);
            }
            cached.value.clone()?
        };
        log::info!("details of {id}: {} lyrics entries", detail.lyrics.len());
        let lyrics = detail.lyrics.iter().find(|l| l.is_synced).or(detail.lyrics.first())?;
        Some(lyrics_view(lyrics))
    }

    // ---- Search ----

    /// What fits the search line (the query language; plain words match fuzzily): the best
    /// `limit` of each kind, best first.
    pub async fn search(&self, text: String, limit: u64) -> SearchResult {
        let Some(catalog) = self.shared.catalog() else { return SearchResult::default() };
        let playlists = self.playlists();
        blocking(move || {
            let hits = acetate_core::catalog::search::Search::default().run_query(&catalog, &text, limit as usize);
            let words: Vec<String> = acetate_core::catalog::query::Query::parse(&text).words;
            SearchResult {
                artists: hits
                    .artists
                    .iter()
                    .map(|&(i, _)| {
                        let a = &catalog.artists[i as usize];
                        ArtistRow { id: a.id.to_string(), name: a.label(), track_count: a.track_count, album_count: a.album_count }
                    })
                    .collect(),
                albums: hits.albums.iter().map(|&(i, _)| album_row(catalog.album_at(i))).collect(),
                tracks: hits.tracks.iter().map(|&(i, _)| track_row(&catalog, catalog.track_at(i))).collect(),
                // Playlists by their names (every word in it), when words were typed.
                playlists: playlists
                    .into_iter()
                    .filter(|p| !words.is_empty() && words.iter().all(|w| p.name.to_lowercase().contains(w)))
                    .take(limit as usize)
                    .collect(),
            }
        })
        .await
    }

    /// Chips to complete the search line (the keys, or values from the library).
    pub async fn suggest(&self, text: String) -> Vec<Chip> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        blocking(move || {
            acetate_core::catalog::suggest::suggest(&catalog, &text)
                .into_iter()
                .map(|s| Chip { label: s.label, line: s.line })
                .collect()
        })
        .await
    }

    // ---- Pages ----

    /// Which of these tracks (by id, in this order) fit a filter in the query language: their
    /// places in the list, in order. For filtering a playlist's tracks without losing their order.
    pub async fn filter_list(&self, ids: Vec<String>, filter: String) -> Vec<u64> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        blocking(move || {
            // Places in the catalog of the ones that are there (a track gone is left out).
            let (places, at): (Vec<u32>, Vec<u64>) =
                ids.iter().enumerate().filter_map(|(n, id)| Some((catalog.track_index(parse_id(id)?)?, n as u64))).unzip();
            let rows = TrackRows { tracks: Some(&places), added: None };
            catalog.order_tracks(&rows, &filter, TrackSort::Natural, false).into_iter().map(|row| at[row]).collect()
        })
        .await
    }

    /// The artists credited on a track or album (to go to their pages), as credited.
    pub fn artists_of(&self, on: MarkOn, id: String) -> Vec<ArtistLink> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        let credits: Vec<_> = match on {
            MarkOn::Track => catalog.track(id).map(|t| t.credits().to_vec()),
            MarkOn::Album => catalog.album(id).map(|a| a.credits().to_vec()),
            MarkOn::Artist => None,
        }
        .unwrap_or_default();
        artist_links(&catalog, &credits)
    }

    /// Everything about one track that its page shows.
    pub fn track_page(&self, id: String) -> Option<TrackPage> {
        let catalog = self.shared.catalog()?;
        let t = catalog.track(parse_id(&id)?)?;
        Some(TrackPage {
            row: track_row(&catalog, t),
            artists: artist_links(&catalog, t.credits()),
            albums: t.albums().iter().filter_map(|m| catalog.album(m.0)).map(album_row).collect(),
            genres: t.genres().map(str::to_string).collect(),
            release_date: t.release_date().map(str::to_string),
            status: t.status().to_string(),
            codec: t.codec().map(str::to_string),
            bitrate: t.bitrate(),
            sample_rate: t.sample_rate(),
        })
    }

    pub fn album(&self, id: String) -> Option<AlbumRow> {
        let catalog = self.shared.catalog()?;
        catalog.album(parse_id(&id)?).map(album_row)
    }

    /// An album's tracks, in its order.
    pub fn album_tracks(&self, id: String) -> Vec<TrackRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        catalog.album_tracks(id).iter().map(|&i| track_row(&catalog, catalog.track_at(i))).collect()
    }

    pub fn artist(&self, id: String) -> Option<ArtistRow> {
        let catalog = self.shared.catalog()?;
        let a = catalog.artist(parse_id(&id)?)?;
        Some(ArtistRow { id: a.id.to_string(), name: a.label(), track_count: a.track_count, album_count: a.album_count })
    }

    /// An artist's albums, newest first.
    pub fn artist_albums(&self, id: String) -> Vec<AlbumRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        catalog.artist_albums(id).iter().map(|&i| album_row(catalog.album_at(i))).collect()
    }

    /// The player: what plays and the queue.
    pub fn playback(&self) -> Arc<Playback> {
        Arc::new(Playback { shared: self.shared.clone() })
    }

    // ---- Covers ----

    /// How to fetch a cover (its key from a row): None when logged out.
    pub fn image_request(&self, key: String, size: ImageSize) -> Option<ImageRequest> {
        let session = self.shared.api.session()?;
        let query = match size {
            ImageSize::Small => "?size=small",
            ImageSize::Standard => "?size=standard",
            ImageSize::Original => "",
        };
        let local = self.shared.offline.lock().unwrap().as_ref().and_then(|o| {
            let size = match size {
                ImageSize::Small => "small",
                ImageSize::Standard => "standard",
                ImageSize::Original => return None,
            };
            o.cover_path(&key, size).map(|p| p.to_string_lossy().into_owned())
        });
        Some(ImageRequest {
            url: format!("{}/api/images/{key}{query}", session.server),
            authorization: format!("Bearer {}", session.token),
            local,
        })
    }
}

/// What the account's fetch calls when `me` and the vocabulary have come.
struct AccountWake {
    account: std::sync::OnceLock<std::sync::Weak<Mutex<acetate_core::account::Account>>>,
    listener: Arc<dyn Listener>,
}

impl acetate_core::Wake for AccountWake {
    fn changed(&self) {
        let Some(account) = self.account.get().and_then(|a| a.upgrade()) else { return };
        account.lock().unwrap().poll();
        self.listener.changed(Change::Account);
    }
}

/// What the details' background fetches call when one has come.
struct DetailWake {
    details: std::sync::OnceLock<std::sync::Weak<Mutex<Details<TrackDetail>>>>,
    listener: Arc<dyn Listener>,
}

impl acetate_core::Wake for DetailWake {
    fn changed(&self) {
        let Some(details) = self.details.get().and_then(|d| d.upgrade()) else { return };
        details.lock().unwrap().poll();
        self.listener.changed(Change::Details);
    }
}

/// What the playlists' background work calls when it has an answer: takes it in, passes
/// on what to tell the user, and lets Kotlin know.
struct PlaylistWake {
    playlists: std::sync::OnceLock<std::sync::Weak<Mutex<Playlists>>>,
    listener: Arc<dyn Listener>,
}

impl acetate_core::Wake for PlaylistWake {
    fn changed(&self) {
        let Some(playlists) = self.playlists.get().and_then(|p| p.upgrade()) else { return };
        let notice = {
            let mut playlists = playlists.lock().unwrap();
            playlists.poll();
            playlists.created = None;
            playlists.notice.take().map(|(text, _)| text)
        };
        if let Some(text) = notice {
            self.listener.notice(text);
        }
        self.listener.changed(Change::Playlists);
    }
}

impl Core {
    fn change_mark(&self, on: MarkOn, id: &str, change: impl FnOnce(&mut Mark), star: bool) {
        let Some(id) = parse_id(id) else { return };
        let shared = self.shared.clone();
        let before = shared.marks.lock().unwrap().get(shared.catalog().as_deref(), id);
        let mut mark = before;
        change(&mut mark);
        if mark == before {
            return;
        }
        shared.marks.lock().unwrap().set(id, mark);
        shared.listener.changed(Change::Marks);
        let kind = match on {
            MarkOn::Track => Kind::Track,
            MarkOn::Album => Kind::Album,
            MarkOn::Artist => Kind::Artist,
        };
        self.runtime.spawn(async move {
            let result = if star {
                shared.api.star(kind.path(), id, mark.starred).await
            } else {
                shared.api.rate(kind.path(), id, mark.rating).await
            };
            match result {
                Ok(a) => shared.marks.lock().unwrap().set(id, Mark { play_count: a.play_count, rating: a.rating, starred: a.starred }),
                Err(err) => {
                    // Only what this change did goes back: another may be on its way.
                    let mut marks = shared.marks.lock().unwrap();
                    let mut now = marks.get(shared.catalog().as_deref(), id);
                    if star {
                        now.starred = before.starred;
                    } else {
                        now.rating = before.rating;
                    }
                    marks.set(id, now);
                    drop(marks);
                    shared.listener.notice(format!("{err:#}"));
                }
            }
            shared.listener.changed(Change::Marks);
        });
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work).await.expect("list work doesn't panic")
}

fn set_sync(shared: &Shared, change: impl FnOnce(&mut SyncState)) {
    change(&mut shared.sync.lock().unwrap());
    shared.listener.changed(Change::Sync);
}

fn set_catalog(shared: &Arc<Shared>, catalog: Arc<Catalog>) {
    *shared.catalog.lock().unwrap() = Some(catalog.clone());
    {
        let mut sync = shared.sync.lock().unwrap();
        sync.have_catalog = true;
        sync.tracks = catalog.track_count() as u64;
        sync.albums = catalog.album_count() as u64;
        sync.artists = catalog.artists.len() as u64;
    }
    // The queue from the last run, loaded (paused) if nothing else is.
    shared.player.restore_saved(&catalog);
    shared.listener.changed(Change::Catalog);
    refresh_offline(shared);
    acetate_core::memory::trim();
}

// ---- Downloads ----

/// What the downloads' background work calls when something moved.
struct DownloadWake(Arc<dyn Listener>);

impl acetate_core::Wake for DownloadWake {
    fn changed(&self) {
        self.0.changed(Change::Downloads);
    }
}

/// Works out which files the pins want now (an album's tracks, a playlist's, the liked
/// ones…) and tells the downloads.
fn refresh_offline(shared: &Arc<Shared>) {
    let (Some(offline), Some(catalog)) = (shared.offline.lock().unwrap().clone(), shared.catalog()) else { return };
    let (wants, complete) = acetate_core::offline::wanted(
        &offline.pins(),
        &catalog,
        &mut |id| shared.playlists.lock().unwrap().detail(id).value.as_ref().map(|d| d.entries.iter().map(|e| e.track_id).collect()),
        &|| shared.marks.lock().unwrap().liked(&catalog),
    );
    offline.set_wanted(wants, complete);
}

// ---- Playing ----

/// Tells Kotlin that what plays, or the queue, changed.
struct ChangeWake(Arc<dyn Listener>);

impl acetate_core::Wake for ChangeWake {
    fn changed(&self) {
        self.0.changed(Change::Player);
    }
}

/// A track to hand to the platform's player: where to fetch it (with the login header) and
/// what to show about it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MediaRequest {
    pub url: String,
    pub authorization: String,
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub cover: Option<ImageRequest>,
}

/// The platform's player (Media3's, in a service). The core calls it from any thread: it
/// has to hand each call over to the thread its player lives on.
#[uniffi::export(with_foreign)]
pub trait PlayerEngine: Send + Sync {
    /// Plays this now from `at_ms`, in place of what plays; its next slot is emptied.
    fn play(&self, media: MediaRequest, at_ms: u64, paused: bool);
    /// What plays after the current track, gaplessly (none: nothing).
    fn set_next(&self, media: Option<MediaRequest>);
    fn set_paused(&self, paused: bool);
    fn seek(&self, to_ms: u64);
    /// Goes on to what's in the next slot, now.
    fn skip(&self);
    fn stop(&self);
}

/// Turns the core's calls into the engine's, with addresses and login.
struct EngineAdapter {
    engine: Arc<dyn PlayerEngine>,
    api: Api,
    /// Where the cover of a key is kept on the device (the standard size), if it is.
    kept_cover: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl EngineAdapter {
    fn media(&self, load: &Load) -> Option<MediaRequest> {
        let session = self.api.session()?;
        let item = &load.item;
        // A file on this device plays from there (no login needed for it).
        let (url, authorization) = match &load.local {
            Some(path) => (format!("file://{}", path.display()), String::new()),
            None => (format!("{}{}", session.server, load.path), format!("Bearer {}", session.token)),
        };
        Some(MediaRequest {
            url,
            authorization,
            track_id: item.track_id.to_string(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration_ms: item.duration_ms,
            cover: item.cover.map(|c| {
                let local = (self.kept_cover)(&cover_key(c.key.0));
                ImageRequest {
                    // A kept cover is read from the device (the player can open files).
                    url: match &local {
                        Some(path) => format!("file://{path}"),
                        None => format!("{}/api/images/{}?size=standard", session.server, cover_key(c.key.0)),
                    },
                    authorization: format!("Bearer {}", session.token),
                    local,
                }
            }),
        })
    }
}

impl Engine for EngineAdapter {
    fn play(&self, load: &Load, at: std::time::Duration, paused: bool) {
        if let Some(media) = self.media(load) {
            self.engine.play(media, at.as_millis() as u64, paused);
        }
    }

    fn set_next(&self, load: Option<&Load>) {
        self.engine.set_next(load.and_then(|l| self.media(l)));
    }

    fn set_paused(&self, paused: bool) {
        self.engine.set_paused(paused);
    }

    fn seek(&self, to: std::time::Duration) {
        self.engine.seek(to.as_millis() as u64);
    }

    fn skip(&self) {
        self.engine.skip();
    }

    fn stop(&self) {
        self.engine.stop();
    }
}

/// What the player shows.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PlayerState {
    /// What plays (or last played, or waits, restored).
    pub current: Option<TrackRow>,
    /// It came from the hand-made queue (else from the list).
    pub current_queued: bool,
    pub playing: bool,
    pub paused: bool,
    pub repeat: bool,
    pub shuffle: bool,
    /// Where it was last told to be (the progress bar reads the platform player instead).
    pub position_ms: u64,
    /// The list played from, and where in it playback is.
    pub list_len: u64,
    pub list_index: u64,
    /// Tracks queued by hand.
    pub queued_len: u64,
}

fn queue_row(item: &QueueItem) -> TrackRow {
    TrackRow {
        id: item.track_id.to_string(),
        title: item.title.clone(),
        artist: item.artist.clone(),
        album: item.album.clone(),
        year: None,
        duration_ms: item.duration_ms,
        cover: item.cover.map(|c| cover_key(c.key.0)),
    }
}

/// A place in the queue.
#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum QueuePart {
    /// The list played from.
    List,
    /// The hand-made queue.
    Queued,
}

fn place(part: QueuePart, index: u64) -> queue::Place {
    match part {
        QueuePart::List => queue::Place::List(index as usize),
        QueuePart::Queued => queue::Place::Queued(index as usize),
    }
}

/// What plays and the queue: commands, and what the platform's player tells.
#[derive(uniffi::Object)]
pub struct Playback {
    shared: Arc<Shared>,
}

impl Playback {
    fn player(&self) -> &Player {
        &self.shared.player
    }

    fn items(&self, positions: impl Iterator<Item = u32>) -> Vec<QueueItem> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        positions.map(|i| queue_item(&catalog, i)).collect()
    }

    fn items_of(&self, ids: &[String]) -> Vec<QueueItem> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        ids.iter().filter_map(|id| catalog.track_index(parse_id(id)?)).map(|i| queue_item(&catalog, i)).collect()
    }

    fn album_items(&self, id: &str) -> Vec<QueueItem> {
        let Some(catalog) = self.shared.catalog() else { return vec![] };
        let Some(id) = parse_id(id) else { return vec![] };
        catalog.album_tracks(id).iter().map(|&i| queue_item(&catalog, i)).collect()
    }
}

#[uniffi::export]
impl Playback {
    /// The platform's player is there: the core drives it from now on. Hand over what's
    /// loaded already (a queue brought back at start), paused.
    pub fn attach(&self, engine: Arc<dyn PlayerEngine>) {
        let weak = Arc::downgrade(&self.shared);
        let kept_cover: Arc<dyn Fn(&str) -> Option<String> + Send + Sync> = Arc::new(move |key| {
            let shared = weak.upgrade()?;
            let offline = shared.offline.lock().unwrap().clone()?;
            offline.cover_path(key, "standard").map(|p| p.to_string_lossy().into_owned())
        });
        self.player().attach(Arc::new(EngineAdapter { engine, api: self.shared.api.clone(), kept_cover }));
    }

    pub fn state(&self) -> PlayerState {
        let v = self.player().view();
        PlayerState {
            current: v.current.as_ref().map(queue_row),
            current_queued: v.current_queued,
            playing: v.playing,
            paused: v.paused,
            repeat: v.repeat,
            shuffle: v.shuffle,
            position_ms: v.position_now().as_millis() as u64,
            list_len: v.list.len() as u64,
            list_index: v.list_index as u64,
            queued_len: v.queued.len() as u64,
        }
    }

    /// `n` rows of the list played from, from `start`.
    pub fn list_rows(&self, start: u64, n: u64) -> Vec<TrackRow> {
        let v = self.player().view();
        let start = (start as usize).min(v.list.len());
        let end = start.saturating_add(n as usize).min(v.list.len());
        v.list[start..end].iter().map(queue_row).collect()
    }

    /// The hand-made queue.
    pub fn queued_rows(&self) -> Vec<TrackRow> {
        self.player().view().queued.iter().map(queue_row).collect()
    }

    // ---- What to play ----

    /// Plays a list of tracks from `start` (a table, a search result, as it's shown).
    pub fn play_view(&self, view: Arc<TrackView>, start: u64) {
        let items = self.items(view.list.iter().copied());
        self.player().play_list(items, start as usize);
    }

    /// Plays an album from its `start`th track.
    pub fn play_album(&self, album: String, start: u64) {
        self.player().play_list(self.album_items(&album), start as usize);
    }

    /// Plays one of an extra's audio files (not counted as a listen: it isn't a track).
    pub fn play_extra(&self, file: String, name: String, kind: String, duration_ms: Option<i64>) {
        let Some(file) = parse_id(&file) else { return };
        let item = QueueItem {
            track_id: file,
            file_id: file,
            source: acetate_core::playback::AudioSource::Extra,
            title: name,
            artist: kind,
            album: None,
            duration_ms,
            cover: None,
            codec: None,
            sha256: None,
        };
        self.player().play_list(vec![item], 0);
    }

    /// Plays tracks by id (an artist's, a playlist's) from `start`.
    pub fn play_tracks(&self, ids: Vec<String>, start: u64) {
        self.player().play_list(self.items_of(&ids), start as usize);
    }

    pub fn enqueue_tracks(&self, ids: Vec<String>, first: bool) {
        self.player().enqueue(self.items_of(&ids), first);
    }

    pub fn enqueue_album(&self, album: String, first: bool) {
        self.player().enqueue(self.album_items(&album), first);
    }

    pub fn play_place(&self, part: QueuePart, index: u64) {
        self.player().play_place(place(part, index));
    }

    pub fn remove(&self, part: QueuePart, index: u64) {
        self.player().remove(place(part, index));
    }

    pub fn clear_queued(&self) {
        self.player().clear_queued();
    }

    pub fn move_queued(&self, from: u64, to: u64) {
        self.player().move_queued(from as usize, to as usize);
    }

    // ---- The player ----

    pub fn next(&self) {
        self.player().next();
    }

    pub fn previous(&self) {
        self.player().previous();
    }

    pub fn set_paused(&self, paused: bool) {
        self.player().set_paused(paused);
    }

    pub fn seek(&self, to_ms: u64) {
        self.player().seek(std::time::Duration::from_millis(to_ms));
    }

    pub fn toggle_repeat(&self) {
        self.player().toggle_repeat();
    }

    pub fn toggle_shuffle(&self) {
        self.player().toggle_shuffle();
    }

    pub fn stop(&self) {
        self.player().stop();
    }

    /// Streams copies at this quality (kbps; none: what the server gives), from the next track.
    pub fn set_quality(&self, kbps: Option<u32>) {
        self.player().set_quality(kbps);
    }

    // ---- What the platform's player tells ----

    /// It went on to the next item by itself (not by a skip or a replace the core asked for).
    pub fn on_transition(&self) {
        self.player().on_transition();
    }

    /// Called about twice a second while it plays, and when play/pause changes.
    pub fn on_position(&self, position_ms: u64, playing: bool, paused: bool) {
        self.player().on_position(std::time::Duration::from_millis(position_ms), playing, paused);
    }

    /// Playback ran out (the last track ended).
    pub fn on_ended(&self) {
        self.player().on_ended();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyrics(synced: bool, body: &str) -> acetate_core::details::Lyrics {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(1), "is_official": true, "is_synced": synced, "language": "en",
            "body": body, "revision_id": Uuid::from_u128(2),
        }))
        .unwrap()
    }

    #[test]
    fn synced_lyrics_are_lines_with_times_and_headings_where_a_part_starts() {
        let view = lyrics_view(&lyrics(true, "[Verse 1: A]\n[00:01.50]one\n[00:03.00]two\n[Chorus]\n[00:05.00]three"));
        assert!(view.synced);
        let lines: Vec<_> = view.lines.iter().map(|l| (l.at_ms, l.text.as_str(), l.heading.as_deref())).collect();
        assert_eq!(lines, [(Some(1500), "one", Some("Verse 1 · A")), (Some(3000), "two", None), (Some(5000), "three", Some("Chorus"))]);
    }

    #[test]
    fn plain_lyrics_are_lines_without_times() {
        let view = lyrics_view(&lyrics(false, "[Intro]\nhello\nthere\n\n[Verse]\nbye"));
        let lines: Vec<_> = view.lines.iter().map(|l| (l.at_ms, l.text.as_str(), l.heading.as_deref())).collect();
        assert_eq!(lines, [(None, "hello", Some("Intro")), (None, "there", None), (None, "bye", Some("Verse"))]);
    }
}
