//! What plays, as the clients see it: the queue (`queue`), play counting (`scrobble`) and
//! the view of the player a UI draws from. The audio itself is each platform's.

pub mod player;
pub mod queue;
pub mod scrobble;
pub mod unsent;

use std::sync::Arc;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// Where a queued item's audio comes from.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AudioSource {
    /// A file in the library (`file_id`).
    #[default]
    File,
    /// A file that didn't import, kept for review (`file_id` is its entry's id): heard
    /// like any track, but it isn't one (no plays counted).
    NotImported,
    /// An extra's file (vocals, a stem…; `file_id` is the extra file's id): not a track
    /// either (no plays counted).
    Extra,
}

/// One queued track, with what the UI shows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub track_id: Uuid,
    pub file_id: Uuid,
    pub source: AudioSource,
    pub title: String,
    pub artist: String,
    /// Its main album's title, for the system's media controls.
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub cover: Option<crate::catalog::model::Cover>,
    /// The file's codec ("flac", "mp3"…), if known.
    pub codec: Option<String>,
}

impl QueueItem {
    /// Where its audio is fetched from.
    pub fn stream_path(&self) -> String {
        match self.source {
            AudioSource::File => format!("/api/files/{}/stream", self.file_id),
            AudioSource::NotImported => format!("/api/ingest-problems/{}/audio", self.file_id),
            AudioSource::Extra => format!("/api/extra-files/{}/stream", self.file_id),
        }
    }
}

/// A track (a position in the catalog) as the queue keeps it.
pub fn queue_item(catalog: &crate::catalog::Catalog, i: u32) -> QueueItem {
    let t = catalog.track_at(i);
    QueueItem {
        track_id: t.id(),
        file_id: t.file_id(),
        source: Default::default(),
        title: t.title().to_string(),
        artist: t.artist_display().to_string(),
        album: catalog.main_album(t).map(|a| a.title().to_string()),
        duration_ms: t.duration_ms(),
        cover: t.cover(),
        codec: t.codec().map(str::to_string),
    }
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

/// Puts `items` in a random order (Fisher–Yates, with a small xorshift generator seeded
/// from the clock: good enough for a playlist, no crate needed).
pub fn shuffle<T>(items: &mut [T]) {
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

