//! Domain types shared across modules. No IO in here.

pub mod sources;

use serde::{Deserialize, Serialize};

/// The scalar metadata of a track that is versioned through revisions.
///
/// This is exactly what gets stored in `track_revisions.data` (as JSON), and what
/// gets copied onto the denormalized columns of `tracks` for the primary variant.
/// Credits, relations, albums etc. are *not* part of this (they're not versioned).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub title: String,
    /// 'YYYY', 'YYYY-MM' or 'YYYY-MM-DD'. Partial dates are common, so this is text.
    pub release_date: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub comment: Option<String>,
}
