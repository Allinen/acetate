//! What every Acetate client shares, with no UI and no audio: talking to the server, who's
//! logged in, the library kept locally and its lookups, playlists, details, lyrics, the
//! queue and play counting. The desktop (egui) and the phone build on it.
//!
//! - `api`: talking to the server; `account`: who's logged in, what they may do.
//! - `catalog`: the library kept locally, and its lookups.
//! - `commandline`: the `key:value` line the pages are driven by.
//! - `details`: everything about one track, album or artist, for their pages.
//! - `lyrics`: synced lyrics (LRC).
//! - `memory`: how much memory the app holds, and giving freed memory back.
//! - `playback`: the queue, and the view of the player a UI draws from.
//! - `playlists`: playlists, kept locally like the catalog.
//! - `resume`: the queue kept between runs.
//! - `review`: the review flags.
//! - `title_credits`: credits written into titles, found by the rules the user picks.

// Answers from background work arrive in shared slots (`Arc<Mutex<Option<…>>>`), which
// clippy counts as "very complex types".
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod account;
pub mod api;
pub mod catalog;
pub mod commandline;
pub mod details;
pub mod lyrics;
pub mod memory;
pub mod playback;
pub mod playlists;
pub mod resume;
pub mod review;
pub mod title_credits;

use std::sync::Arc;

/// How background work tells the UI that something arrived and should be drawn again. The
/// desktop repaints its window; the phone calls back into Kotlin.
pub trait Wake: Send + Sync {
    fn changed(&self);
}

/// A shared `Wake`, handed to whatever works in the background.
pub type Waker = Arc<dyn Wake>;

/// A `Waker` that does nothing (tests).
pub struct NoWake;

impl Wake for NoWake {
    fn changed(&self) {}
}
