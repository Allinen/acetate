//! What plays: the queue, jumping around in it, and keeping the next track ready for
//! gapless playback. Runs on the async side; the audio itself is `engine`'s thread.

pub mod engine;
pub mod eq;
pub mod opus;
pub mod media_keys;
pub use acetate_core::playback::{AudioSource, PlayerView, QueueItem, queue, queue_item, scrobble};
pub mod spectrum;
pub mod stream;

use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use eframe::egui;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::api::Api;
use scrobble::Listen;
use engine::{Command, Scope, Status};

/// The track playing now, as the queue has it.
fn q_current(state: &State) -> Option<Uuid> {
    state.queue.current.as_ref().map(|c| c.item.track_id)
}

/// The queue, and what the controller keeps about handing its tracks to the audio thread.
#[derive(Default)]
struct State {
    queue: queue::Queue,
    /// What's in the audio thread's "next" slot (to play after the current one,
    /// gaplessly). None: nothing yet.
    appended: Option<queue::Upcoming>,
    /// Changes with every track started, so downloads for an older one are dropped.
    generation: u64,
    fetching: bool,
    /// Numbers each fetch of a next track, so one that's no longer wanted (what comes
    /// next changed meanwhile) is dropped.
    next_asked: u64,
    /// The engine's switch count last seen (a change: the next track plays now).
    seen_switches: u64,
    /// The fetch for the next slot failed (online) at this `next_asked`: not tried again
    /// until what comes next changes.
    next_failed: Option<u64>,
}

pub struct Controller {
    api: Api,
    audio: Mutex<mpsc::Sender<Command>>,
    status: Arc<Mutex<Status>>,
    pub scope: Arc<Mutex<Scope>>,
    state: AsyncMutex<State>,
    pub view: Mutex<PlayerView>,
    /// A seek on its way: (where to, when asked). Until playback reports being near it,
    /// the view shows the target, so the bar doesn't jump back to the old position and
    /// forward again (the audio thread seeks a moment later).
    pending_seek: Mutex<Option<(Duration, Instant)>>,
    /// The last error from playback (or anything reported through `report`), and when:
    /// shown for a few seconds.
    pub error: Arc<Mutex<Option<(String, Instant)>>>,
    /// Counts listens (see `scrobble`).
    listen: Mutex<Listen>,
    /// The system's media controls, once attached (see `attach_media_keys`).
    media: std::sync::OnceLock<media_keys::MediaKeys>,
    /// Play the current track again when it ends (instead of the next).
    repeat: std::sync::atomic::AtomicBool,
    shuffle: std::sync::atomic::AtomicBool,
    /// The volume slider and mute, as the UI and the system's media controls see them.
    volume: Mutex<(f32, bool)>,
    /// Where listens go (the outbox of whoever is logged in).
    outbox: Mutex<Option<Arc<acetate_core::outbox::Outbox>>>,
    /// The quality asked for (kbps, see `set_quality`): None, as the server decides.
    quality: Mutex<Option<u32>>,
    /// Files downloaded ahead (see `prepare`).
    kept: Mutex<Vec<(Uuid, stream::RemoteFile)>>,
    /// The last two files handed to the audio thread (what plays, and what's next): a
    /// track looped, or played again right away, is read from memory, not downloaded
    /// again.
    recent: Mutex<Vec<(Uuid, stream::RemoteFile)>>,
    /// Where downloaded files are (by their hash on the server), and which of the files in
    /// play are found there (decided in `path_of`).
    local: Mutex<Option<acetate_core::playback::player::LocalFiles>>,
    locals: Mutex<std::collections::HashMap<Uuid, std::path::PathBuf>>,
    /// While offline, what can play (the rest is passed over).
    pub on_disk: acetate_core::playback::on_disk::OnDisk,
    /// Files that failed to come while offline (on disk by the list, but not there after
    /// all): passed over too, until the server is back.
    unplayable: Mutex<std::collections::HashSet<Uuid>>,
    /// Something to tell the user (tracks passed over), for the UI to take.
    pub notice: Mutex<Option<String>>,
    ctx: egui::Context,
}

impl Controller {
    pub fn new(api: Api, ctx: egui::Context) -> Arc<Controller> {
        // Errors from the audio thread and from here land in the same place.
        let error = Arc::new(Mutex::new(None));
        let (errors, error_ctx) = (error.clone(), ctx.clone());
        let (audio, status, scope) = engine::start(move |err| {
            *errors.lock().unwrap() = Some((err, Instant::now()));
            error_ctx.request_repaint();
        });
        Arc::new(Controller {
            api,
            audio: Mutex::new(audio),
            status,
            scope,
            state: AsyncMutex::new(State::default()),
            view: Mutex::new(PlayerView::default()),
            pending_seek: Mutex::new(None),
            error,
            listen: Mutex::default(),
            media: std::sync::OnceLock::new(),
            repeat: Default::default(),
            shuffle: Default::default(),
            volume: Mutex::new((1.0, false)),
            outbox: Mutex::default(),
            quality: Mutex::new(None),
            kept: Mutex::default(),
            recent: Mutex::default(),
            local: Mutex::new(None),
            locals: Mutex::default(),
            on_disk: Default::default(),
            unplayable: Mutex::default(),
            notice: Mutex::default(),
            ctx,
        })
    }

    /// Whether an item can play now (offline: only what's on this computer).
    fn playable(&self, item: &QueueItem) -> bool {
        self.on_disk.playable(item) && !self.unplayable.lock().unwrap().contains(&item.file_id)
    }

    /// What's on this computer changed, or the server went or came: the next slot is filled
    /// again if what comes next is no longer what it holds.
    pub async fn playable_changed(&self) {
        if !self.on_disk.is_limited() {
            self.unplayable.lock().unwrap().clear();
        }
        let mut state = self.state.lock().await;
        let next = self.upcoming(&state).map(|(upcoming, _)| upcoming);
        if state.appended.is_some() && state.appended != next || state.next_failed.is_some() {
            state.appended = None;
            state.fetching = false;
            state.next_asked += 1;
            state.next_failed = None;
            self.send(Command::SetNext(None));
        }
        self.ctx.request_repaint();
    }

    fn tell(&self, notice: String) {
        *self.notice.lock().unwrap() = Some(notice);
        self.ctx.request_repaint();
    }

    fn send(&self, command: Command) {
        let _ = self.audio.lock().unwrap().send(command);
    }

    /// Where an item's audio is fetched from, at the quality set (library files only).
    fn path_of(&self, item: &QueueItem) -> String {
        // A downloaded copy is played from the disk (`fetch` finds it by the file's id).
        let local = item.sha256.as_deref().zip(self.local.lock().unwrap().as_ref()).and_then(|(sha, find)| find(sha));
        match local {
            Some(path) if matches!(item.source, AudioSource::File | AudioSource::Extra) => self.locals.lock().unwrap().insert(item.file_id, path),
            _ => self.locals.lock().unwrap().remove(&item.file_id),
        };
        let path = item.stream_path();
        match (item.source, *self.quality.lock().unwrap()) {
            (AudioSource::File, Some(kbps)) => format!("{path}?kbps={kbps}"),
            _ => path,
        }
    }

    /// `path_of` for a library file by its id.
    fn path_of_file(&self, id: Uuid) -> String {
        match *self.quality.lock().unwrap() {
            Some(kbps) => format!("/api/files/{id}/stream?kbps={kbps}"),
            None => format!("/api/files/{id}/stream"),
        }
    }

    /// Streams copies at this quality (kbps, one the server makes; None: what the server
    /// gives: the original if you may have it, else its usual copy). Applies from the next
    /// track: what plays goes on, and what was fetched ahead is let go.
    pub async fn set_quality(self: &Arc<Self>, kbps: Option<u32>) {
        *self.quality.lock().unwrap() = kbps;
        self.kept.lock().unwrap().clear();
        self.recent.lock().unwrap().clear();
        let mut state = self.state.lock().await;
        state.appended = None;
        state.fetching = false;
        state.next_asked += 1;
        self.send(Command::SetNext(None));
    }

    /// Where listens go from now on.
    pub fn send_listens_to(&self, outbox: Option<Arc<acetate_core::outbox::Outbox>>) {
        *self.outbox.lock().unwrap() = outbox;
    }

    /// Where downloaded files are: what's there plays from the disk.
    pub fn set_local(&self, find: acetate_core::playback::player::LocalFiles) {
        *self.local.lock().unwrap() = Some(find);
    }

    /// Starts downloading a file; it can be played as soon as this returns.
    async fn fetch(&self, file_id: Uuid, path: String) -> anyhow::Result<stream::RemoteFile> {
        // Kept (see `prepare`), or just played: another reader of the same download.
        let kept = self.kept.lock().unwrap().iter().chain(self.recent.lock().unwrap().iter()).find(|(id, _)| *id == file_id).map(|(_, f)| f.clone());
        let local = self.locals.lock().unwrap().get(&file_id).cloned();
        let file = match (kept, local) {
            (Some(file), _) => file,
            // On the disk: no network, and works with the server gone.
            (None, Some(local)) => match stream::RemoteFile::local(&local) {
                Ok(file) => file,
                Err(_) => stream::RemoteFile::open(&self.api, path).await?,
            },
            (None, None) => stream::RemoteFile::open(&self.api, path).await?,
        };
        let mut recent = self.recent.lock().unwrap();
        recent.retain(|(id, _)| *id != file_id);
        recent.push((file_id, file.clone()));
        if recent.len() > 2 {
            recent.remove(0);
        }
        Ok(file)
    }

    /// Plays from a list (an album, a playlist, a table), starting with `start`. Shuffled:
    /// `start` first, then the others in a random order. The hand-made queue stays.
    pub async fn play(self: &Arc<Self>, items: Vec<QueueItem>, start: usize) {
        self.play_from(items, start, Duration::ZERO).await;
    }

    /// Like `play`, starting `at` this point of the first track, with a quick fade from
    /// what plays.
    pub async fn play_from(self: &Arc<Self>, items: Vec<QueueItem>, start: usize, at: Duration) {
        let item = self.state.lock().await.queue.play_list(items, start, self.shuffle.load(Relaxed));
        if let Some(item) = item {
            self.start_fading(item, false, at, true).await;
        }
    }

    /// Comparing two files: plays `item`, with `other` decoded in step beside it, so
    /// playing the other afterwards is a switch of what's heard, at the same moment, with
    /// no gap. `follow`: starts where what plays is (else at `at`).
    pub async fn compare(self: &Arc<Self>, item: QueueItem, other: QueueItem, at: Duration, follow: bool) {
        let item = self.state.lock().await.queue.play_list(vec![item], 0, self.shuffle.load(Relaxed));
        let Some(item) = item else { return };
        let generation = {
            let mut state = self.state.lock().await;
            state.generation += 1;
            state.appended = None;
            state.fetching = false;
            state.next_asked += 1;
            *self.pending_seek.lock().unwrap() = None;
            state.generation
        };
        let (mine, theirs) = (self.fetch(item.file_id, self.path_of(&item)).await, self.fetch(other.file_id, self.path_of(&other)).await);
        match (mine, theirs) {
            (Ok(mine), Ok(theirs)) => {
                if self.state.lock().await.generation == generation {
                    // Ordered by file id, so a switch back and forth is the same pair.
                    let side_b = item.file_id > other.file_id;
                    let (a, b) = if side_b { ((other.file_id, theirs), (item.file_id, mine)) } else { ((item.file_id, mine), (other.file_id, theirs)) };
                    self.send(Command::Compare { a, b, side_b, at, follow });
                }
            }
            (Err(err), _) | (_, Err(err)) => self.report(format!("{err:#}")),
        }
        self.ctx.request_repaint();
    }

    /// Opens two files for `compare` at `at`, ahead of it: the first play then starts at
    /// once, like the switches after it.
    pub async fn warm(&self, a: QueueItem, b: QueueItem, at: Duration) {
        let (a, b) = if a.file_id > b.file_id { (b, a) } else { (a, b) };
        // (Both downloaded ahead and kept, as `prepare` does.)
        self.prepare(vec![a.file_id, b.file_id]).await;
        let (x, y) = (self.fetch(a.file_id, self.path_of(&a)).await, self.fetch(b.file_id, self.path_of(&b)).await);
        if let (Ok(x), Ok(y)) = (x, y) {
            self.send(Command::Warm { a: (a.file_id, x), b: (b.file_id, y), at });
        }
    }

    /// Downloads these files ahead and keeps them (until the next call), so playing them
    /// starts at once, anywhere in them: comparing two files, switching back and forth.
    /// `prepare(vec![])` lets them go.
    pub async fn prepare(&self, files: Vec<Uuid>) {
        let missing: Vec<Uuid> = {
            let mut kept = self.kept.lock().unwrap();
            kept.retain(|(id, _)| files.contains(id));
            files.iter().filter(|id| !kept.iter().any(|(k, _)| k == *id)).copied().collect()
        };
        for id in missing {
            // (Library files: comparing two tracks.)
            match self.fetch(id, self.path_of_file(id)).await {
                Ok(file) => self.kept.lock().unwrap().push((id, file)),
                Err(err) => self.report(format!("{err:#}")),
            }
        }
    }

    /// Plays a place in the queue or the list now.
    pub async fn play_place(self: &Arc<Self>, place: queue::Place) {
        let item = self.state.lock().await.queue.play(place);
        if let Some(item) = item {
            self.start(item, false, Duration::ZERO).await;
        }
    }

    /// Starts playing `item` (paused and at `at`: a queue restored after a restart).
    async fn start(self: &Arc<Self>, item: QueueItem, paused: bool, at: Duration) {
        self.start_fading(item, paused, at, false).await;
    }

    /// `start`, with `fade`: a quick fade from what plays to it. While offline, one that
    /// isn't on this computer (or doesn't come: the server just went away) is passed over
    /// for the next that is.
    async fn start_fading(self: &Arc<Self>, mut item: QueueItem, paused: bool, mut at: Duration, fade: bool) {
        let mut skipped = 0;
        loop {
            let generation = {
                let mut state = self.state.lock().await;
                state.generation += 1;
                state.appended = None;
                state.fetching = false;
                state.next_asked += 1;
                *self.pending_seek.lock().unwrap() = None;
                state.generation
            };
            let fetched = if self.playable(&item) { Some(self.fetch(item.file_id, self.path_of(&item)).await) } else { None };
            match fetched {
                Some(Ok(file)) => {
                    if self.state.lock().await.generation == generation {
                        self.send(Command::Replace { file, paused, at, fade, follow: false });
                    }
                    break;
                }
                // Online, a failure is a failure.
                Some(Err(err)) if !self.api.connection().is_offline() => {
                    self.report(format!("{err:#}"));
                    break;
                }
                Some(Err(_)) => {
                    self.unplayable.lock().unwrap().insert(item.file_id);
                }
                None => {}
            }
            skipped += 1;
            let next = {
                let mut state = self.state.lock().await;
                if state.generation != generation {
                    return;
                }
                let next = state.queue.upcoming_where(false, |i| self.playable(i));
                next.and_then(|upcoming| {
                    let item = state.queue.item(upcoming).cloned();
                    state.queue.advance(upcoming);
                    item
                })
            };
            match next {
                Some(next) => (item, at) = (next, Duration::ZERO),
                None => {
                    self.send(Command::Stop);
                    self.tell("Not on this computer, and nothing after it is: nothing to play offline.".into());
                    self.ctx.request_repaint();
                    return;
                }
            }
        }
        if skipped > 0 {
            self.tell(format!("Not on this computer: skipped {skipped}."));
        }
        self.ctx.request_repaint();
    }

    /// What comes next, as the "next" slot would hold it: which, and which file (and where
    /// it's fetched from).
    fn upcoming(&self, state: &State) -> Option<(queue::Upcoming, (Uuid, String))> {
        let upcoming = state.queue.upcoming_where(self.repeat.load(Relaxed), |i| self.playable(i))?;
        let item = state.queue.item(upcoming)?;
        Some((upcoming, (item.file_id, self.path_of(item))))
    }

    /// After a change: if what comes next isn't what it was, the "next" slot is emptied
    /// (and filled again with the right track). Works until the current track's last
    /// sample.
    fn next_changed(&self, state: &mut State, before: Option<(queue::Upcoming, (Uuid, String))>) {
        if self.upcoming(state) != before || (state.appended.is_none() && state.fetching) {
            state.appended = None;
            state.fetching = false;
            state.next_asked += 1;
            self.send(Command::SetNext(None));
        }
        self.ctx.request_repaint();
    }

    pub async fn toggle_repeat(&self) {
        let mut state = self.state.lock().await;
        let before = self.upcoming(&state);
        self.repeat.fetch_xor(true, Relaxed);
        self.next_changed(&mut state, before);
    }

    /// Shuffles the rest of the list (not the hand-made queue), or puts it back in its
    /// own order from where playback is.
    pub async fn toggle_shuffle(&self) {
        let on = !self.shuffle.fetch_xor(true, Relaxed);
        let mut state = self.state.lock().await;
        let before = self.upcoming(&state);
        state.queue.set_shuffled(on);
        self.next_changed(&mut state, before);
    }

    /// Queues tracks by hand: at the end of the queue, or (`first`) at its start. With
    /// nothing playing, they play.
    pub async fn enqueue(self: &Arc<Self>, items: Vec<QueueItem>, first: bool) {
        if items.is_empty() {
            return;
        }
        let playing = self.status.lock().unwrap().playing;
        let start = {
            let mut state = self.state.lock().await;
            let before = self.upcoming(&state);
            let started = state.queue.enqueue(items, first);
            match started {
                Some(item) => Some(item),
                // The list had ended: go on with what was queued.
                None if !playing => state.queue.play(queue::Place::Queued(0)),
                None => {
                    self.next_changed(&mut state, before);
                    None
                }
            }
        };
        if let Some(item) = start {
            self.start(item, false, Duration::ZERO).await;
        }
    }

    /// Takes a place out of the queue or the list (not the one playing).
    pub async fn remove(&self, place: queue::Place) {
        let mut state = self.state.lock().await;
        let before = self.upcoming(&state);
        if state.queue.remove(place) {
            self.next_changed(&mut state, before);
        }
    }

    /// Empties the hand-made queue.
    pub async fn clear_queued(&self) {
        let mut state = self.state.lock().await;
        let before = self.upcoming(&state);
        if state.queue.clear_queued() {
            self.next_changed(&mut state, before);
        }
    }

    /// Moves a queued track to another place in the queue.
    pub async fn move_queued(&self, from: usize, to: usize) {
        let mut state = self.state.lock().await;
        let before = self.upcoming(&state);
        if state.queue.move_queued(from, to) {
            self.next_changed(&mut state, before);
        }
    }

    /// The queue as it was when the app last closed: loaded, paused at `at`.
    #[allow(clippy::too_many_arguments)]
    pub async fn restore(
        self: &Arc<Self>,
        list: Vec<QueueItem>,
        list_index: usize,
        queued: Vec<QueueItem>,
        current_queued: Option<QueueItem>,
        at: Duration,
        repeat: bool,
        shuffle: bool,
    ) {
        self.repeat.store(repeat, Relaxed);
        self.shuffle.store(shuffle, Relaxed);
        let item = {
            let mut state = self.state.lock().await;
            let q = &mut state.queue;
            q.play_list(list, list_index, false);
            q.queued = Arc::new(queued);
            if let Some(item) = current_queued {
                q.current = Some(queue::Current { item, queued: true });
            }
            q.current.as_ref().map(|c| c.item.clone())
        };
        if let Some(item) = item {
            self.start(item, true, at).await;
        }
    }

    /// Stops playback and empties the list and the queue (logging out: the next user
    /// starts with their own, brought back from their last session).
    pub async fn stop(&self) {
        let mut state = self.state.lock().await;
        state.queue = queue::Queue::default();
        state.generation += 1;
        state.appended = None;
        state.fetching = false;
        state.next_asked += 1;
        *self.pending_seek.lock().unwrap() = None;
        self.recent.lock().unwrap().clear();
        self.send(Command::Stop);
        self.ctx.request_repaint();
    }

    pub async fn next(self: &Arc<Self>) {
        let item = {
            let mut state = self.state.lock().await;
            // Next means the next track, even when looping this one.
            let Some(upcoming) = state.queue.upcoming_where(false, |i| self.playable(i)) else { return };
            // Already in the audio thread's "next" slot: go on to it.
            if state.appended == Some(upcoming) {
                self.send(Command::Skip);
                return;
            }
            let item = state.queue.item(upcoming).cloned();
            state.queue.advance(upcoming);
            item
        };
        if let Some(item) = item {
            self.start(item, false, Duration::ZERO).await;
        }
    }

    /// The track before (from a queued one: back to the list's), or this one again from
    /// its start when it has played more than 3 s.
    pub async fn previous(self: &Arc<Self>) {
        let position = self.status.lock().unwrap().position;
        let place = self.state.lock().await.queue.previous();
        match place {
            Some(place) if position <= Duration::from_secs(3) => self.play_place(place).await,
            _ => self.seek(Duration::ZERO),
        }
    }

    /// Connects the system's media keys to this controller (`window`: the window's handle,
    /// which Windows needs).
    pub fn attach_media_keys(self: &Arc<Self>, runtime: tokio::runtime::Handle, covers: std::path::PathBuf, window: Option<*mut std::ffi::c_void>) {
        if let Some(keys) = media_keys::MediaKeys::start(self, runtime, covers, window) {
            let _ = self.media.set(keys);
        }
    }

    /// An error to show for a few seconds.
    pub fn report(&self, error: String) {
        *self.error.lock().unwrap() = Some((error, Instant::now()));
        self.ctx.request_repaint();
    }

    /// Sets the volume slider (0 to 1) and mute.
    pub fn set_volume(&self, volume: f32, muted: bool) {
        let volume = volume.clamp(0.0, 1.0);
        *self.volume.lock().unwrap() = (volume, muted);
        self.send(Command::Volume(crate::settings::volume_amplitude(volume, muted)));
        self.view.lock().unwrap().volume = volume;
        self.view.lock().unwrap().muted = muted;
        self.ctx.request_repaint();
    }

    /// The equalizer: on or off, and each band's gain (dB, see `eq::BANDS`).
    pub fn set_equalizer(&self, enabled: bool, gains: eq::Gains) {
        self.send(Command::Equalizer(enabled, gains));
    }

    pub fn volume(&self) -> (f32, bool) {
        *self.volume.lock().unwrap()
    }

    /// Plays through another sound output (by name; None: the default). What plays goes
    /// on there, from where it was.
    pub async fn set_output(self: &Arc<Self>, name: Option<String>) {
        let status = *self.status.lock().unwrap();
        self.send(Command::Output(name));
        let current = self.state.lock().await.queue.current.as_ref().map(|c| c.item.clone());
        if let (Some(item), true) = (current, status.playing) {
            self.start(item, status.paused, status.position).await;
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.send(if paused { Command::Pause } else { Command::Resume });
    }

    /// Seeks by `seconds` from where it is (negative: back), within the track.
    pub fn seek_by(&self, seconds: f32) {
        let view = self.view.lock().unwrap().clone();
        if let Some(item) = view.current() {
            let position = view.position_now().as_secs_f32() + seconds;
            let end = item.duration_ms.unwrap_or(i64::MAX) as f32 / 1000.0;
            self.seek(Duration::from_secs_f32(position.clamp(0.0, (end - 0.5).max(0.0))));
        }
    }

    pub fn toggle_pause(&self) {
        let paused = self.status.lock().unwrap().paused;
        self.send(if paused { Command::Resume } else { Command::Pause });
    }

    pub fn seek(&self, position: Duration) {
        *self.pending_seek.lock().unwrap() = Some((position, Instant::now()));
        self.send(Command::Seek(position));
        // Show it right away, not at the next report.
        let mut view = self.view.lock().unwrap();
        view.position = position;
        view.at = Instant::now();
        if let Some(media) = self.media.get() {
            media.seeked();
        }
    }

    /// Every 50 ms: the position for the UI, a track ending, fetching the next in time.
    pub async fn monitor(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tick.tick().await;
            let status = *self.status.lock().unwrap();
            let mut state = self.state.lock().await;
            // A track ended and the one in the "next" slot plays now.
            if status.switches != state.seen_switches {
                state.seen_switches = status.switches;
                if let Some(upcoming) = state.appended.take() {
                    state.queue.advance(upcoming);
                }
            }
            // A pending seek: its target, moved on by the time since, until playback is
            // near it (or it's taking too long to tell).
            let (position, at) = {
                let mut pending = self.pending_seek.lock().unwrap();
                let shown = self.view.lock().unwrap().clone();
                match *pending {
                    Some((target, asked)) => {
                        let expected = if status.paused { target } else { target + asked.elapsed() };
                        let near = status.position.abs_diff(expected) < Duration::from_millis(400);
                        if near || asked.elapsed() > Duration::from_millis(1500) {
                            *pending = None;
                            (status.position, Instant::now())
                        } else {
                            (target, asked)
                        }
                    }
                    // Never a step back while it plays on: the position played is told in
                    // chunks, so a report can be a little behind what the UI already
                    // counted on to (a lyrics line lit would go back to the one before it
                    // for a moment). A real jump back (a seek, another track) still shows.
                    None => {
                        let counted = shown.position_now();
                        let same_track = shown.current.as_ref().map(|c| c.track_id) == q_current(&state);
                        let behind = counted.saturating_sub(status.position);
                        let playing_on = status.playing && !status.paused && shown.playing && !shown.paused;
                        if same_track && playing_on && status.position < counted && behind < Duration::from_millis(250) {
                            (counted, Instant::now())
                        } else {
                            (status.position, Instant::now())
                        }
                    }
                }
            };
            // Read into a local first: a lock taken inside the struct below would be held
            // until the whole statement ends, and a second one would wait on it forever.
            let (volume, muted) = *self.volume.lock().unwrap();
            let q = &state.queue;
            let view = PlayerView {
                current: q.current.as_ref().map(|c| c.item.clone()),
                current_queued: q.current.as_ref().is_some_and(|c| c.queued),
                list: q.list.clone(),
                list_index: q.list_index,
                queued: q.queued.clone(),
                position,
                at,
                paused: status.paused,
                playing: status.playing,
                repeat: self.repeat.load(Relaxed),
                shuffle: self.shuffle.load(Relaxed),
                volume,
                muted,
            };
            // A listen that counts is reported (here, not in the UI: it runs even while
            // the window isn't drawn).
            let current = view.current();
            let length = current.and_then(|c| c.duration_ms).map(|ms| Duration::from_millis(ms.max(0) as u64));
            // (A file that didn't import isn't a track: nothing to count.)
            let track = current.filter(|c| c.source == AudioSource::File).map(|c| c.track_id);
            let heard = self.listen.lock().unwrap().update(track, length, view.position_now(), view.paused);
            if let Some(track) = heard
                && let Some(outbox) = self.outbox.lock().unwrap().clone()
            {
                outbox.push(acetate_core::outbox::Change::played(track));
            }
            if let Some(media) = self.media.get() {
                media.update(&view);
            }
            {
                let mut shared = self.view.lock().unwrap();
                let changed = shared.current.as_ref().map(|c| c.track_id) != view.current.as_ref().map(|c| c.track_id)
                    || shared.list_index != view.list_index
                    || shared.paused != view.paused
                    || shared.playing != view.playing
                    || shared.repeat != view.repeat
                    || shared.shuffle != view.shuffle
                    || !Arc::ptr_eq(&shared.list, &view.list)
                    || !Arc::ptr_eq(&shared.queued, &view.queued);
                *shared = view;
                if changed {
                    self.ctx.request_repaint();
                }
            }

            // The next track goes into the audio thread's "next" slot as soon as the
            // current one plays (for gapless playback; it can still be changed later).
            let wanted = status.playing && state.appended.is_none() && !state.fetching && state.next_failed != Some(state.next_asked);
            if let Some((next, file_id)) = wanted.then(|| self.upcoming(&state)).flatten() {
                state.fetching = true;
                let asked = state.next_asked;
                drop(state);
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this.fetch(file_id.0, file_id.1).await;
                    let mut state = this.state.lock().await;
                    if state.next_asked != asked {
                        return;
                    }
                    state.fetching = false;
                    match result {
                        Ok(file) => {
                            state.appended = Some(next);
                            this.send(Command::SetNext(Some(file)));
                        }
                        // Offline: passed over; the one after it is tried next.
                        Err(_) if this.api.connection().is_offline() => {
                            this.unplayable.lock().unwrap().insert(file_id.0);
                        }
                        Err(err) => {
                            state.next_failed = Some(asked);
                            this.report(format!("{err:#}"));
                        }
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A set quality is asked for on library files only (not on videos or kept files).
    #[test]
    fn the_quality_is_asked_for_on_library_files_only() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = std::env::temp_dir().join(format!("acetate-quality-test-{}", std::process::id()));
        let controller = runtime.block_on(async { Controller::new(Api::new(dir), egui::Context::default()) });
        let item = |source| QueueItem {
            track_id: Uuid::nil(),
            file_id: Uuid::nil(),
            source,
            title: String::new(),
            artist: String::new(),
            album: None,
            duration_ms: None,
            cover: None,
            codec: None,
            sha256: None,
        };
        let nil = Uuid::nil();
        assert_eq!(controller.path_of(&item(AudioSource::File)), format!("/api/files/{nil}/stream"));
        runtime.block_on(controller.set_quality(Some(64)));
        assert_eq!(controller.path_of(&item(AudioSource::File)), format!("/api/files/{nil}/stream?kbps=64"));
        assert_eq!(controller.path_of_file(nil), format!("/api/files/{nil}/stream?kbps=64"));
        assert_eq!(controller.path_of(&item(AudioSource::Extra)), format!("/api/extra-files/{nil}/stream"));
        runtime.block_on(controller.set_quality(None));
        assert_eq!(controller.path_of(&item(AudioSource::File)), format!("/api/files/{nil}/stream"));
    }

    /// The playback loop and the volume use the same locks: neither may wait on the
    /// other forever (they once did, and the app never drew its window).
    #[test]
    fn the_loop_and_the_volume_dont_wait_on_each_other() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dir = std::env::temp_dir().join(format!("acetate-controller-test-{}", std::process::id()));
        let controller = runtime.block_on(async { Controller::new(Api::new(dir), egui::Context::default()) });
        runtime.spawn(controller.clone().monitor());
        for volume in [0.2, 0.7, 0.4] {
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (c, flag) = (controller.clone(), done.clone());
            std::thread::spawn(move || {
                c.set_volume(volume, false);
                flag.store(true, Relaxed);
            });
            let started = Instant::now();
            while !done.load(Relaxed) || (controller.view.lock().unwrap().volume - volume).abs() > 1e-6 {
                assert!(started.elapsed() < Duration::from_secs(2), "stuck setting the volume to {volume}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[test]
    fn shuffling_keeps_every_item() {
        let mut items: Vec<u32> = (0..100).collect();
        acetate_core::playback::shuffle(&mut items);
        assert_ne!(items, (0..100).collect::<Vec<_>>(), "(1 in 100! to fail by chance)");
        items.sort();
        assert_eq!(items, (0..100).collect::<Vec<_>>());
    }
}
