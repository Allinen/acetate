//! Picking up where it was: the queue (which tracks, which one plays, loop and shuffle)
//! and how far into the track, kept per server and user, and brought back paused when
//! the app starts again.
//!
//! The queue is written when it changes; the position every few seconds, in a small file
//! of its own (a queue can be the whole library, 185k tracks: not something to write
//! every few seconds).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::playback::{PlayerView, QueueItem};

#[derive(Serialize, Deserialize)]
pub struct SavedQueue {
    pub tracks: Vec<Uuid>,
    pub index: usize,
    pub repeat: bool,
    pub shuffle: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SavedPosition {
    pub track: Uuid,
    pub position_ms: u64,
}

pub struct Resume {
    /// `queue-<key>.json` and `position-<key>.json`.
    queue_file: PathBuf,
    position_file: PathBuf,
    /// What was written last: the queue (by identity) and index, and when the position.
    written: Option<(usize, usize, bool, bool)>,
    position_written: Instant,
    last_position: Option<(Uuid, u64)>,
}

impl Resume {
    pub fn new(dir: &std::path::Path, key: &str) -> Resume {
        Resume {
            queue_file: dir.join(format!("queue-{key}.json")),
            position_file: dir.join(format!("position-{key}.json")),
            written: None,
            position_written: Instant::now(),
            last_position: None,
        }
    }

    /// The saved queue and position, if any.
    pub fn load(&self) -> Option<(SavedQueue, Option<SavedPosition>)> {
        let queue: SavedQueue = serde_json::from_slice(&std::fs::read(&self.queue_file).ok()?).ok()?;
        let position = std::fs::read(&self.position_file).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Some((queue, position))
    }

    /// Called every frame: writes what changed (in the background).
    pub fn save(&mut self, view: &PlayerView, runtime: &tokio::runtime::Handle) {
        let state = (Arc::as_ptr(&view.items) as usize, view.index, view.repeat, view.shuffle);
        if view.items.is_empty() {
            return;
        }
        if self.written != Some(state) {
            self.written = Some(state);
            let saved = SavedQueue {
                tracks: view.items.iter().map(|i: &QueueItem| i.track_id).collect(),
                index: view.index,
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
