//! The phone's door into `acetate-core`: the only FFI code. UniFFI turns what's marked here
//! into Kotlin (and Swift) bindings; nothing else in the repo knows about them.
//!
//! One `Core` per process. Lists never cross whole: Kotlin asks for a *view* (albums sorted
//! by X, filtered by Y), gets a handle with a count, and reads the rows on screen with
//! `rows(start, n)`. A view keeps the catalog it was built from, so its rows stay
//! consistent; when the catalog is replaced the listener says so and Kotlin asks again.

use std::sync::{Arc, Mutex};

use acetate_core::api::{Api, ApiError};
use acetate_core::catalog::index::{AlbumSort, ArtistSort, TrackRows, TrackSort};
use acetate_core::catalog::store::{self, Store, Update};
use acetate_core::catalog::{Album, Catalog, Track};
use uuid::Uuid;

uniffi::setup_scaffolding!();

// ---- Errors, events ----

/// A request that failed: `message` is a sentence for a person, `code` is for acting on.
#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    Failed { code: String, message: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let CoreError::Failed { message, .. } = self;
        f.write_str(message)
    }
}

impl From<ApiError> for CoreError {
    fn from(err: ApiError) -> Self {
        CoreError::Failed { code: err.code, message: err.message }
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
#[derive(Debug, Clone, uniffi::Record)]
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
        let shared = Arc::new(Shared {
            api: Api::new(data_dir.clone()),
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

    /// Whether the session ended since last asked (a request was refused as not logged in).
    pub fn take_expired(&self) -> bool {
        self.shared.api.take_expired()
    }

    pub async fn login(&self, server: String, username: String, password: String) -> Result<SessionInfo, CoreError> {
        let session = self.shared.api.login(&server, &username, &password).await?;
        self.shared.listener.changed(Change::Session);
        Ok(SessionInfo { server: session.server, username: session.username })
    }

    /// Logs out; the kept catalog stays on disk for the next login of the same user.
    pub fn logout(&self) {
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
    shared.listener.changed(Change::Catalog);
    acetate_core::memory::trim();
}
