//! Domain types shared across modules. No IO in here.

pub mod sources;

use serde::{Deserialize, Serialize};

/// The scalar metadata of a track that is versioned through revisions.
///
/// This is exactly what gets stored in `track_revisions.data` (as JSON), and what
/// gets copied onto the denormalized columns of `tracks` for the primary variant.
/// Credits, relations, albums etc. are *not* part of this (they're not versioned).
///
/// Only fields with a value are stored, and every field has a default when reading.
/// That makes old revisions forward compatible: a field added later is simply missing
/// from them and reads as its default, no data migration needed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackMetadata {
    pub title: String,
    /// Verbatim from the source, e.g. '2019', '2019-05-12T11:11:11Z'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_set_fields_are_stored() {
        let meta = TrackMetadata {
            title: "Song".into(),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&meta).unwrap(), json!({ "title": "Song" }));
    }

    #[test]
    fn missing_fields_read_as_defaults() {
        let meta: TrackMetadata = serde_json::from_value(json!({ "title": "Song" })).unwrap();
        assert_eq!(meta.release_date, None);
        assert!(meta.genres.is_empty());
        // Fields this version doesn't know about (e.g. written by a newer one) are ignored.
        let meta: TrackMetadata =
            serde_json::from_value(json!({ "title": "Song", "future_field": 1 })).unwrap();
        assert_eq!(meta.title, "Song");
    }
}
