//! Picking up where it was: the queue (which tracks, which one plays, loop and shuffle)
//! and how far into the track, kept per server and user, and brought back paused when
//! the app starts again.
//!
//! Three files, each written only when it changes: the list played from (it can be the
//! whole library, 185k tracks: written when another list is played), where playback is
//! and the hand-made queue, and (every few seconds) how far into the track.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::playback::{PlayerView, QueueItem};

/// The list played from.
#[derive(Serialize, Deserialize)]
pub struct SavedList {
    pub tracks: Vec<Uuid>,
}

#[derive(Serialize, Deserialize)]
pub struct SavedQueue {
    /// Where playback was in the list.
    pub index: usize,
    /// Tracks queued by hand.
    #[serde(default)]
    pub queued: Vec<Uuid>,
    /// What played, when it came from the queue (else it's the list's `index`).
    #[serde(default)]
    pub current_queued: Option<Uuid>,
    pub repeat: bool,
    pub shuffle: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SavedPosition {
    pub track: Uuid,
    pub position_ms: u64,
}

pub struct Resume {
    /// `list-<key>.json`, `queue-<key>.json` and `position-<key>.json`.
    list_file: PathBuf,
    queue_file: PathBuf,
    position_file: PathBuf,
    /// What was written last: the list (by identity), the rest, and when the position.
    list_written: Option<usize>,
    written: Option<(usize, usize, Option<Uuid>, bool, bool)>,
    position_written: Instant,
    last_position: Option<(Uuid, u64)>,
}

impl Resume {
    pub fn new(dir: &std::path::Path, key: &str) -> Resume {
        Resume {
            list_file: dir.join(format!("list-{key}.json")),
            queue_file: dir.join(format!("queue-{key}.json")),
            list_written: None,
            position_file: dir.join(format!("position-{key}.json")),
            written: None,
            position_written: Instant::now(),
            last_position: None,
        }
    }

    /// The saved list, queue and position, if any.
    pub fn load(&self) -> Option<(SavedList, SavedQueue, Option<SavedPosition>)> {
        let list: SavedList = serde_json::from_slice(&std::fs::read(&self.list_file).ok()?).ok()?;
        let queue: SavedQueue = serde_json::from_slice(&std::fs::read(&self.queue_file).ok()?).ok()?;
        let position = std::fs::read(&self.position_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Some((list, queue, position))
    }

    /// Called every frame: writes what changed (in the background).
    pub fn save(&mut self, view: &PlayerView, runtime: &tokio::runtime::Handle) {
        let current = view.current.as_ref().map(|c| c.track_id);
        if view.current.is_none() {
            return;
        }
        let list = Arc::as_ptr(&view.list) as usize;
        if self.list_written != Some(list) {
            self.list_written = Some(list);
            let saved = SavedList { tracks: view.list.iter().map(|i: &QueueItem| i.track_id).collect() };
            write_later(runtime, self.list_file.clone(), saved);
        }
        let state = (Arc::as_ptr(&view.queued) as usize, view.list_index, current, view.repeat, view.shuffle);
        if self.written != Some(state) {
            self.written = Some(state);
            let saved = SavedQueue {
                index: view.list_index,
                queued: view.queued.iter().map(|i| i.track_id).collect(),
                current_queued: if view.current_queued { current } else { None },
                repeat: view.repeat,
                shuffle: view.shuffle,
            };
            write_later(runtime, self.queue_file.clone(), saved);
        }
        let Some(current) = view.current() else { return };
        let position = (current.track_id, view.position_now().as_millis() as u64);
        let moved = self.last_position.is_none_or(|(t, p)| t != position.0 || p.abs_diff(position.1) > 1000);
        if moved && self.position_written.elapsed() > Duration::from_secs(5) {
            self.position_written = Instant::now();
            self.last_position = Some(position);
            write_later(runtime, self.position_file.clone(), SavedPosition { track: position.0, position_ms: position.1 });
        }
    }
}

/// Writes `value` as JSON to `path`, off the UI thread (next to it first, then renamed
/// over it, so a crash never leaves half a file).
fn write_later<T: Serialize + Send + 'static>(runtime: &tokio::runtime::Handle, path: PathBuf, value: T) {
    runtime.spawn_blocking(move || {
        let Ok(bytes) = serde_json::to_vec(&value) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let temp = path.with_extension("part");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(temp, path);
        }
    });
}
