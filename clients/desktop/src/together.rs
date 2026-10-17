//! Listening together, on the desktop: keeps this player in step with a session while one is
//! joined, and tells the server what plays (and lists what others play) when asked to. What
//! decides is in `acetate_core::together`; this reads the player, applies what it says and
//! sends what the user changed.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use tokio::task::JoinHandle;
use uuid::Uuid;

use acetate_core::playback::AudioSource;
use acetate_core::together::{self as core, Change, Listening, Local, Room, Synced};

use crate::api::Api;
use crate::catalog::Catalog;
use crate::playback::{Controller, queue_item};

/// The server holds at most this many tracks in a session's list.
const MAX_TRACKS: usize = 10_000;
/// How often the player is looked at.
const TICK: Duration = Duration::from_millis(250);
/// After the player is told to follow the session, it's left alone this long at most while
/// it gets there (a track has to load).
const SETTLE: Duration = Duration::from_secs(10);
/// What plays is told to the server again this often even if it didn't change (it forgets
/// after a while).
const REPORT_EVERY: Duration = Duration::from_secs(30);
/// While the page is shown, who listens to what is asked again this often.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// What the page draws from.
#[derive(Default)]
struct Shared {
    room: Option<Room>,
    listening: Vec<Listening>,
    error: Option<String>,
    catalog: Option<Arc<Catalog>>,
    /// Others may see what plays.
    sharing: bool,
    /// When the page was last drawn (who listens to what is only asked for while it is).
    page_shown: Option<Instant>,
}

/// What the page shows.
pub struct View {
    pub room: Option<Room>,
    pub listening: Vec<Listening>,
    pub error: Option<String>,
    pub sharing: bool,
}

pub struct Together {
    api: Api,
    controller: Arc<Controller>,
    runtime: tokio::runtime::Handle,
    ctx: egui::Context,
    shared: Arc<Mutex<Shared>>,
    session: Option<JoinHandle<()>>,
    /// Asks the background work to look at who listens to what now.
    refresh: Arc<tokio::sync::Notify>,
}

impl Together {
    pub fn new(api: Api, controller: Arc<Controller>, runtime: tokio::runtime::Handle, ctx: egui::Context, sharing: bool) -> Together {
        let shared = Arc::new(Mutex::new(Shared { sharing, ..Shared::default() }));
        let refresh = Arc::new(tokio::sync::Notify::new());
        let together = Together { api, controller, runtime, ctx, shared, session: None, refresh };
        together.spawn_reporting();
        together
    }

    pub fn view(&self) -> View {
        let s = self.shared.lock().unwrap();
        View { room: s.room.clone(), listening: s.listening.clone(), error: s.error.clone(), sharing: s.sharing }
    }

    pub fn in_session(&self) -> bool {
        self.shared.lock().unwrap().room.is_some()
    }

    /// The library as it is now (to find the tracks a session plays).
    pub fn set_catalog(&self, catalog: Option<Arc<Catalog>>) {
        let mut s = self.shared.lock().unwrap();
        if s.catalog.as_ref().map(Arc::as_ptr) != catalog.as_ref().map(Arc::as_ptr) {
            s.catalog = catalog;
        }
    }

    /// The page is drawn: who listens to what is kept up to date. Asks at once if it has
    /// been a while.
    pub fn page_shown(&self) {
        let mut s = self.shared.lock().unwrap();
        let stale = s.page_shown.is_none_or(|at| at.elapsed() > Duration::from_secs(4));
        s.page_shown = Some(Instant::now());
        drop(s);
        if stale {
            self.refresh.notify_one();
        }
    }

    pub fn refresh_now(&self) {
        self.refresh.notify_one();
    }

    pub fn set_sharing(&self, sharing: bool) {
        self.shared.lock().unwrap().sharing = sharing;
    }

    /// Starts a session and joins it.
    pub fn start(&mut self) {
        let api = self.api.clone();
        self.enter(async move { core::start(&api).await });
    }

    /// Joins the session with this invite code.
    pub fn join(&mut self, code: &str) {
        let (api, code) = (self.api.clone(), code.to_string());
        self.enter(async move { core::join(&api, &code).await });
    }

    fn enter(&mut self, asking: impl std::future::Future<Output = Result<Room, crate::api::ApiError>> + Send + 'static) {
        self.stop_following();
        self.shared.lock().unwrap().error = None;
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        let (api, controller) = (self.api.clone(), self.controller.clone());
        self.session = Some(self.runtime.spawn(async move {
            match asking.await {
                Ok(room) => {
                    shared.lock().unwrap().room = Some(room.clone());
                    ctx.request_repaint();
                    follow(api, controller, shared.clone(), ctx.clone(), room).await;
                }
                Err(err) => shared.lock().unwrap().error = Some(err.message),
            }
            ctx.request_repaint();
        }));
    }

    fn stop_following(&mut self) {
        if let Some(task) = self.session.take() {
            task.abort();
        }
    }

    /// Leaves the session (the music goes on playing, on its own).
    pub fn leave(&mut self) {
        self.stop_following();
        let room = self.shared.lock().unwrap().room.take();
        if let Some(room) = room {
            let api = self.api.clone();
            self.runtime.spawn(async move {
                let _ = core::leave(&api, room.id).await;
            });
        }
    }

    /// Tells the server what plays while others may see it, and asks what they play while
    /// the page is shown.
    fn spawn_reporting(&self) {
        let (api, controller, shared, ctx, refresh) = (self.api.clone(), self.controller.clone(), self.shared.clone(), self.ctx.clone(), self.refresh.clone());
        self.runtime.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            // What the server was last told, and when.
            let mut told: Option<(Option<(Uuid, bool)>, Instant)> = None;
            let mut asked = Instant::now() - REFRESH_EVERY;
            loop {
                let wanted_now = tokio::select! {
                    _ = tick.tick() => false,
                    _ = refresh.notified() => true,
                };
                // Not logged in: nothing to say or ask.
                if api.session().is_none() {
                    told = None;
                    continue;
                }
                let (sharing, shown) = {
                    let s = shared.lock().unwrap();
                    (s.sharing, s.page_shown.is_some_and(|at| at.elapsed() < REFRESH_EVERY * 2))
                };
                let playing = {
                    let view = controller.view.lock().unwrap();
                    view.current().filter(|c| c.source == AudioSource::File).map(|c| (c.track_id, view.paused))
                };
                let wanted = if sharing { playing } else { None };
                let changed = told.as_ref().is_none_or(|(was, at)| *was != wanted || (wanted.is_some() && at.elapsed() > REPORT_EVERY));
                // (Not sharing and never said anything: nothing to take back.)
                let nothing_to_say = !sharing && told.as_ref().is_none_or(|(was, _)| was.is_none());
                if changed && !nothing_to_say && core::report(&api, wanted).await.is_ok() {
                    told = Some((wanted, Instant::now()));
                }
                if wanted_now || (shown && asked.elapsed() >= REFRESH_EVERY) {
                    asked = Instant::now();
                    if let Ok(list) = core::listening(&api).await {
                        shared.lock().unwrap().listening = list;
                        ctx.request_repaint();
                    }
                }
            }
        });
    }
}

/// The player's state, in the terms of a session. Playing something that isn't a library
/// track (a file that didn't import, an extra) is not a state a session can hold.
fn local_state(controller: &Controller) -> Local {
    let view = controller.view.lock().unwrap().clone();
    let Some(current) = view.current() else { return Local::default() };
    if current.source != AudioSource::File {
        return Local::default();
    }
    Local {
        list: view.list.iter().map(|i| i.track_id).collect(),
        index: view.list_index,
        queued: view.queued.iter().map(|i| i.track_id).collect(),
        current_queued: view.current_queued.then_some(current.track_id),
        loaded: true,
        paused: view.paused,
        position: view.position_now(),
    }
}

/// Does what makes the player be where the session is.
async fn apply(controller: &Arc<Controller>, catalog: &Catalog, synced: &Synced, change: Change) {
    let session = &synced.room.playing;
    let items = |ids: &[Uuid]| -> Vec<_> { ids.iter().filter_map(|id| catalog.track_index(*id)).map(|i| queue_item(catalog, i)).collect() };
    match change {
        Change::Nothing => {}
        Change::Replace { position } => {
            let list = items(&session.list);
            let Some(index) = session.list.get(session.index).and_then(|t| list.iter().position(|i| i.track_id == *t)) else {
                // The list's place is a track this library hasn't: only a queued one can play.
                return;
            };
            let queued = items(&session.queued);
            let current_queued = session.current_queued.and_then(|t| catalog.track_index(t)).map(|i| queue_item(catalog, i));
            let (repeat, shuffle) = {
                let view = controller.view.lock().unwrap();
                (view.repeat, view.shuffle)
            };
            controller.restore(list, index, queued, current_queued, position, repeat, shuffle).await;
            if !session.paused {
                controller.set_paused(false);
            }
        }
        Change::Queue => {
            controller.clear_queued().await;
            controller.enqueue(items(&session.queued), false).await;
        }
        Change::Pause(paused) => controller.set_paused(paused),
        Change::Seek(to) => controller.seek(to),
    }
}

/// Follows a session until it ends or the task is stopped: takes what others change, and
/// sends what this player's user changes.
async fn follow(api: Api, controller: Arc<Controller>, shared: Arc<Mutex<Shared>>, ctx: egui::Context, first: Room) {
    let id = first.id;
    let mut synced = Synced::new(first);
    let mut tick = tokio::time::interval(TICK);
    let mut settling: Option<Instant> = None;

    let catalog = || shared.lock().unwrap().catalog.clone();
    let publish = |synced: &Synced, error: Option<String>| {
        let mut s = shared.lock().unwrap();
        s.room = Some(synced.room.clone());
        s.error = error;
        ctx.request_repaint();
    };
    // Makes the player follow `synced`; it's then left alone while it gets there.
    let follow_it = |synced: &Synced, settling: &mut Option<Instant>| {
        let (controller, synced, catalog) = (controller.clone(), synced.clone(), catalog());
        *settling = Some(Instant::now() + SETTLE);
        async move {
            let Some(catalog) = catalog else { return };
            let local = local_state(&controller);
            let known = |t: Uuid| catalog.track_index(t).is_some();
            let change = core::compare(&synced.room.playing, synced.position_now(), &local, &known);
            apply(&controller, &catalog, &synced, change).await;
        }
    };

    follow_it(&synced, &mut settling).await;
    let mut polling = Box::pin(core::poll(&api, id, Some(synced.room.version), 20));
    loop {
        tokio::select! {
            answer = &mut polling => {
                match answer {
                    Ok(room) => {
                        if room.version != synced.room.version || room.members != synced.room.members {
                            let changed = room.version != synced.room.version;
                            synced = Synced::new(room);
                            publish(&synced, None);
                            if changed {
                                follow_it(&synced, &mut settling).await;
                            }
                        }
                    }
                    Err(err) if err.status == Some(404) => {
                        let mut s = shared.lock().unwrap();
                        s.room = None;
                        s.error = Some("The session has ended.".into());
                        ctx.request_repaint();
                        return;
                    }
                    Err(err) => {
                        publish(&synced, Some(format!("Can't reach the session: {}", err.message)));
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
                polling = Box::pin(core::poll(&api, id, Some(synced.room.version), 20));
            }
            _ = tick.tick() => {
                let Some(catalog) = catalog() else { continue };
                let local = local_state(&controller);
                let known = |t: Uuid| catalog.track_index(t).is_some();
                let change = core::compare(&synced.room.playing, synced.position_now(), &local, &known);
                if let Some(deadline) = settling {
                    // Left alone until it's there, or it's taking too long.
                    if change == Change::Nothing || Instant::now() > deadline {
                        settling = None;
                    }
                    continue;
                }
                // Nothing loaded: nothing the user changed.
                if change == Change::Nothing || !local.loaded {
                    continue;
                }
                if local.list.len() + local.queued.len() > MAX_TRACKS {
                    publish(&synced, Some(format!("That list is too long to share (at most {MAX_TRACKS} tracks).")));
                    settling = Some(Instant::now() + SETTLE);
                    continue;
                }
                match core::send(&api, id, synced.room.version, &local.playing()).await {
                    Ok((accepted, room)) => {
                        synced = Synced::new(room);
                        publish(&synced, None);
                        if !accepted {
                            follow_it(&synced, &mut settling).await;
                        }
                    }
                    Err(err) => {
                        publish(&synced, Some(format!("Couldn't share that: {}", err.message)));
                        settling = Some(Instant::now() + SETTLE);
                    }
                }
            }
        }
    }
}
