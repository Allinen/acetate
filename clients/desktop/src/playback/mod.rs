//! What plays: the queue, jumping around in it, and keeping the next track ready for
//! gapless playback. Runs on the async side; the audio itself is `engine`'s thread.

pub mod engine;
pub mod media_keys;
pub mod queue;
pub mod scrobble;
pub mod spectrum;
pub mod stream;

use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use eframe::egui;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::api::{Annotation, Api};
use scrobble::Listen;
use engine::{Command, Scope, Status};

/// One queued track, with what the UI shows about it.
#[derive(Debug, Clone)]
pub struct QueueItem {
    pub track_id: Uuid,
    pub file_id: Uuid,
    pub title: String,
    pub artist: String,
    /// Its main album's title, for the system's media controls.
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub cover: Option<crate::catalog::model::Cover>,
}

/// What the UI needs about playback.
#[derive(Clone)]
pub struct PlayerView {
    /// What plays (or last played, with `playing` false).
    pub current: Option<QueueItem>,
    /// It came from the queue (else from the list).
    pub current_queued: bool,
    /// The list played from, and where in it playback is.
    pub list: Arc<Vec<QueueItem>>,
    pub list_index: usize,
    /// Tracks queued by hand, next up.
    pub queued: Arc<Vec<QueueItem>>,
    pub position: Duration,
    /// When `position` was read: the UI moves the progress bar on from there.
    pub at: Instant,
    pub paused: bool,
    pub playing: bool,
    /// The current track plays again when it ends.
    pub repeat: bool,
    pub shuffle: bool,
    /// The volume slider (0 to 1) and mute.
    pub volume: f32,
    pub muted: bool,
}

impl Default for PlayerView {
    fn default() -> Self {
        PlayerView {
            current: None,
            current_queued: false,
            list: Arc::default(),
            list_index: 0,
            queued: Arc::default(),
            position: Duration::ZERO,
            at: Instant::now(),
            paused: false,
            playing: false,
            repeat: false,
            shuffle: false,
            volume: 1.0,
            muted: false,
        }
    }
}

impl PlayerView {
    pub fn current(&self) -> Option<&QueueItem> {
        if self.playing { self.current.as_ref() } else { None }
    }

    /// What plays after the current track.
    pub fn next(&self) -> Option<&QueueItem> {
        if self.repeat {
            return self.current();
        }
        self.queued.first().or_else(|| self.list.get(self.list_index + 1))
    }

    /// The position now, moved on smoothly since the last report while playing.
    pub fn position_now(&self) -> Duration {
        if self.playing && !self.paused { self.position + self.at.elapsed() } else { self.position }
    }
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
    /// Plays reported to the server, with its answers, for the UI to take.
    pub played: Mutex<Vec<(Uuid, Result<Annotation, String>)>>,
    /// Files downloaded ahead (see `prepare`).
    kept: Mutex<Vec<(Uuid, stream::RemoteFile)>>,
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
            played: Mutex::default(),
            kept: Mutex::default(),
            ctx,
        })
    }

    fn send(&self, command: Command) {
        let _ = self.audio.lock().unwrap().send(command);
    }

    /// Starts downloading a file; it can be played as soon as this returns.
    async fn fetch(&self, file_id: Uuid) -> anyhow::Result<stream::RemoteFile> {
        // Kept (see `prepare`): another reader of the same download.
        let kept = self.kept.lock().unwrap().iter().find(|(id, _)| *id == file_id).map(|(_, f)| f.clone());
        if let Some(file) = kept {
            return Ok(file);
        }
        stream::RemoteFile::open(&self.api, file_id).await
    }

    /// Plays from a list (an album, a playlist, a table), starting with `start`. Shuffled:
    /// `start` first, then the others in a random order. The hand-made queue stays.
    pub async fn play(self: &Arc<Self>, items: Vec<QueueItem>, start: usize) {
        self.play_from(items, start, Duration::ZERO).await;
    }

    /// Like `play`, starting `at` this point of the first track, with a quick fade from
    /// what plays (comparing two files: the other one goes on from where this one was).
    pub async fn play_from(self: &Arc<Self>, items: Vec<QueueItem>, start: usize, at: Duration) {
        let item = self.state.lock().await.queue.play_list(items, start, self.shuffle.load(Relaxed));
        if let Some(item) = item {
            self.start_fading(item, false, at, true).await;
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
            match self.fetch(id).await {
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

    /// `start`, with `fade`: a quick fade from what plays to it (comparing two files).
    async fn start_fading(self: &Arc<Self>, item: QueueItem, paused: bool, at: Duration, fade: bool) {
        let generation = {
            let mut state = self.state.lock().await;
            state.generation += 1;
            state.appended = None;
            state.fetching = false;
            state.next_asked += 1;
            *self.pending_seek.lock().unwrap() = None;
            state.generation
        };
        match self.fetch(item.file_id).await {
            Ok(file) => {
                if self.state.lock().await.generation == generation {
                    self.send(Command::Replace { file, paused, at, fade });
                }
            }
            Err(err) => self.report(format!("{err:#}")),
        }
        self.ctx.request_repaint();
    }

    /// What comes next, as the "next" slot would hold it: which, and which file.
    fn upcoming(&self, state: &State) -> Option<(queue::Upcoming, Uuid)> {
        let upcoming = state.queue.upcoming(self.repeat.load(Relaxed))?;
        Some((upcoming, state.queue.item(upcoming)?.file_id))
    }

    /// After a change: if what comes next isn't what it was, the "next" slot is emptied
    /// (and filled again with the right track). Works until the current track's last
    /// sample.
    fn next_changed(&self, state: &mut State, before: Option<(queue::Upcoming, Uuid)>) {
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
        self.send(Command::Stop);
        self.ctx.request_repaint();
    }

    pub async fn next(self: &Arc<Self>) {
        let item = {
            let mut state = self.state.lock().await;
            // Next means the next track, even when looping this one.
            let Some(upcoming) = state.queue.upcoming(false) else { return };
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

    /// Connects the system's media keys to this controller.
    pub fn attach_media_keys(self: &Arc<Self>, runtime: tokio::runtime::Handle, covers: std::path::PathBuf) {
        if let Some(keys) = media_keys::MediaKeys::start(self, runtime, covers) {
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
                    None => (status.position, Instant::now()),
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
            let heard = self.listen.lock().unwrap().update(current.map(|c| c.track_id), length, view.position_now(), view.paused);
            if let Some(track) = heard {
                let this = self.clone();
                tokio::spawn(async move {
                    let answer = this.api.played(track).await.map_err(|err| format!("{err:#}"));
                    this.played.lock().unwrap().push((track, answer));
                    this.ctx.request_repaint();
                });
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
            let upcoming = self.upcoming(&state);
            if let (true, None, false, Some((next, file_id))) = (status.playing, state.appended, state.fetching, upcoming) {
                state.fetching = true;
                let asked = state.next_asked;
                drop(state);
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this.fetch(file_id).await;
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
                        Err(err) => this.report(format!("{err:#}")),
                    }
                });
            }
        }
    }
}

/// Puts `items` in a random order (Fisher–Yates, with a small xorshift generator seeded
/// from the clock: good enough for a playlist, no crate needed).
fn shuffle<T>(items: &mut [T]) {
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in (1..items.len()).rev() {
        let j = (random() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        super::shuffle(&mut items);
        assert_ne!(items, (0..100).collect::<Vec<_>>(), "(1 in 100! to fail by chance)");
        items.sort();
        assert_eq!(items, (0..100).collect::<Vec<_>>());
    }
}
