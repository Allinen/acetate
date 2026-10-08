//! The core as Kotlin uses it, against a kept catalog on disk (no server: the refresh
//! fails, and the kept copy stays).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use acetate_ffi::*;

struct Events(Mutex<Vec<Change>>);

impl Listener for Events {
    fn changed(&self, what: Change) {
        self.0.lock().unwrap().push(what);
    }
}

const SERVER: &str = "http://127.0.0.1:1";

fn catalog_json() -> String {
    serde_json::json!({
        "format": 8,
        "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate"],
                   "rows": [
            ["00000000-0000-7000-8000-000000000001", "Second", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]],
             [["00000000-0000-7000-8000-0000000000b1", 1, 2]], 1000, "00000000-0000-7000-8000-0000000000f1", null, null, "2020", 2020, "released", [], "", false, "flac"],
            ["00000000-0000-7000-8000-000000000002", "First", "Ann feat. Bo",
             [["00000000-0000-7000-8000-0000000000a1", "primary", null], ["00000000-0000-7000-8000-0000000000a2", "featured", "B.O."]],
             [["00000000-0000-7000-8000-0000000000b1", 1, 1]], 2000, "00000000-0000-7000-8000-0000000000f2", null, null, "2021", 2021, "released", [], "", false, "flac"],
            ["00000000-0000-7000-8000-000000000003", "The Third", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", null]],
             [["00000000-0000-7000-8000-0000000000b2", null, 1], ["00000000-0000-7000-8000-0000000000b1", 1, 3]], 500, "00000000-0000-7000-8000-0000000000f3", null, null, null, null, "released", [], "", false, "mp3"]
        ]},
        "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                   "rows": [
            ["00000000-0000-7000-8000-0000000000b1", "A", "album", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]], "2021", 2021, "00000000-0000-7000-8000-0000000000c1", "00112233445566778899", "", 3, 3500],
            ["00000000-0000-7000-8000-0000000000b2", "B", "single", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", null]], null, null, null, null, "", 1, 500]
        ]},
        "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                    "rows": [
            ["00000000-0000-7000-8000-0000000000a1", "Ann", "", [], 2, 1],
            ["00000000-0000-7000-8000-0000000000a2", "Bo", "", ["B.O."], 2, 1]
        ]},
        "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
        "issues": {"columns": ["track_id", "flags"], "rows": []},
        "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
        "playlists": []
    })
    .to_string()
}

/// A data folder with a session and a kept catalog, and a core on it.
fn core(with_catalog: bool) -> (tempfile::TempDir, Arc<Core>, Arc<Events>) {
    let dir = tempfile::tempdir().unwrap();
    let session = serde_json::json!({ "server": SERVER, "token": "tok", "username": "ann" });
    std::fs::write(dir.path().join("session.json"), session.to_string()).unwrap();
    if with_catalog {
        let key = acetate_core::catalog::store::key(&acetate_core::api::Session {
            server: SERVER.into(),
            token: String::new(),
            username: "ann".into(),
        });
        // Kept the way the store keeps it: a snapshot, generation 1.
        let catalog = acetate_core::catalog::Catalog::build(acetate_core::catalog::model::parse(catalog_json().as_bytes()).unwrap());
        catalog.write_snapshot(&dir.path().join(format!("catalog-{key}.0000000000000001.snap"))).unwrap();
    }
    let events = Arc::new(Events(Mutex::new(vec![])));
    let core = Core::new(dir.path().to_string_lossy().into_owned(), events.clone());
    (dir, core, events)
}

fn synced(core: &Core) {
    core.start_sync();
    let start = Instant::now();
    // Done: the (failed) refresh over, so the state no longer says "refreshing".
    while core.sync_state().refreshing {
        assert!(start.elapsed() < Duration::from_secs(20), "the sync never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

#[test]
fn the_session_is_remembered() {
    let (_dir, core, _) = core(false);
    let session = core.session().unwrap();
    assert_eq!((session.server.as_str(), session.username.as_str()), (SERVER, "ann"));
}

#[test]
fn the_kept_catalog_shows_while_the_server_is_unreachable() {
    let (_dir, core, events) = core(true);
    synced(&core);
    let state = core.sync_state();
    assert!(state.have_catalog);
    assert_eq!((state.tracks, state.albums, state.artists), (3, 2, 2));
    assert!(state.error.is_some(), "the refresh failed and says so");
    assert!(events.0.lock().unwrap().contains(&Change::Catalog));
}

#[test]
fn views_hand_over_only_the_rows_asked_for() {
    let (_dir, core, _) = core(true);
    synced(&core);
    let rt = rt();
    let albums = rt.block_on(core.albums(String::new(), AlbumOrder::Title));
    assert_eq!(albums.count(), 2);
    let rows = albums.rows(0, 10);
    assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(rows[0].cover.as_deref(), Some("0011223344556677"));
    assert_eq!(albums.rows(1, 10).len(), 1);
    assert!(albums.rows(5, 10).is_empty(), "past the end is empty, not a panic");

    let filtered = rt.block_on(core.albums("artist:bo".into(), AlbumOrder::Title));
    assert_eq!(filtered.rows(0, 10).iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["B"]);

    let tracks = rt.block_on(core.tracks(String::new(), TrackOrder::Title, false));
    assert_eq!(tracks.rows(0, 10).iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["First", "Second", "The Third"]);
    assert_eq!(tracks.rows(0, 1)[0].album.as_deref(), Some("A"));

    let artists = rt.block_on(core.artists(String::new(), ArtistOrder::Name));
    assert_eq!(artists.rows(0, 10).iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Ann", "Bo"]);
}

#[test]
fn pages_answer_from_the_catalog() {
    let (_dir, core, _) = core(true);
    synced(&core);
    let album = "00000000-0000-7000-8000-0000000000b1";
    assert_eq!(core.album(album.into()).unwrap().title, "A");
    let titles: Vec<_> = core.album_tracks(album.into()).into_iter().map(|t| t.title).collect();
    assert_eq!(titles, ["First", "Second", "The Third"]);
    let bo = "00000000-0000-7000-8000-0000000000a2";
    assert_eq!(core.artist(bo.into()).unwrap().name, "Bo");
    assert_eq!(core.artist_albums(bo.into()).len(), 1);
    assert!(core.album("not an id".into()).is_none());
}

#[test]
fn without_a_catalog_views_are_empty() {
    let (_dir, core, _) = core(false);
    let rt = rt();
    let albums = rt.block_on(core.albums(String::new(), AlbumOrder::Title));
    assert_eq!(albums.count(), 0);
    assert!(albums.rows(0, 10).is_empty());
}

#[test]
fn covers_are_fetched_with_the_login() {
    let (_dir, core, _) = core(false);
    let request = core.image_request("0011223344556677".into(), ImageSize::Small).unwrap();
    assert_eq!(request.url, format!("{SERVER}/api/images/0011223344556677?size=small"));
    assert_eq!(request.authorization, "Bearer tok");
    core.logout();
    assert!(core.image_request("0011223344556677".into(), ImageSize::Small).is_none());
    assert!(core.session().is_none());
}

// ---- Playing ----

#[derive(Default)]
struct Engine(Mutex<Vec<String>>);

impl Engine {
    fn took(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl PlayerEngine for Engine {
    fn play(&self, media: MediaRequest, at_ms: u64, paused: bool) {
        self.0.lock().unwrap().push(format!("play {} {} @{at_ms}{} [{}]", media.title, media.url, if paused { " paused" } else { "" }, media.authorization));
    }
    fn set_next(&self, media: Option<MediaRequest>) {
        self.0.lock().unwrap().push(format!("next {}", media.map_or("-".to_string(), |m| m.title)));
    }
    fn set_paused(&self, paused: bool) {
        self.0.lock().unwrap().push(format!("paused {paused}"));
    }
    fn seek(&self, to_ms: u64) {
        self.0.lock().unwrap().push(format!("seek {to_ms}"));
    }
    fn skip(&self) {
        self.0.lock().unwrap().push("skip".into());
    }
    fn stop(&self) {
        self.0.lock().unwrap().push("stop".into());
    }
}

const ALBUM: &str = "00000000-0000-7000-8000-0000000000b1";

#[test]
fn an_album_plays_through_the_engine_with_the_login() {
    let (_dir, core, events) = core(true);
    synced(&core);
    let playback = core.playback();
    let engine = Arc::new(Engine::default());
    playback.attach(engine.clone());
    playback.play_album(ALBUM.into(), 0);
    let took = engine.took();
    assert_eq!(took[0], format!("play First {SERVER}/api/files/00000000-0000-7000-8000-0000000000f2/stream @0 [Bearer tok]"));
    assert_eq!(took[1], "next Second");
    assert!(events.0.lock().unwrap().contains(&Change::Player));

    let state = playback.state();
    assert_eq!(state.current.unwrap().title, "First");
    assert_eq!((state.list_len, state.list_index, state.queued_len, state.playing), (3, 0, 0, true));

    playback.on_transition();
    assert_eq!(playback.state().current.unwrap().title, "Second");
    assert_eq!(engine.took(), ["next The Third"]);
    assert_eq!(playback.list_rows(1, 5).iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Second", "The Third"]);

    playback.enqueue_tracks(vec!["00000000-0000-7000-8000-000000000001".into(), "nonsense".into()], false);
    assert_eq!(playback.queued_rows().len(), 1, "an id that isn't there is left out");
    playback.set_quality(Some(128));
    playback.seek(5000);
    playback.stop();
    assert!(playback.state().current.is_none());
}

#[test]
fn a_view_plays_as_it_is_shown() {
    let (_dir, core, _) = core(true);
    synced(&core);
    let playback = core.playback();
    let engine = Arc::new(Engine::default());
    playback.attach(engine.clone());
    let view = rt().block_on(core.tracks(String::new(), TrackOrder::Title, false));
    playback.play_view(view, 1);
    assert_eq!(playback.state().current.unwrap().title, "Second");
    assert_eq!(playback.state().list_len, 3);
}

#[test]
fn the_queue_comes_back_after_a_restart_paused() {
    let (dir, core1, _) = core(true);
    synced(&core1);
    let playback = core1.playback();
    playback.attach(Arc::new(Engine::default()));
    playback.play_album(ALBUM.into(), 1);
    playback.on_position(12_000, true, false);
    // Written by tasks of their own: wait for both files.
    let written = |prefix: &str| std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(prefix));
    let start = Instant::now();
    while !(written("queue-") && written("list-")) {
        assert!(start.elapsed() < Duration::from_secs(5), "the queue was never kept");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(playback);
    drop(core1);

    let events = Arc::new(Events(Mutex::new(vec![])));
    let core2 = Core::new(dir.path().to_string_lossy().into_owned(), events);
    synced(&core2);
    let playback = core2.playback();
    assert_eq!(playback.state().current.unwrap().title, "Second", "the second track of the album");
    let engine = Arc::new(Engine::default());
    playback.attach(engine.clone());
    let took = engine.took();
    assert!(took[0].starts_with("play Second ") && took[0].contains("paused"), "{took:?}");
    assert_eq!(took[1], "next The Third");
}
