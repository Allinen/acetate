//! The window. `App` holds the pages' states and a `Shell` with what every page uses:
//! the catalog, covers, playback. Pages don't change each other or start playback
//! themselves: they push `Action`s, which the app carries out after the frame. The
//! keyboard (and later the command palette) do the same, so there's one way to do each
//! thing.
//!
//! Keys, see `keys`: Ctrl+1–5 (anywhere) or 1–5 (outside text fields): albums, artists,
//! tracks, now playing, queue · q add to the queue · Shift+Q play next · Esc / Alt+← back · Alt+→ forward · / filter · Space pause · Ctrl+←/→
//! previous/next · Shift+←/→ seek 10 s · s star the selected one · Shift+S star what
//! plays · i the selected track's page · +/− rate the selected one · r loop the track · z shuffle · F5 refresh the library · arrows or hjkl move ·
//! Enter open or play · Shift+Enter play.

pub mod covers;
pub mod theme;
pub mod widgets;
mod views;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Instant, SystemTime};

use eframe::egui::{self, Frame, Key, Margin, Stroke};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::api::Api;
use crate::catalog::marks::{Kind, Mark, Marks};
use crate::catalog::model::Cover;
use crate::catalog::store::{self, Store, Update};
use crate::catalog::{Cached, Catalog};
use crate::playback::spectrum::Spectrum;
use crate::playback::{Controller, PlayerView, QueueItem};
use covers::{ease_out, Covers};
use theme::PALETTE as P;

/// A place in the app. Pages with an id show that album or artist.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Albums,
    Artists,
    Tracks,
    Album(Uuid),
    Artist(Uuid),
    NowPlaying,
    /// What plays, what's next, what played.
    Queue,
    /// The starred tracks (a playlist that's always there).
    Liked,
    Playlist(Uuid),
    /// Everything about one track.
    Track(Uuid),
}

/// Something to do, asked for by a page, a key or a button.
pub enum Action {
    Go(Page),
    Back,
    Forward,
    /// Play these tracks (positions in the catalog), starting with `start`.
    Play { tracks: Vec<u32>, start: usize },
    /// Queue these tracks by hand: at the end of the queue, or (`next`) at its start.
    Enqueue { tracks: Vec<u32>, next: bool },
    /// Play this track of the queue or the list now.
    PlayPlace(crate::playback::queue::Place),
    /// Take it out of the queue or the list.
    RemovePlace(crate::playback::queue::Place),
    /// Move a queued track to another place in the queue.
    MoveQueued(usize, usize),
    ClearQueued,
    TogglePause,
    Next,
    Previous,
    /// Seek by this many seconds (negative: back).
    SeekBy(f32),
    /// Seek to this point of the current track.
    SeekTo(std::time::Duration),
    /// Loop the current track (or stop looping it).
    ToggleRepeat,
    ToggleShuffle,
    /// Star it, or unstar it if it's starred.
    ToggleStar(Kind, Uuid),
    /// Set its rating (0 to 10, half stars).
    Rate(Kind, Uuid, i16),
    /// Change its rating by this many half stars.
    RateBy(Kind, Uuid, i16),
    /// Choose a playlist to add these tracks to (or make one).
    AddToPlaylist(Vec<Uuid>),
    NewPlaylist,
    RenamePlaylist(Uuid),
    RefreshLibrary,
    LoggedIn,
    LogOut,
}

/// What every page uses.
pub struct Shell {
    /// For waking the UI from background work.
    pub ctx: egui::Context,
    /// The logo, as a texture.
    pub logo: egui::TextureHandle,
    pub runtime: Runtime,
    pub api: Api,
    pub controller: Arc<Controller>,
    pub covers: Covers,
    pub catalog: Cached<Catalog>,
    /// Counts catalog replacements: a page's lists are rebuilt when it changes.
    pub generation: u64,
    pub actions: Vec<Action>,
    pub data_dir: PathBuf,
    /// Stars, ratings and plays as shown (the catalog's, with this session's changes).
    pub marks: Marks,
    /// The queue kept between runs (set at login), and whether it was brought back yet.
    resume: Option<crate::resume::Resume>,
    resumed: bool,
    /// What each changing cover place shows (see `cover_crossfade`).
    cover_slots: std::collections::HashMap<&'static str, CoverSlot>,
    pub playlists: crate::playlists::Playlists,
    /// Everything about one track, album or artist, for their pages.
    pub track_details: crate::details::Details<crate::details::TrackDetail>,
    pub album_details: crate::details::Details<crate::details::AlbumDetail>,
    pub artist_details: crate::details::Details<crate::details::ArtistDetail>,
    updates: (mpsc::Sender<Update>, mpsc::Receiver<Update>),
    store: Option<Arc<Store>>,
    /// When the catalog being loaded was asked for.
    refresh_asked: Option<Instant>,
    /// The server's answers to mark changes: (id, the mark it has, or an error with the
    /// mark to go back to).
    mark_answers: Arc<Mutex<Vec<(Uuid, Result<Mark, (String, Mark)>)>>>,
}

impl Shell {
    pub fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// The user's star, rating and play count on a track, album or artist.
    pub fn mark(&self, id: Uuid) -> Mark {
        self.marks.get(self.catalog.value.as_deref(), id)
    }

    /// Shows `mark` at once and sends the change to the server.
    fn change_mark(&mut self, kind: Kind, id: Uuid, mark: Mark) {
        let before = self.mark(id);
        if mark == before {
            return;
        }
        self.marks.set(id, mark);
        let (api, answers, ctx) = (self.api.clone(), self.mark_answers.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let result = if mark.starred != before.starred {
                api.star(kind.path(), id, mark.starred).await
            } else {
                api.rate(kind.path(), id, mark.rating).await
            };
            let answer = result
                .map(|a| Mark { play_count: a.play_count, rating: a.rating, starred: a.starred })
                .map_err(|err| (format!("{err:#}"), before));
            answers.lock().unwrap().push((id, answer));
            ctx.request_repaint();
        });
    }

    fn take_mark_answers(&mut self) {
        // Plays counted by playback: the answer has the new play count.
        for (track, answer) in std::mem::take(&mut *self.controller.played.lock().unwrap()) {
            match answer {
                Ok(a) => self.marks.set(track, Mark { play_count: a.play_count, rating: a.rating, starred: a.starred }),
                Err(err) => self.controller.report(format!("couldn't count the play: {err}")),
            }
        }
        for (id, answer) in std::mem::take(&mut *self.mark_answers.lock().unwrap()) {
            match answer {
                Ok(mark) => self.marks.set(id, mark),
                Err((err, before)) => {
                    self.marks.set(id, before);
                    self.controller.report(err);
                }
            }
        }
    }

    /// A cover, with a placeholder (a small disc on a color made from `seed`) until it
    /// has loaded, fading in. `lift`: 0 to 1, how raised it looks (its shadow).
    pub fn cover(&mut self, ui: &egui::Ui, cover: Option<Cover>, seed: Uuid, rect: egui::Rect, radius: u8, lift: f32) {
        let painter = ui.painter();
        painter.add(
            egui::Shadow {
                offset: [0, (4.0 + lift * 10.0) as i8],
                blur: (10.0 + lift * 22.0) as u8,
                spread: 0,
                color: egui::Color32::from_black_alpha((90.0 + lift * 70.0) as u8),
            }
            .as_shape(rect, radius),
        );
        let loaded = cover.and_then(|c| self.covers.get(ui.ctx(), c));
        let fade = loaded.as_ref().map(|(_, f)| *f).unwrap_or(0.0);
        if fade < 1.0 {
            widgets::placeholder(ui.painter(), rect, radius, seed);
        }
        if let Some((texture, fade)) = loaded {
            egui::Image::from_texture(&texture)
                .corner_radius(radius)
                .tint(egui::Color32::from_white_alpha((fade * 255.0) as u8))
                .paint_at(ui, rect);
        }
    }

    /// Keeps the queue for next time, and brings back last time's once the library is
    /// here (paused, where it was; tracks no longer in the library are left out).
    fn resume_queue(&mut self, player: &PlayerView) {
        let Some(resume) = self.resume.as_mut() else { return };
        if self.resumed {
            resume.save(player, self.runtime.handle());
            return;
        }
        let Some(catalog) = self.catalog.value.clone() else { return };
        self.resumed = true;
        if player.current.is_some() {
            return;
        }
        let Some((list, saved, position)) = resume.load() else { return };
        let current = saved.current_queued.or_else(|| list.tracks.get(saved.index).copied());
        let at = position
            .filter(|p| Some(p.track) == current)
            .map(|p| std::time::Duration::from_millis(p.position_ms))
            .unwrap_or_default();
        let controller = self.controller.clone();
        // Built in the background: it can be the whole library.
        self.runtime.spawn(async move {
            let items = |ids: &[Uuid]| -> Vec<QueueItem> {
                ids.iter().filter_map(|id| catalog.track_index(*id)).map(|i| queue_item(&catalog, i)).collect()
            };
            let list_track = list.tracks.get(saved.index).copied();
            let list = items(&list.tracks);
            let index = list_track.and_then(|id| list.iter().position(|i| i.track_id == id)).unwrap_or(0);
            let current_queued = saved.current_queued.and_then(|id| catalog.track_index(id)).map(|i| queue_item(&catalog, i));
            let queued = items(&saved.queued);
            controller.restore(list, index, queued, current_queued, at, saved.repeat, saved.shuffle).await
        });
    }

    /// Like `cover`, for a place whose cover changes (what plays): the old one fades out
    /// as the new one fades in, and stays until the new one has loaded. `slot` names the
    /// place ("player-bar"), each remembering its own.
    #[allow(clippy::too_many_arguments)]
    pub fn cover_crossfade(&mut self, ui: &egui::Ui, slot: &'static str, cover: Option<Cover>, seed: Uuid, rect: egui::Rect, radius: u8, lift: f32) {
        let now = ui.input(|i| i.time);
        let state = self.cover_slots.entry(slot).or_insert(CoverSlot { shown: cover, seed, previous: None, since: now });
        if state.shown.map(|c| c.key) != cover.map(|c| c.key) {
            // The old one fades out from the moment the new one is there.
            state.previous = state.shown.map(|c| (c, state.seed));
            state.shown = cover;
            state.seed = seed;
            state.since = f64::INFINITY;
        }
        let state = *state;
        let new = cover.and_then(|c| self.covers.get(ui.ctx(), c));
        let old = state.previous.and_then(|(c, _)| self.covers.get(ui.ctx(), c));
        if new.is_some() && state.since.is_infinite() {
            self.cover_slots.get_mut(slot).unwrap().since = now;
        }
        let t = if new.is_some() { ease_out((((now - state.since) as f32) / 0.4).clamp(0.0, 1.0)) } else { 0.0 };
        let painter = ui.painter();
        painter.add(
            egui::Shadow {
                offset: [0, (4.0 + lift * 10.0) as i8],
                blur: (10.0 + lift * 22.0) as u8,
                spread: 0,
                color: egui::Color32::from_black_alpha((90.0 + lift * 70.0) as u8),
            }
            .as_shape(rect, radius),
        );
        match &old {
            Some((texture, _)) if t < 1.0 => {
                egui::Image::from_texture(texture).corner_radius(radius).paint_at(ui, rect);
                ui.ctx().request_repaint();
            }
            None if t < 1.0 => widgets::placeholder(ui.painter(), rect, radius, seed),
            _ => {}
        }
        if let Some((texture, _)) = new {
            egui::Image::from_texture(&texture)
                .corner_radius(radius)
                .tint(egui::Color32::from_white_alpha((t * 255.0) as u8))
                .paint_at(ui, rect);
            if t < 1.0 {
                ui.ctx().request_repaint();
            }
        }
    }

    /// Starts loading the library: the copy on disk first, then the server's.
    fn load_library(&mut self, ctx: &egui::Context) {
        let Some(session) = self.api.session() else { return };
        let store = Arc::new(Store::new(&self.data_dir, &session));
        self.store = Some(store.clone());
        self.playlists.reset(Some(self.data_dir.join(format!("playlists-{}", store::key(&session)))));
        self.resume = Some(crate::resume::Resume::new(&self.data_dir, &store::key(&session)));
        self.resumed = false;
        let details = self.data_dir.join(format!("details-{}", store::key(&session)));
        self.track_details.reset(Some(details.clone()));
        self.album_details.reset(Some(details.clone()));
        self.artist_details.reset(Some(details));
        self.catalog = Cached { refreshing: true, ..Cached::default() };
        self.refresh_asked = Some(Instant::now());
        let (sender, api, ctx) = (self.updates.0.clone(), self.api.clone(), ctx.clone());
        self.runtime.spawn(store::load_and_refresh(store, api, move |update| {
            let _ = sender.send(update);
            ctx.request_repaint();
        }));
    }

    fn refresh_library(&mut self, ctx: &egui::Context) {
        let Some(store) = self.store.clone() else { return };
        if self.catalog.refreshing {
            return;
        }
        self.catalog.refreshing = true;
        self.refresh_asked = Some(Instant::now());
        let have_copy = self.catalog.value.is_some();
        let (sender, api, ctx) = (self.updates.0.clone(), self.api.clone(), ctx.clone());
        self.runtime.spawn(async move {
            store::refresh(&store, &api, have_copy, &move |update| {
                let _ = sender.send(update);
                ctx.request_repaint();
            })
            .await;
        });
    }

    /// Takes in what the library loading reported.
    fn take_updates(&mut self) {
        while let Ok(update) = self.updates.1.try_recv() {
            match update {
                Update::Loaded { catalog, from_server } => {
                    self.catalog.value = Some(catalog);
                    self.generation += 1;
                    if from_server {
                        if let Some(asked) = self.refresh_asked {
                            self.marks.catalog_arrived(asked);
                            self.playlists.catalog_arrived(asked);
                        }
                        self.catalog.refreshing = false;
                        self.catalog.confirmed_at = Some(SystemTime::now());
                        self.catalog.error = None;
                    }
                }
                Update::Unchanged => {
                    self.catalog.refreshing = false;
                    self.catalog.confirmed_at = Some(SystemTime::now());
                    self.catalog.error = None;
                }
                Update::Failed(err) => {
                    self.catalog.refreshing = false;
                    self.catalog.error = Some(err);
                }
            }
        }
    }

    /// A line about the library for the sidebar.
    pub fn library_status(&self) -> String {
        let count = self.catalog.value.as_ref().map(|c| format!("{} tracks", thousands(c.tracks.len())));
        let state = if self.catalog.refreshing {
            "checking for changes…".to_string()
        } else if let Some(err) = &self.catalog.error {
            format!("offline copy ({err})")
        } else if self.catalog.confirmed_at.is_some() {
            "up to date".to_string()
        } else {
            String::new()
        };
        match count {
            Some(count) if !state.is_empty() => format!("{count} · {state}"),
            Some(count) => count,
            None if self.catalog.refreshing => "Loading the library…".into(),
            None => state,
        }
    }
}

/// A place whose cover changes: what it shows, what it showed before (fading out), and
/// since when the new one fades in (infinite: not loaded yet).
#[derive(Clone, Copy)]
struct CoverSlot {
    shown: Option<Cover>,
    seed: Uuid,
    previous: Option<(Cover, Uuid)>,
    since: f64,
}

pub struct App {
    shell: Shell,
    spectrum: Spectrum,
    last_time: f64,
    pages: views::Pages,

    page: Page,
    back: Vec<Page>,
    forward: Vec<Page>,
    page_since: f64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> App {
        theme::apply(&cc.egui_ctx);
        let runtime = Runtime::new().expect("a tokio runtime");
        let data_dir = dirs::data_dir().unwrap_or_default().join("dev.acetate.desktop");
        let cache_dir = dirs::cache_dir().unwrap_or_default().join("dev.acetate.desktop");
        let api = Api::new(data_dir.clone());
        let controller = runtime.block_on(async { Controller::new(api.clone(), cc.egui_ctx.clone()) });
        runtime.spawn(controller.clone().monitor());
        controller.attach_media_keys(runtime.handle().clone(), cache_dir.join("covers"));
        let covers = Covers::new(api.clone(), runtime.handle().clone(), cache_dir);
        let playlists = crate::playlists::Playlists::new(api.clone(), runtime.handle().clone(), cc.egui_ctx.clone());
        let details = |kind| (kind, api.clone(), runtime.handle().clone(), cc.egui_ctx.clone());
        let (k, a, r, c) = details("tracks");
        let track_details = crate::details::Details::new(k, a, r, c);
        let (k, a, r, c) = details("albums");
        let album_details = crate::details::Details::new(k, a, r, c);
        let (k, a, r, c) = details("artists");
        let artist_details = crate::details::Details::new(k, a, r, c);
        let mut shell = Shell {
            ctx: cc.egui_ctx.clone(),
            logo: widgets::logo_texture(&cc.egui_ctx),
            runtime,
            api,
            controller,
            covers,
            catalog: Cached::default(),
            generation: 0,
            actions: Vec::new(),
            data_dir,
            marks: Marks::default(),
            cover_slots: Default::default(),
            resume: None,
            resumed: false,
            playlists,
            track_details,
            album_details,
            artist_details,
            updates: mpsc::channel(),
            store: None,
            refresh_asked: None,
            mark_answers: Arc::default(),
        };
        if shell.api.session().is_some() {
            shell.load_library(&cc.egui_ctx);
        }
        App {
            shell,
            spectrum: Spectrum::new(64),
            last_time: 0.0,
            pages: views::Pages::default(),
            page: Page::Albums,
            back: Vec::new(),
            forward: Vec::new(),
            page_since: 0.0,
        }
    }

    fn go(&mut self, page: Page, now: f64) {
        if page != self.page {
            self.back.push(self.page);
            self.forward.clear();
            self.page = page;
            self.page_since = now;
            self.pages.opened(page);
            // Details shown as kept, and fetched again (cache first).
            match page {
                Page::Track(id) => self.shell.track_details.open(id),
                Page::Album(id) => self.shell.album_details.open(id),
                Page::Artist(id) => self.shell.artist_details.open(id),
                _ => {}
            }
        }
    }

    /// Carries out what the frame asked for.
    fn apply(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for action in std::mem::take(&mut self.shell.actions) {
            match action {
                Action::Go(page) => self.go(page, now),
                Action::Back => {
                    if let Some(page) = self.back.pop() {
                        self.forward.push(self.page);
                        self.page = page;
                        self.page_since = now;
                    }
                }
                Action::Forward => {
                    if let Some(page) = self.forward.pop() {
                        self.back.push(self.page);
                        self.page = page;
                        self.page_since = now;
                    }
                }
                Action::Play { tracks, start } => self.play(tracks, start),
                Action::Enqueue { tracks, next } => {
                    let Some(catalog) = self.shell.catalog.value.clone() else { continue };
                    let (controller, count) = (self.shell.controller.clone(), tracks.len());
                    self.shell.runtime.spawn(async move {
                        let items: Vec<QueueItem> = tracks.iter().map(|&i| queue_item(&catalog, i)).collect();
                        controller.enqueue(items, next).await
                    });
                    let what = if count == 1 { "1 track".to_string() } else { format!("{count} tracks") };
                    let text = if next { format!("Queued {what} first") } else { format!("Queued {what}") };
                    self.shell.playlists.notice = Some((text, Instant::now()));
                }
                Action::PlayPlace(place) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.play_place(place).await });
                }
                Action::RemovePlace(place) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.remove(place).await });
                }
                Action::ClearQueued => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.clear_queued().await });
                }
                Action::MoveQueued(from, to) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.move_queued(from, to).await });
                }
                Action::TogglePause => self.shell.controller.toggle_pause(),
                Action::Next => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.next().await });
                }
                Action::Previous => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.previous().await });
                }
                Action::SeekBy(seconds) => self.shell.controller.seek_by(seconds),
                Action::SeekTo(at) => self.shell.controller.seek(at),
                Action::ToggleRepeat => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.toggle_repeat().await });
                }
                Action::ToggleShuffle => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.toggle_shuffle().await });
                }
                Action::ToggleStar(kind, id) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { starred: !mark.starred, ..mark });
                }
                Action::Rate(kind, id, rating) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { rating: rating.clamp(0, 10), ..mark });
                }
                Action::RateBy(kind, id, by) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { rating: (mark.rating + by).clamp(0, 10), ..mark });
                }
                Action::AddToPlaylist(tracks) => self.pages.palette.open(ctx, views::palette::Mode::AddTo(tracks)),
                Action::NewPlaylist => self.pages.palette.open(ctx, views::palette::Mode::Name(None)),
                Action::RenamePlaylist(id) => self.pages.palette.open(ctx, views::palette::Mode::Name(Some(id))),
                Action::RefreshLibrary => self.shell.refresh_library(ctx),
                Action::LoggedIn => {
                    self.shell.load_library(ctx);
                    self.page = Page::Albums;
                    self.back.clear();
                    self.forward.clear();
                }
                Action::LogOut => {
                    self.shell.api.logout();
                    self.shell.catalog = Cached::default();
                    self.shell.marks.clear();
                    self.shell.playlists.reset(None);
                    self.shell.resume = None;
                    self.shell.track_details.reset(None);
                    self.shell.album_details.reset(None);
                    self.shell.artist_details.reset(None);
                    self.shell.store = None;
                    self.shell.generation += 1;
                }
            }
        }
    }

    /// Queues tracks from the catalog and starts playing.
    fn play(&mut self, tracks: Vec<u32>, start: usize) {
        let Some(catalog) = self.shell.catalog.value.clone() else { return };
        let controller = self.shell.controller.clone();
        // Built in the background: a queue of the whole library is 185k items.
        self.shell.runtime.spawn(async move {
            let items: Vec<QueueItem> = tracks.iter().map(|&i| queue_item(&catalog, i)).collect();
            controller.play(items, start).await
        });
    }

    /// Keys that work everywhere (outside text fields).
    fn keys(&mut self, ctx: &egui::Context) {
        let m = ctx.input(|i| i.modifiers);
        let key = |k: Key| ctx.input(|i| i.key_pressed(k));
        let pages = [
            (Key::Num1, Page::Albums),
            (Key::Num2, Page::Artists),
            (Key::Num3, Page::Tracks),
            (Key::Num4, Page::NowPlaying),
            (Key::Num5, Page::Queue),
        ];

        // Everywhere, text fields included.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
            self.pages.palette.toggle(ctx);
        }
        if key(Key::F5) {
            self.shell.act(Action::RefreshLibrary);
        }
        if m.alt && key(Key::ArrowLeft) {
            self.shell.act(Action::Back);
        } else if m.alt && key(Key::ArrowRight) {
            self.shell.act(Action::Forward);
        }
        if m.command {
            for (k, page) in pages {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k)) {
                    self.shell.act(Action::Go(page));
                    // Leave the field, so the page's keys work right away.
                    ctx.memory_mut(|mem| mem.stop_text_input());
                }
            }
        }

        // Outside text fields (there the arrows and letters are for the text).
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if m.ctrl && key(Key::ArrowLeft) {
            self.shell.act(Action::Previous);
        } else if m.ctrl && key(Key::ArrowRight) {
            self.shell.act(Action::Next);
        } else if m.shift && key(Key::ArrowLeft) {
            self.shell.act(Action::SeekBy(-10.0));
        } else if m.shift && key(Key::ArrowRight) {
            self.shell.act(Action::SeekBy(10.0));
        }
        if m.shift && key(Key::S) {
            // Star what plays.
            // (Read first: the lock is released at the end of this statement.)
            let playing = playing_track(&self.shell.controller.view.lock().unwrap());
            if let Some(track) = playing {
                self.shell.act(Action::ToggleStar(Kind::Track, track));
            }
        }
        if m.ctrl || m.alt || m.command {
            return;
        }
        for (k, page) in pages {
            if key(k) {
                self.shell.act(Action::Go(page));
            }
        }
        if key(Key::Escape) || key(Key::Backspace) {
            self.shell.act(Action::Back);
        } else if key(Key::Space) {
            self.shell.act(Action::TogglePause);
        } else if !m.shift && key(Key::R) {
            self.shell.act(Action::ToggleRepeat);
        } else if !m.shift && key(Key::Z) {
            self.shell.act(Action::ToggleShuffle);
        } else if key(Key::Slash) {
            // Swallow the "/" itself, or the field would get it as text this frame.
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Slash);
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
            });
            ctx.memory_mut(|mem| mem.request_focus(egui::Id::new(views::FILTER_ID)));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ((now - self.last_time) as f32).clamp(0.0, 0.1);
        self.last_time = now;
        self.shell.covers.poll(&ctx);
        self.shell.take_updates();
        self.pages.login.poll(&mut self.shell);

        let Some(session) = self.shell.api.session() else {
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(P.bg))
                .show(ui, |ui| self.pages.login.show(ui, &mut self.shell));
            self.apply(&ctx);
            return;
        };
        self.keys(&ctx);
        self.shell.take_mark_answers();
        self.shell.playlists.poll();
        self.shell.track_details.poll();
        self.shell.album_details.poll();
        self.shell.artist_details.poll();
        if let Some(id) = self.shell.playlists.created.take() {
            self.shell.act(Action::Go(Page::Playlist(id)));
        }
        let player = self.shell.controller.view.lock().unwrap().clone();
        self.shell.resume_queue(&player);
        // The next track's cover, loaded ahead, so it can fade in the moment it plays.
        if let Some(next) = player.next().and_then(|i| i.cover) {
            self.shell.covers.get(&ctx, next);
        }

        egui::Panel::bottom("player")
            .exact_size(84.0)
            .frame(Frame::NONE.fill(P.sidebar).inner_margin(Margin::symmetric(18, 10)).stroke(Stroke::new(1.0, P.line)))
            .show(ui, |ui| views::player_bar::show(ui, &mut self.shell, &player));

        egui::Panel::left("sidebar")
            .exact_size(212.0)
            .resizable(false)
            .frame(Frame::NONE.fill(P.sidebar).inner_margin(Margin::symmetric(14, 18)))
            .show(ui, |ui| views::sidebar::show(ui, &mut self.shell, self.page, &session.username));

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(P.bg).inner_margin(Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                // The page slides and fades in when switched.
                let t = ease_out((((now - self.page_since) as f32) / 0.24).clamp(0.0, 1.0));
                if t < 1.0 {
                    ctx.request_repaint();
                }
                ui.multiply_opacity(t);
                ui.add_space((1.0 - t) * 16.0);
                self.pages.show(ui, &mut self.shell, self.page, &player, &mut self.spectrum, dt);
            });
        self.pages.palette.show(&ctx, &mut self.shell);
        self.apply(&ctx);
    }
}

/// A track (a position in the catalog) as the queue keeps it.
pub fn queue_item(catalog: &Catalog, i: u32) -> QueueItem {
    let t = &catalog.tracks[i as usize];
    QueueItem {
        track_id: t.id,
        file_id: t.file_id,
        title: t.title.clone(),
        artist: t.artist_display.clone(),
        album: catalog.main_album(t).map(|a| a.title.clone()),
        duration_ms: t.duration_ms,
        cover: t.cover(),
    }
}

/// 185000 → "185,000".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What plays right now, if anything (for pages marking the playing track).
pub fn playing_track(player: &PlayerView) -> Option<Uuid> {
    player.current().map(|c| c.track_id)
}
