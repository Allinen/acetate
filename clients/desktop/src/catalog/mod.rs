//! The library as this user sees it, kept locally (`GET /api/catalog`): every list,
//! sort and search in the app works on this, in memory, with no network wait.
//!
//! `model` is the data as the server sends it; `index` adds what's needed to look things
//! up quickly (by id, an album's tracks, an artist's...); `store` keeps it on disk and
//! brings it up to date.

pub mod index;
pub mod marks;
pub mod model;
pub mod query;
pub mod search;
pub mod store;

pub use index::Catalog;

use std::sync::Arc;
use std::time::SystemTime;

/// Something from the server, kept locally: shown at once, refreshed in the background.
/// When the server's copy differs, it wins and replaces this one.
pub struct Cached<T> {
    pub value: Option<Arc<T>>,
    /// When the server last confirmed it (None: only the copy from disk so far).
    pub confirmed_at: Option<SystemTime>,
    pub refreshing: bool,
    /// The last refresh's error, if it failed (the kept copy is still shown).
    pub error: Option<String>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Cached { value: None, confirmed_at: None, refreshing: false, error: None }
    }
}
