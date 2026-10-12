//! The library as this user sees it, kept locally (`GET /api/catalog`): every list,
//! sort and search in the app works on this, in memory, with no network wait.
//!
//! `model` is the data as the server sends it; `index` adds what's needed to look things
//! up quickly (by id, an album's tracks, an artist's...); `store` keeps it on disk and
//! brings it up to date.

#[cfg(test)]
pub(crate) mod fixture;
pub mod suggest;
mod flat;
#[cfg(test)]
mod measure;
pub mod index;
pub mod lyrics;
pub mod marks;
pub mod model;
pub mod query;
mod rows;
pub mod search;
mod snapshot;
pub mod store;
mod text;

pub use index::{Album, Catalog, CreditRef, Credits, Memberships, Track};

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
    /// A track that was made into an extra since: that extra (its page is shown instead).
    pub became_extra: Option<uuid::Uuid>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Cached { value: None, confirmed_at: None, refreshing: false, error: None, became_extra: None }
    }
}
