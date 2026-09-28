//! What plays: the queue, jumping around in it, and keeping the next track ready for
//! gapless playback. Runs on the async side; the audio itself is `engine`'s thread.

pub mod engine;
pub mod media_keys;
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
    pub items: Arc<Vec<QueueItem>>,
    pub index: usize,
    pub position: Duration,
    /// When `position` was read: the UI moves the progress bar on from there.
    pub at: Instant,
    pub paused: bool,
    pub playing: bool,
    /// The current track plays again when it ends.
    pub repeat: bool,
    pub shuffle: bool,
}

impl Default for PlayerView {
    fn default() -> Self {
        PlayerView {
            items: Arc::default(),
            index: 0,
            position: Duration::ZERO,
            at: Instant::now(),
            paused: false,
            playing: false,
            repeat: false,
            shuffle: false,
        }
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
    /// The item in the audio thread's "next" slot, to play after the current one
    /// (gapless): `index + 1`, or `index` itself when repeating. None: nothing yet.
    appended: Option<usize>,
    /// While shuffled: each item's place in the order it was queued in (to go back to it
    /// when shuffle is turned off), and that order.
    original: Option<(Vec<usize>, Arc<Vec<QueueItem>>)>,
    /// Changes with every new queue or jump, so downloads for an old one are dropped.
    generation: u64,
    fetching: bool,
    /// Numbers each fetch of a next track, so one that's no longer wanted (repeat or
    /// shuffle changed meanwhile) is dropped.
    next_asked: u64,
    /// The engine's switch count last seen (a change: the next track plays now).
    seen_switches: u64,
    /// Where the next "play next" goes: after the ones already added that way since the
    /// current track started (so several keep their order).
    next_run_end: usize,
}

impl Queue {
    /// Inserts `items`: at the end, or (`next`) after the current track and the ones
    /// already added that way. Returns where they went.
    fn insert(&mut self, items: Vec<QueueItem>, next: bool) -> usize {
        let len = self.items.len();
        let at = if next { self.next_run_end.clamp(self.index + 1, len) } else { len };
        let n = items.len();
        if next {
            self.next_run_end = at + n;
        }
        // Shuffled: they're also added to the queue's own order, at its end.
        if let Some((order, original)) = self.original.as_mut() {
            let first = original.len();
            Arc::make_mut(original).extend(items.iter().cloned());
            order.splice(at..at, first..first + n);
        }
        Arc::make_mut(&mut self.items).splice(at..at, items);
        at
    }

    /// Takes item `at` out (not the one playing). Returns whether what comes next
    /// changed (the audio thread's "next" slot has to be filled again).
    fn remove(&mut self, at: usize) -> bool {
        if at == self.index || at >= self.items.len() {
            return false;
        }
        Arc::make_mut(&mut self.items).remove(at);
        if let Some((order, original)) = self.original.as_mut() {
            let gone = order.remove(at);
            Arc::make_mut(original).remove(gone);
            for i in order.iter_mut() {
                if *i > gone {
                    *i -= 1;
                }
            }
        }
        if at < self.index {
            self.index -= 1;
        }
        if at < self.next_run_end {
            self.next_run_end -= 1;
        }
        self.next_run_end = self.next_run_end.max(self.index + 1);
        at <= self.index + 1 || self.appended.is_some_and(|a| at <= a)
    }

    /// Moves item `from` to `to` (neither the one playing). Returns whether it did.
    fn move_item(&mut self, from: usize, to: usize) -> bool {
        let len = self.items.len();
        if from == to || from >= len || to >= len || from == self.index || to == self.index {
            return false;
        }
        let item = Arc::make_mut(&mut self.items).remove(from);
        Arc::make_mut(&mut self.items).insert(to, item);
        if let Some((order, _)) = self.original.as_mut() {
            let i = order.remove(from);
            order.insert(to, i);
        }
        if from < self.index && to > self.index {
            self.index -= 1;
        } else if from > self.index && to < self.index {
            self.index += 1;
        }
        self.next_run_end = self.index + 1;
        true
    }
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
    /// Plays reported to the server, with its answers, for the UI to take.
    pub played: Mutex<Vec<(Uuid, Result<Annotation, String>)>>,
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
            queue: AsyncMutex::new(Queue::default()),
            view: Mutex::new(PlayerView::default()),
            pending_seek: Mutex::new(None),
            error,
            listen: Mutex::default(),
            media: std::sync::OnceLock::new(),
            repeat: Default::default(),
            shuffle: Default::default(),
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

    /// Queues `items` and plays from `start`. Shuffled: `start` first, then the others in
    /// a random order.
    pub async fn play(self: &Arc<Self>, items: Vec<QueueItem>, start: usize) {
        let start = {
            let mut queue = self.queue.lock().await;
            if self.shuffle.load(Relaxed) && start < items.len() {
                let mut order: Vec<usize> = (0..items.len()).filter(|&i| i != start).collect();
                shuffle(&mut order);
                order.insert(0, start);
                let shuffled = order.iter().map(|&i| items[i].clone()).collect();
                queue.original = Some((order, Arc::new(items)));
                queue.items = Arc::new(shuffled);
                0
            } else {
                queue.original = None;
                queue.items = Arc::new(items);
                start
            }
        };
        self.jump(start).await;
    }

    /// Empties the audio thread's "next" slot: the monitor fills it again with what
    /// comes next now. Works until the current track's last sample.
    fn clear_next(&self, queue: &mut Queue) {
        queue.appended = None;
        queue.fetching = false;
        queue.next_asked += 1;
        self.send(Command::SetNext(None));
    }

    pub async fn toggle_repeat(&self) {
        self.repeat.fetch_xor(true, Relaxed);
        let mut queue = self.queue.lock().await;
        self.clear_next(&mut queue);
        self.ctx.request_repaint();
    }

    /// Shuffles what's still to come (the current track stays), or goes back to the
    /// queue's own order from the current track.
    pub async fn toggle_shuffle(&self) {
        let on = !self.shuffle.fetch_xor(true, Relaxed);
        let mut queue = self.queue.lock().await;
        self.clear_next(&mut queue);
        let items = queue.items.clone();
        if on {
            // Everything up to what's playing stays where it is.
            let fixed = queue.index + 1;
            let mut order: Vec<usize> = (0..items.len()).collect();
            if fixed < order.len() {
                shuffle(&mut order[fixed..]);
            }
            queue.items = Arc::new(order.iter().map(|&i| items[i].clone()).collect());
            queue.original = Some((order, items));
        } else if let Some((order, original)) = queue.original.take() {
            // Back to the queue's order, from where the current track is in it.
            queue.index = order.get(queue.index).copied().unwrap_or(0);
            queue.items = original;
        }
        self.ctx.request_repaint();
    }

    async fn jump(self: &Arc<Self>, index: usize) {
        let (file_id, generation) = {
            let mut queue = self.queue.lock().await;
            let Some(item) = queue.items.get(index) else { return };
            let file_id = item.file_id;
            queue.generation += 1;
            *self.pending_seek.lock().unwrap() = None;
            queue.index = index;
            queue.next_run_end = index + 1;
            queue.appended = None;
            queue.fetching = false;
            queue.next_asked += 1;
            (file_id, queue.generation)
        };
        match self.fetch(file_id).await {
            Ok(file) => {
                let queue = self.queue.lock().await;
                if queue.generation == generation {
                    self.send(Command::Replace { file, paused: false, at: Duration::ZERO });
                }
            }
            Err(err) => self.report(format!("{err:#}")),
        }
        self.ctx.request_repaint();
    }

    /// Plays the queue from item `index`.
    pub async fn play_at(self: &Arc<Self>, index: usize) {
        self.jump(index).await;
    }

    /// Adds to the queue: at the end, or (`next`) right after the current track (after
    /// others added that way). With nothing playing, it plays them.
    pub async fn enqueue(self: &Arc<Self>, items: Vec<QueueItem>, next: bool) {
        if items.is_empty() {
            return;
        }
        let playing = self.status.lock().unwrap().playing;
        let start = {
            let mut queue = self.queue.lock().await;
            if queue.items.is_empty() {
                drop(queue);
                return self.play(items, 0).await;
            }
            let at = queue.insert(items, next);
            if at == queue.index + 1 {
                self.clear_next(&mut queue);
            }
            // Nothing playing (the queue had ended): start with what was added.
            (!playing).then_some(at)
        };
        if let Some(at) = start {
            self.jump(at).await;
        }
        self.ctx.request_repaint();
    }

    /// Takes item `at` out of the queue (not the one playing).
    pub async fn remove(&self, at: usize) {
        let mut queue = self.queue.lock().await;
        if queue.remove(at) {
            self.clear_next(&mut queue);
        }
        self.ctx.request_repaint();
    }

    /// Moves item `from` to `to` (neither the one playing).
    pub async fn move_item(&self, from: usize, to: usize) {
        let mut queue = self.queue.lock().await;
        if queue.move_item(from, to) {
            self.clear_next(&mut queue);
        }
        self.ctx.request_repaint();
    }

    /// The queue as it was when the app last closed: loaded, paused at `at`.
    pub async fn restore(self: &Arc<Self>, items: Vec<QueueItem>, index: usize, at: Duration, repeat: bool, shuffle: bool) {
        let Some(file_id) = items.get(index).map(|i| i.file_id) else { return };
        self.repeat.store(repeat, Relaxed);
        self.shuffle.store(shuffle, Relaxed);
        let generation = {
            let mut queue = self.queue.lock().await;
            queue.items = Arc::new(items);
            queue.original = None;
            queue.index = index;
            queue.next_run_end = index + 1;
            queue.appended = None;
            queue.generation += 1;
            queue.next_asked += 1;
            queue.generation
        };
        match self.fetch(file_id).await {
            Ok(file) => {
                if self.queue.lock().await.generation == generation {
                    self.send(Command::Replace { file, paused: true, at });
                }
            }
            Err(err) => self.report(format!("{err:#}")),
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
        // The next one is already queued in the audio thread: go on to it. (Not when
        // it's the same track again, repeating: Next means the next track.)
        if appended == Some(index + 1) {
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

    /// An error to show for a few seconds.
    pub fn report(&self, error: String) {
        *self.error.lock().unwrap() = Some((error, Instant::now()));
        self.ctx.request_repaint();
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
            // A track ended and the one in the "next" slot plays now.
            if status.switches != queue.seen_switches {
                queue.seen_switches = status.switches;
                queue.index = queue.appended.take().unwrap_or(queue.index + 1);
                queue.next_run_end = queue.index + 1;
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
            let view = PlayerView {
                items: queue.items.clone(),
                index: queue.index,
                position,
                at,
                paused: status.paused,
                playing: status.playing,
                repeat: self.repeat.load(Relaxed),
                shuffle: self.shuffle.load(Relaxed),
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
                    || shared.repeat != view.repeat
                    || shared.shuffle != view.shuffle
                    || !Arc::ptr_eq(&shared.items, &view.items);
                *shared = view;
                if changed {
                    self.ctx.request_repaint();
                }
            }

            // The next track goes into the audio thread's "next" slot as soon as the
            // current one plays (for gapless playback; it can still be changed later).
            let next = if self.repeat.load(Relaxed) { queue.index } else { queue.index + 1 };
            if status.playing && queue.appended.is_none() && next < queue.items.len() && !queue.fetching {
                queue.fetching = true;
                let (file_id, asked) = (queue.items[next].file_id, queue.next_asked);
                drop(queue);
                let this = self.clone();
                tokio::spawn(async move {
                    let result = this.fetch(file_id).await;
                    let mut queue = this.queue.lock().await;
                    if queue.next_asked != asked {
                        return;
                    }
                    queue.fetching = false;
                    match result {
                        Ok(file) => {
                            queue.appended = Some(next);
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

    fn item(n: u128) -> QueueItem {
        QueueItem {
            track_id: Uuid::from_u128(n),
            file_id: Uuid::from_u128(n),
            title: n.to_string(),
            artist: String::new(),
            album: None,
            duration_ms: None,
            cover: None,
        }
    }

    fn titles(items: &[QueueItem]) -> Vec<String> {
        items.iter().map(|i| i.title.clone()).collect()
    }

    fn queue(n: u128, index: usize) -> Queue {
        Queue { items: Arc::new((1..=n).map(item).collect()), index, next_run_end: index + 1, ..Queue::default() }
    }

    #[test]
    fn play_next_keeps_its_order_and_last_goes_last() {
        let mut q = queue(4, 1); // playing "2"
        assert_eq!(q.insert(vec![item(10)], true), 2);
        assert_eq!(q.insert(vec![item(11), item(12)], true), 3, "after the first play-next");
        assert_eq!(q.insert(vec![item(20)], false), 7, "at the end");
        assert_eq!(titles(&q.items), ["1", "2", "10", "11", "12", "3", "4", "20"]);
    }

    #[test]
    fn removing_and_moving_keep_the_current_track() {
        let mut q = queue(5, 2); // playing "3"
        assert!(!q.remove(2), "not the one playing");
        assert!(!q.remove(4), "a later one: what's next is the same");
        assert!(q.remove(0), "an earlier one");
        assert_eq!((titles(&q.items), q.index), (vec!["2".into(), "3".into(), "4".into()], 1));
        // Moving "2" (before the current track) after it: the current track's place moves.
        assert!(q.move_item(0, 2));
        assert_eq!((titles(&q.items), q.index), (vec!["3".into(), "4".into(), "2".into()], 0));
        assert!(!q.move_item(0, 1), "not the one playing");
    }

    #[test]
    fn shuffled_queues_keep_their_own_order_in_step() {
        // Shuffled: items are [3, 1, 2]; their places in the queue's own order [2, 0, 1].
        let original: Vec<QueueItem> = (1..=3).map(item).collect();
        let mut q = Queue {
            items: Arc::new(vec![item(3), item(1), item(2)]),
            original: Some((vec![2, 0, 1], Arc::new(original))),
            next_run_end: 1,
            ..Queue::default()
        };
        q.insert(vec![item(4)], true);
        q.remove(2); // "1"
        let (order, original) = q.original.clone().unwrap();
        assert_eq!(titles(&original), ["2", "3", "4"]);
        // Each item is still the one at its place in the own order.
        for (i, o) in order.iter().enumerate() {
            assert_eq!(q.items[i].title, original[*o].title);
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
