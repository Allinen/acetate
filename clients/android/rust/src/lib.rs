//! The phone's door into `acetate-core`: the only FFI code. UniFFI turns what's marked here
//! into Kotlin (and Swift) bindings; nothing else in the repo knows about them.
//!
//! One `Core` per process. Lists never cross whole: Kotlin asks for a *view* (albums sorted
//! by X, filtered by Y), gets a handle with a count, and reads the rows on screen with
//! `rows(start, n)`. A view keeps the catalog it was built from, so its rows stay
//! consistent; when the catalog is replaced the listener says so and Kotlin asks again.

use std::sync::{Arc, Mutex};

use acetate_core::api::{Api, ApiError};
use acetate_core::playback::player::{Engine, Load, Player};
use acetate_core::playback::{QueueItem, queue, queue_item};
use acetate_core::resume::Resume;
use acetate_core::catalog::index::{AlbumSort, ArtistSort, TrackRows, TrackSort};
use acetate_core::catalog::store::{self, Store, Update};
use acetate_core::catalog::{Album, Catalog, Track};
use uuid::Uuid;

uniffi::setup_scaffolding!();

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
}

/// Kotlin code the core calls, from any thread.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn changed(&self, what: Change);
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
        let shared = Arc::new(Shared {
            api,
            player,
            keeping: Mutex::new(None),
            store: Mutex::new(None),
            catalog: Mutex::new(None),
            sync: Mutex::new(SyncState::default()),
            listener,
        });
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
            *keeping = Some(key);
        }
        drop(keeping);
        let shared = self.shared.clone();
        set_sync(&shared, |s| {
            s.refreshing = true;
            s.error = None;
        });
        self.runtime.spawn(async move {
            let report = |update: Update| match update {
                Update::Loaded { catalog, .. } => {
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

    // ---- Pages ----

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
        Some(ImageRequest {
            url: format!("{}/api/images/{key}{query}", session.server),
            authorization: format!("Bearer {}", session.token),
        })
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work).await.expect("list work doesn't panic")
}

fn set_sync(shared: &Shared, change: impl FnOnce(&mut SyncState)) {
    change(&mut shared.sync.lock().unwrap());
    shared.listener.changed(Change::Sync);
}

fn set_catalog(shared: &Shared, catalog: Arc<Catalog>) {
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
    acetate_core::memory::trim();
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
}

impl EngineAdapter {
    fn media(&self, load: &Load) -> Option<MediaRequest> {
        let session = self.api.session()?;
        let item = &load.item;
        Some(MediaRequest {
            url: format!("{}{}", session.server, load.path),
            authorization: format!("Bearer {}", session.token),
            track_id: item.track_id.to_string(),
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            duration_ms: item.duration_ms,
            cover: item.cover.map(|c| ImageRequest {
                url: format!("{}/api/images/{}?size=standard", session.server, cover_key(c.key.0)),
                authorization: format!("Bearer {}", session.token),
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
        self.player().attach(Arc::new(EngineAdapter { engine, api: self.shared.api.clone() }));
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
