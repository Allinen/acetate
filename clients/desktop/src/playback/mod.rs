//! What plays: the queue, jumping around in it, and keeping the next track ready for
//! gapless playback. Runs on the async side; the audio itself is `engine`'s thread.

pub mod engine;
pub mod media_keys;
pub mod scrobble;
pub mod spectrum;
pub mod stream;

use std::sync::{mpsc, Arc, Mutex};
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
    pub cover_id: Option<Uuid>,
}

/// What the UI needs about playback.
#[derive(Clone)]
pub struct PlayerView {
    pub items: Arc<Vec<QueueItem>>,
    pub index: usize,
    pub position: Duration,
    /// When `position` was read: the UI moves the progress bar on from there.
    pub at: Instant,
    pub paused: bool,
    pub playing: bool,
}

impl Default for PlayerView {
    fn default() -> Self {
        PlayerView { items: Arc::default(), index: 0, position: Duration::ZERO, at: Instant::now(), paused: false, playing: false }
    }
}

impl PlayerView {
    pub fn current(&self) -> Option<&QueueItem> {
        if self.playing { self.items.get(self.index) } else { None }
    }

    /// The position now, moved on smoothly since the last report while playing.
    pub fn position_now(&self) -> Duration {
        if self.playing && !self.paused { self.position + self.at.elapsed() } else { self.position }
    }
}

#[derive(Default)]
struct Queue {
    items: Arc<Vec<QueueItem>>,
    index: usize,
    /// The last one handed to the audio thread (`index` or `index + 1`, gapless).
    appended: usize,
    /// Changes with every new queue or jump, so downloads for an old one are dropped.
    generation: u64,
    fetching: bool,
    last_queued: Option<usize>,
    seen_replacements: u64,
}

pub struct Controller {
    api: Api,
    audio: Mutex<mpsc::Sender<Command>>,
    status: Arc<Mutex<Status>>,
    pub scope: Arc<Mutex<Scope>>,
    queue: AsyncMutex<Queue>,
    pub view: Mutex<PlayerView>,
    /// A seek on its way: (where to, when asked). Until playback reports being near it,
    /// the view shows the target, so the bar doesn't jump back to the old position and
    /// forward again (the audio thread seeks a moment later).
    pending_seek: Mutex<Option<(Duration, Instant)>>,
    pub error: Arc<Mutex<String>>,
    /// Counts listens (see `scrobble`).
    listen: Mutex<Listen>,
    /// The system's media controls, once attached (see `attach_media_keys`).
    media: std::sync::OnceLock<media_keys::MediaKeys>,
    /// Plays reported to the server, with its answers, for the UI to take.
    pub played: Mutex<Vec<(Uuid, Result<Annotation, String>)>>,
    ctx: egui::Context,
}

impl Controller {
    pub fn new(api: Api, ctx: egui::Context) -> Arc<Controller> {
        // Errors from the audio thread and from here land in the same place.
        let error = Arc::new(Mutex::new(String::new()));
        let (errors, error_ctx) = (error.clone(), ctx.clone());
        let (audio, status, scope) = engine::start(move |err| {
            *errors.lock().unwrap() = err;
            error_ctx.request_repaint();
        });
        Arc::new(Controller {
            api,
            audio: Mutex::new(audio),
            status,
            scope,
            queue: AsyncMutex::new(Queue::default()),
            view: Mutex::new(PlayerView::default()),
            pending_seek: Mutex::new(None),
            error,
            listen: Mutex::default(),
            media: std::sync::OnceLock::new(),
            played: Mutex::default(),
            ctx,
        })
    }

    fn send(&self, command: Command) {
        let _ = self.audio.lock().unwrap().send(command);
    }

    /// Starts downloading a file; it can be played as soon as this returns.
    async fn fetch(&self, file_id: Uuid) -> anyhow::Result<stream::RemoteFile> {
        stream::RemoteFile::open(&self.api, file_id).await
    }

    pub async fn play(self: &Arc<Self>, items: Vec<QueueItem>, start: usize) {
        self.queue.lock().await.items = Arc::new(items);
        self.jump(start).await;
    }

    async fn jump(self: &Arc<Self>, index: usize) {
        let (file_id, generation) = {
            let mut queue = self.queue.lock().await;
            let Some(item) = queue.items.get(index) else { return };
            let file_id = item.file_id;
            queue.generation += 1;
            *self.pending_seek.lock().unwrap() = None;
            queue.index = index;
            queue.appended = index;
            queue.fetching = false;
            queue.last_queued = None;
            (file_id, queue.generation)
        };
        match self.fetch(file_id).await {
            Ok(file) => {
                let mut queue = self.queue.lock().await;
                if queue.generation == generation {
                    queue.last_queued = None;
                    self.send(Command::Replace(file));
                }
            }
            Err(err) => *self.error.lock().unwrap() = format!("{err:#}"),
        }
        self.ctx.request_repaint();
    }

    pub async fn next(self: &Arc<Self>) {
        let (index, appended, len) = {
            let queue = self.queue.lock().await;
            (queue.index, queue.appended, queue.items.len())
        };
        if index + 1 >= len {
            return;
        }
        if appended > index {
            self.send(Command::Skip);
        } else {
            self.jump(index + 1).await;
        }
    }

    pub async fn previous(self: &Arc<Self>) {
        let index = self.queue.lock().await.index;
        let position = self.status.lock().unwrap().position;
        let target = if position > Duration::from_secs(3) || index == 0 { index } else { index - 1 };
        self.jump(target).await;
    }

    /// Connects the system's media keys to this controller.
    pub fn attach_media_keys(self: &Arc<Self>, runtime: tokio::runtime::Handle, covers: std::path::PathBuf) {
        if let Some(keys) = media_keys::MediaKeys::start(self, runtime, covers) {
            let _ = self.media.set(keys);
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
            let mut queue = self.queue.lock().await;
            if status.replacements != queue.seen_replacements {
                queue.seen_replacements = status.replacements;
            } else if let Some(last) = queue.last_queued {
                if status.queued < last && status.queued > 0 {
                    queue.index += 1;
                }
            }
            queue.last_queued = Some(status.queued);
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
            let view = PlayerView {
                items: queue.items.clone(),
                index: queue.index,
                position,
                at,
                paused: status.paused,
                playing: status.queued > 0,
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
                let changed = shared.index != view.index
                    || shared.paused != view.paused
                    || shared.playing != view.playing
                    || !Arc::ptr_eq(&shared.items, &view.items);
                *shared = view;
                if changed {
                    self.ctx.request_repaint();
                }
            }

            let next = queue.index + 1;
            if status.queued == 1 && queue.appended == queue.index && next < queue.items.len() && !queue.fetching {
                queue.fetching = true;
                let (file_id, generation) = (queue.items[next].file_id, queue.generation);
                drop(queue);
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this.fetch(file_id).await;
                    let mut queue = this.queue.lock().await;
                    queue.fetching = false;
                    if queue.generation != generation {
                        return;
                    }
                    match result {
                        Ok(file) => {
                            queue.appended = next;
                            this.send(Command::Append(file));
                        }
                        Err(err) => *this.error.lock().unwrap() = format!("{err:#}"),
                    }
                });
            }
        }
    }
}
