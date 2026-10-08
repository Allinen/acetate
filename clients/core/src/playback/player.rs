//! What decides what plays, for a platform that does the sound itself (Android's Media3):
//! the queue, repeat and shuffle, the quality, counting listens and keeping the queue
//! between runs, with no audio and no UI. The platform's player is an `Engine` this asks
//! things of, and tells this what happens (`on_transition`, `on_position`…).
//!
//! The same rules as the desktop's controller: the platform player holds the current
//! track and the one that comes next (its "next" slot), so playback goes on gaplessly. When
//! something changes what comes next (repeat, shuffle, the queue, the quality) the slot is
//! filled again with the right one.
//!
//! Everything here is a plain call that returns at once. The engine is only called after
//! the state is updated and its lock let go, so an engine may tell this about what it did
//! from inside the call.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::queue::{self, Queue, Upcoming};
use super::scrobble::Listen;
use super::{AudioSource, PlayerView, QueueItem};
use crate::api::Api;
use crate::resume::Resume;

/// Something to load into the platform player: the track and where its audio is fetched
/// from (a path on the server, with the quality applied).
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    pub item: QueueItem,
    pub path: String,
}

/// The platform's player.
pub trait Engine: Send + Sync {
    /// Plays this now, from `at`, paused or not, in place of what plays. The next slot is
    /// emptied (it's filled again by `set_next`).
    fn play(&self, load: &Load, at: Duration, paused: bool);
    /// What plays after the current track, gaplessly (None: nothing).
    fn set_next(&self, load: Option<&Load>);
    fn set_paused(&self, paused: bool);
    fn seek(&self, to: Duration);
    /// Goes on to what is in the next slot, now.
    fn skip(&self);
    fn stop(&self);
}

enum Command {
    Play(Load, Duration, bool),
    SetNext(Option<Load>),
    Paused(bool),
    Seek(Duration),
    Skip,
    Stop,
}

/// What is in the engine's next slot.
#[derive(Clone, PartialEq)]
struct Slot {
    upcoming: Upcoming,
    load: Load,
}

struct State {
    queue: Queue,
    slot: Option<Slot>,
    repeat: bool,
    shuffle: bool,
    quality: Option<u32>,
    position: Duration,
    /// When `position` was reported.
    at: Instant,
    playing: bool,
    paused: bool,
    listen: Listen,
}

pub struct Player {
    state: Mutex<State>,
    engine: Mutex<Option<Arc<dyn Engine>>>,
    api: Option<Api>,
    runtime: tokio::runtime::Handle,
    waker: crate::Waker,
    resume: Mutex<Option<Resume>>,
}

impl Player {
    /// `api`: where listens are reported (None: nowhere, for tests).
    pub fn new(api: Option<Api>, runtime: tokio::runtime::Handle, waker: crate::Waker) -> Player {
        Player {
            state: Mutex::new(State {
                queue: Queue::default(),
                slot: None,
                repeat: false,
                shuffle: false,
                quality: None,
                position: Duration::ZERO,
                at: Instant::now(),
                playing: false,
                paused: false,
                listen: Listen::default(),
            }),
            engine: Mutex::new(None),
            api,
            runtime,
            waker,
            resume: Mutex::new(None),
        }
    }

    /// The platform's player, once it's there. If something is loaded already (a queue
    /// brought back at start, before the player started), it's handed over, paused.
    pub fn attach(&self, engine: Arc<dyn Engine>) {
        *self.engine.lock().unwrap() = Some(engine);
        let mut commands = Vec::new();
        {
            let mut s = self.state.lock().unwrap();
            if let Some(item) = s.queue.current.as_ref().map(|c| c.item.clone()) {
                s.slot = None;
                s.paused = true;
                commands.push(Command::Play(s.load_of(&item), s.position, true));
                s.fill_next_slot(&mut commands);
            }
        }
        self.run(commands);
    }

    /// Keeps the queue between runs from now on (see `resume`).
    pub fn keep_with(&self, resume: Resume) {
        *self.resume.lock().unwrap() = Some(resume);
    }

    // ---- What to play ----

    /// Plays from a list (an album, a playlist), starting with `start`. Shuffled (if
    /// shuffle is on): `start` first, then the others in a random order. The hand-made
    /// queue stays.
    pub fn play_list(&self, items: Vec<QueueItem>, start: usize) {
        self.play_list_at(items, start, Duration::ZERO, false);
    }

    fn play_list_at(&self, items: Vec<QueueItem>, start: usize, at: Duration, paused: bool) {
        self.change(|s, out| {
            let shuffled = s.shuffle;
            if let Some(item) = s.queue.play_list(items, start, shuffled) {
                s.start(item, at, paused, out);
            }
        });
    }

    /// Plays a place in the queue or the list now.
    pub fn play_place(&self, place: queue::Place) {
        self.change(|s, out| {
            if let Some(item) = s.queue.play(place) {
                s.start(item, Duration::ZERO, false, out);
            }
        });
    }

    /// The next track (even when looping this one).
    pub fn next(&self) {
        self.change(|s, out| {
            let Some(upcoming) = s.queue.upcoming(false) else { return };
            // Already in the next slot: go on to it.
            let loaded = s.slot.as_ref().is_some_and(|slot| slot.upcoming == upcoming);
            let item = s.queue.item(upcoming).cloned();
            s.queue.advance(upcoming);
            let Some(item) = item else { return };
            if loaded {
                s.slot = None;
                s.note_start(&item);
                out.push(Command::Skip);
            } else {
                s.start(item, Duration::ZERO, false, out);
            }
        });
    }

    /// The track before (from a queued one: back to the list's), or this one again from
    /// its start when it has played more than 3 seconds.
    pub fn previous(&self) {
        self.change(|s, out| {
            let place = s.queue.previous();
            match place {
                Some(place) if s.position <= Duration::from_secs(3) => {
                    if let Some(item) = s.queue.play(place) {
                        s.start(item, Duration::ZERO, false, out);
                    }
                }
                _ => s.seek(Duration::ZERO, out),
            }
        });
    }

    pub fn toggle_repeat(&self) {
        self.change(|s, _| s.repeat = !s.repeat);
    }

    pub fn toggle_shuffle(&self) {
        self.change(|s, _| {
            s.shuffle = !s.shuffle;
            let on = s.shuffle;
            s.queue.set_shuffled(on);
        });
    }

    /// Queues tracks by hand: at the end of the queue, or (`first`) at its start. With
    /// nothing playing, the first plays.
    pub fn enqueue(&self, items: Vec<QueueItem>, first: bool) {
        self.change(|s, out| {
            if let Some(item) = s.queue.enqueue(items, first) {
                s.start(item, Duration::ZERO, false, out);
            }
        });
    }

    pub fn remove(&self, place: queue::Place) {
        self.change(|s, _| {
            s.queue.remove(place);
        });
    }

    pub fn clear_queued(&self) {
        self.change(|s, _| {
            s.queue.clear_queued();
        });
    }

    pub fn move_queued(&self, from: usize, to: usize) {
        self.change(|s, _| {
            s.queue.move_queued(from, to);
        });
    }

    /// Streams copies at this quality (kbps; None: what the server gives). Applies from
    /// the next track: what plays goes on, and the next slot is filled again.
    pub fn set_quality(&self, kbps: Option<u32>) {
        self.change(|s, _| s.quality = kbps);
    }

    // ---- The player ----

    pub fn set_paused(&self, paused: bool) {
        self.change(|s, out| {
            if s.queue.current.is_some() {
                s.paused = paused;
                out.push(Command::Paused(paused));
            }
        });
    }

    pub fn seek(&self, to: Duration) {
        self.change(|s, out| s.seek(to, out));
    }

    /// Stops and empties the list and the queue.
    pub fn stop(&self) {
        self.change(|s, out| {
            s.queue = Queue::default();
            s.slot = None;
            s.playing = false;
            s.paused = false;
            s.position = Duration::ZERO;
            out.push(Command::Stop);
        });
    }

    /// The queue as it was when the app last closed: loaded, paused at `at`.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        &self,
        list: Vec<QueueItem>,
        list_index: usize,
        queued: Vec<QueueItem>,
        current_queued: Option<QueueItem>,
        at: Duration,
        repeat: bool,
        shuffle: bool,
    ) {
        self.change(|s, out| {
            s.repeat = repeat;
            s.shuffle = shuffle;
            s.queue.play_list(list, list_index, false);
            s.queue.queued = Arc::new(queued);
            if let Some(item) = current_queued {
                s.queue.current = Some(queue::Current { item, queued: true });
            }
            if let Some(item) = s.queue.current.as_ref().map(|c| c.item.clone()) {
                s.start(item, at, true, out);
            }
        });
    }

    /// Brings back the queue kept by `keep_with` (from the last run), paused where it was,
    /// if nothing is loaded yet. Tracks that are gone from the catalog are left out.
    pub fn restore_saved(&self, catalog: &crate::catalog::Catalog) {
        if self.state.lock().unwrap().queue.current.is_some() {
            return;
        }
        let Some((list, queue, position)) = self.resume.lock().unwrap().as_ref().and_then(|r| r.load()) else { return };
        let item = |id: &Uuid| catalog.track_index(*id).map(|i| super::queue_item(catalog, i));
        let (mut items, mut before) = (Vec::new(), 0);
        for (n, id) in list.tracks.iter().enumerate() {
            if let Some(found) = item(id) {
                before += usize::from(n < queue.index);
                items.push(found);
            }
        }
        let index = before.min(items.len().saturating_sub(1));
        let queued = queue.queued.iter().filter_map(item).collect();
        let current_queued = queue.current_queued.as_ref().and_then(item);
        let playing = current_queued.as_ref().or(items.get(index)).map(|i: &QueueItem| i.track_id);
        let at = position.filter(|p| Some(p.track) == playing).map_or(Duration::ZERO, |p| Duration::from_millis(p.position_ms));
        self.restore(items, index, queued, current_queued, at, queue.repeat, queue.shuffle);
    }

    // ---- What the platform's player tells ----

    /// The platform's player went on to the item in the next slot by itself.
    pub fn on_transition(&self) {
        self.change(|s, _| {
            let Some(slot) = s.slot.take() else { return };
            s.queue.advance(slot.upcoming);
            let item = slot.load.item;
            s.note_start(&item);
        });
    }

    /// Called regularly (about twice a second) while it plays, and when something about
    /// it changes: how far into the current track, and whether it plays or waits.
    pub fn on_position(&self, position: Duration, playing: bool, paused: bool) {
        let (heard, view) = {
            let mut s = self.state.lock().unwrap();
            s.position = position;
            s.at = Instant::now();
            s.playing = playing;
            s.paused = paused;
            let current = s.queue.current.as_ref().map(|c| c.item.clone());
            let length = current.as_ref().and_then(|c| c.duration_ms).map(|ms| Duration::from_millis(ms.max(0) as u64));
            // A file that didn't import, or an extra, isn't a track: nothing to count.
            let track = current.filter(|c| c.source == AudioSource::File).map(|c| c.track_id);
            let heard = s.listen.update(track.filter(|_| playing), length, position, paused);
            (heard, s.view())
        };
        if let Some(track) = heard {
            self.report_listen(track);
        }
        if let Some(resume) = self.resume.lock().unwrap().as_mut() {
            resume.save(&view, &self.runtime);
        }
    }

    /// Playback ended: nothing comes after the last track.
    pub fn on_ended(&self) {
        self.change(|s, _| {
            s.playing = false;
            s.paused = false;
        });
    }

    pub fn view(&self) -> PlayerView {
        self.state.lock().unwrap().view()
    }

    // ---- Inside ----

    /// Runs a change of state, then does what it asked of the engine, and fills the next
    /// slot if what comes next is no longer what it holds.
    fn change(&self, f: impl FnOnce(&mut State, &mut Vec<Command>)) {
        let mut commands = Vec::new();
        {
            let mut s = self.state.lock().unwrap();
            f(&mut s, &mut commands);
            s.fill_next_slot(&mut commands);
        }
        self.run(commands);
        self.waker.changed();
    }

    fn run(&self, commands: Vec<Command>) {
        let Some(engine) = self.engine.lock().unwrap().clone() else { return };
        for command in commands {
            match command {
                Command::Play(load, at, paused) => engine.play(&load, at, paused),
                Command::SetNext(load) => engine.set_next(load.as_ref()),
                Command::Paused(paused) => engine.set_paused(paused),
                Command::Seek(to) => engine.seek(to),
                Command::Skip => engine.skip(),
                Command::Stop => engine.stop(),
            }
        }
    }

    fn report_listen(&self, track: Uuid) {
        let Some(api) = self.api.clone() else { return };
        self.runtime.spawn(async move {
            if let Err(err) = api.played(track).await {
                eprintln!("couldn't report a listen of {track}: {err}");
            }
        });
        self.waker.changed();
    }
}

impl State {
    fn path_of(&self, item: &QueueItem) -> String {
        let path = item.stream_path();
        match (item.source, self.quality) {
            (AudioSource::File, Some(kbps)) => format!("{path}?kbps={kbps}"),
            _ => path,
        }
    }

    fn load_of(&self, item: &QueueItem) -> Load {
        Load { item: item.clone(), path: self.path_of(item) }
    }

    /// Starts `item` now: the engine plays it, and its next slot is empty.
    fn start(&mut self, item: QueueItem, at: Duration, paused: bool, out: &mut Vec<Command>) {
        self.slot = None;
        self.note_start(&item);
        self.position = at;
        self.at = Instant::now();
        self.paused = paused;
        self.playing = true;
        out.push(Command::Play(self.load_of(&item), at, paused));
    }

    /// Something else plays now (the view shows its start).
    fn note_start(&mut self, _item: &QueueItem) {
        self.position = Duration::ZERO;
        self.at = Instant::now();
        self.playing = true;
    }

    fn seek(&mut self, to: Duration, out: &mut Vec<Command>) {
        if self.queue.current.is_some() {
            self.position = to;
            self.at = Instant::now();
            out.push(Command::Seek(to));
        }
    }

    /// If what comes next isn't what the slot holds, it's filled again (emptied: nothing
    /// comes next).
    fn fill_next_slot(&mut self, out: &mut Vec<Command>) {
        if self.queue.current.is_none() {
            return;
        }
        let want = self.queue.upcoming(self.repeat).and_then(|upcoming| {
            let load = self.load_of(self.queue.item(upcoming)?);
            Some(Slot { upcoming, load })
        });
        if want != self.slot {
            out.push(Command::SetNext(want.as_ref().map(|s| s.load.clone())));
            self.slot = want;
        }
    }

    fn view(&self) -> PlayerView {
        let q = &self.queue;
        PlayerView {
            current: q.current.as_ref().map(|c| c.item.clone()),
            current_queued: q.current.as_ref().is_some_and(|c| c.queued),
            list: q.list.clone(),
            list_index: q.list_index,
            queued: q.queued.clone(),
            position: self.position,
            at: self.at,
            paused: self.paused,
            playing: self.playing,
            repeat: self.repeat,
            shuffle: self.shuffle,
            volume: 1.0,
            muted: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Weak;

    /// Writes down what it's asked, one line each; and asks the player about itself from
    /// inside every call (which must not wait on a lock the call holds).
    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<String>>,
        player: Mutex<Weak<Player>>,
    }

    impl Fake {
        fn note(&self, line: String) {
            if let Some(player) = self.player.lock().unwrap().upgrade() {
                player.view();
            }
            self.log.lock().unwrap().push(line);
        }

        /// What was asked since last time.
        fn took(&self) -> Vec<String> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl Engine for Fake {
        fn play(&self, load: &Load, at: Duration, paused: bool) {
            self.note(format!("play {} @{}{}", load.item.title, at.as_secs(), if paused { " paused" } else { "" }));
        }
        fn set_next(&self, load: Option<&Load>) {
            self.note(format!("next {}", load.map_or("-".to_string(), |l| format!("{} {}", l.item.title, l.path))));
        }
        fn set_paused(&self, paused: bool) {
            self.note(format!("paused {paused}"));
        }
        fn seek(&self, to: Duration) {
            self.note(format!("seek {}", to.as_secs()));
        }
        fn skip(&self) {
            self.note("skip".into());
        }
        fn stop(&self) {
            self.note("stop".into());
        }
    }

    fn item(n: u128) -> QueueItem {
        QueueItem {
            track_id: Uuid::from_u128(n),
            file_id: Uuid::from_u128(n),
            source: AudioSource::File,
            title: n.to_string(),
            artist: String::new(),
            album: None,
            duration_ms: Some(100_000),
            cover: None,
        }
    }

    fn items(ns: &[u128]) -> Vec<QueueItem> {
        ns.iter().map(|&n| item(n)).collect()
    }

    fn path(n: u128) -> String {
        format!("/api/files/{}/stream", Uuid::from_u128(n))
    }

    struct Rig {
        player: Arc<Player>,
        engine: Arc<Fake>,
        _runtime: tokio::runtime::Runtime,
    }

    fn rig() -> Rig {
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let player = Arc::new(Player::new(None, runtime.handle().clone(), Arc::new(crate::NoWake)));
        let engine = Arc::new(Fake::default());
        *engine.player.lock().unwrap() = Arc::downgrade(&player);
        player.attach(engine.clone());
        Rig { player, engine, _runtime: runtime }
    }

    fn current(p: &Player) -> String {
        p.view().current.map(|c| c.title).unwrap_or_default()
    }

    #[test]
    fn a_list_plays_and_the_next_slot_holds_what_follows() {
        let r = rig();
        r.player.play_list(items(&[1, 2, 3]), 0);
        assert_eq!(r.engine.took(), ["play 1 @0", &format!("next 2 {}", path(2))]);
        // The platform went on by itself: 2 plays, 3 is next, then nothing is.
        r.player.on_transition();
        assert_eq!((current(&r.player), r.engine.took()), ("2".into(), vec![format!("next 3 {}", path(3))]));
        r.player.on_transition();
        assert_eq!((current(&r.player), r.engine.took()), ("3".into(), vec![]));
        r.player.on_transition();
        assert_eq!(current(&r.player), "3", "nothing was in the slot: nothing changes");
    }

    #[test]
    fn next_goes_on_to_what_is_loaded() {
        let r = rig();
        r.player.play_list(items(&[1, 2, 3]), 0);
        r.engine.took();
        r.player.next();
        assert_eq!(r.engine.took(), ["skip".to_string(), format!("next 3 {}", path(3))]);
        assert_eq!(current(&r.player), "2");
        r.player.next();
        r.player.next();
        assert_eq!(current(&r.player), "3", "the last one stays");
    }

    #[test]
    fn previous_restarts_after_three_seconds_else_goes_back() {
        let r = rig();
        r.player.play_list(items(&[1, 2, 3]), 1);
        r.engine.took();
        r.player.on_position(Duration::from_secs(10), true, false);
        r.player.previous();
        assert_eq!(r.engine.took(), ["seek 0"]);
        assert_eq!(current(&r.player), "2");
        r.player.on_position(Duration::from_secs(1), true, false);
        r.player.previous();
        assert_eq!(r.engine.took(), ["play 1 @0".to_string(), format!("next 2 {}", path(2))]);
        r.player.on_position(Duration::from_secs(1), true, false);
        r.player.previous();
        assert_eq!(r.engine.took(), ["seek 0"], "at the start of the list: this one again");
    }

    #[test]
    fn repeat_makes_the_current_track_the_next_one() {
        let r = rig();
        r.player.play_list(items(&[1, 2]), 0);
        r.engine.took();
        r.player.toggle_repeat();
        assert_eq!(r.engine.took(), [format!("next 1 {}", path(1))]);
        r.player.on_transition();
        // 1 plays again; the slot was used up, so 1 is put there again.
        assert_eq!((current(&r.player), r.engine.took()), ("1".into(), vec![format!("next 1 {}", path(1))]));
        r.player.toggle_repeat();
        assert_eq!(r.engine.took(), [format!("next 2 {}", path(2))]);
    }

    #[test]
    fn queueing_changes_the_slot_and_plays_when_idle() {
        let r = rig();
        r.player.enqueue(items(&[10, 11]), false);
        assert_eq!(r.engine.took(), ["play 10 @0".to_string(), format!("next 11 {}", path(11))]);
        r.player.play_list(items(&[1, 2]), 0);
        r.engine.took();
        r.player.enqueue(items(&[20]), false);
        assert_eq!(r.engine.took(), Vec::<String>::new(), "11 was first in the queue already");
        r.player.enqueue(items(&[30]), true);
        assert_eq!(r.engine.took(), [format!("next 30 {}", path(30))]);
        r.player.remove(queue::Place::Queued(0));
        assert_eq!(r.engine.took(), [format!("next 11 {}", path(11))]);
        r.player.clear_queued();
        assert_eq!(r.engine.took(), [format!("next 2 {}", path(2))]);
    }

    #[test]
    fn the_quality_applies_to_what_comes_next() {
        let r = rig();
        r.player.play_list(items(&[1, 2]), 0);
        r.engine.took();
        r.player.set_quality(Some(128));
        assert_eq!(r.engine.took(), [format!("next 2 {}?kbps=128", path(2))]);
        r.player.set_quality(None);
        assert_eq!(r.engine.took(), [format!("next 2 {}", path(2))]);
    }

    #[test]
    fn shuffle_keeps_what_plays_and_mixes_the_rest() {
        let r = rig();
        r.player.play_list(items(&(1..=30).collect::<Vec<_>>()), 4);
        r.engine.took();
        r.player.toggle_shuffle();
        let v = r.player.view();
        assert!(v.shuffle);
        assert_eq!(current(&r.player), "5");
        let mut all: Vec<_> = v.list.iter().map(|i| i.title.clone()).collect();
        all.sort_by_key(|t| t.parse::<u32>().unwrap());
        assert_eq!(all.len(), 30);
        r.player.toggle_shuffle();
        assert!(!r.player.view().shuffle);
        // Back in order, still on 5, so 6 is next.
        assert_eq!(r.engine.took().last().cloned(), Some(format!("next 6 {}", path(6))));
    }

    #[test]
    fn pausing_seeking_and_stopping() {
        let r = rig();
        r.player.set_paused(true);
        assert_eq!(r.engine.took(), Vec::<String>::new(), "nothing plays: nothing to pause");
        r.player.play_list(items(&[1, 2]), 0);
        r.engine.took();
        r.player.set_paused(true);
        r.player.seek(Duration::from_secs(42));
        assert_eq!(r.engine.took(), ["paused true", "seek 42"]);
        assert_eq!(r.player.view().position, Duration::from_secs(42));
        r.player.stop();
        assert_eq!(r.engine.took(), ["stop"]);
        assert!(r.player.view().current.is_none());
    }

    #[test]
    fn a_restored_queue_waits_paused_where_it_was() {
        let r = rig();
        r.player.restore(items(&[1, 2, 3]), 1, items(&[10]), None, Duration::from_secs(30), true, false);
        assert_eq!(r.engine.took(), ["play 2 @30 paused".to_string(), format!("next 2 {}", path(2))], "repeat is on");
        let v = r.player.view();
        assert!(v.paused && v.repeat && v.playing);
        assert_eq!(v.queued.len(), 1);
    }

    #[test]
    fn an_engine_that_comes_late_gets_what_is_loaded() {
        let r = rig();
        let late = Arc::new(Fake::default());
        let player = Arc::new(Player::new(None, r._runtime.handle().clone(), Arc::new(crate::NoWake)));
        player.restore(items(&[1, 2]), 0, vec![], None, Duration::from_secs(7), false, false);
        player.attach(late.clone());
        assert_eq!(late.took(), ["play 1 @7 paused".to_string(), format!("next 2 {}", path(2))]);
    }

    #[test]
    fn playing_another_list_keeps_the_queue() {
        let r = rig();
        r.player.play_list(items(&[1, 2]), 0);
        r.player.enqueue(items(&[10]), false);
        r.player.play_list(items(&[5, 6, 7]), 1);
        r.engine.took();
        r.player.next();
        assert_eq!(current(&r.player), "10");
        r.player.next();
        assert_eq!(current(&r.player), "7");
    }
}
