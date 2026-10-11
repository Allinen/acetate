//! Folder layouts: what a file's path says about it, for `acetate ingest`.
//!
//! Downloaders put ids in paths, not in tags: yt-dlp names files `Title [id].ext`. A layout
//! reads those ids and turns them into hints (see `ingest::describe`); the metadata itself
//! still comes from the tags.
//!
//! Layouts never guess: a path that doesn't fit gives no ids (and a warning), rather than
//! ids read from the wrong part of a name. To support another downloader, add a layout.

pub mod ytdlp;

use std::path::Path;

use serde_json::json;

use crate::model::sources::{self, Kind, NewSource, ValidSource};

/// Which layout to read paths with (`--layout`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// yt-dlp's `Title [id].ext`, from one platform (`--platform`).
    Ytdlp(ytdlp::Platform),
    /// Nothing from the path: only the tags (and `--origin`).
    Tags,
}

/// What a layout read from one file's path.
#[derive(Debug, Default)]
pub struct PathInfo {
    /// The track on its platform (e.g. the YouTube video). It's where the file came from,
    /// so it becomes the origin.
    pub track: Option<ValidSource>,
    pub warnings: Vec<String>,
}

impl Layout {
    /// Reads `path`.
    pub fn read(&self, path: &Path) -> PathInfo {
        match self {
            Layout::Ytdlp(platform) => ytdlp::read(platform, path),
            Layout::Tags => PathInfo::default(),
        }
    }
}

/// A source with a single id field, e.g. `netease`, `{"song_id": 1000000111}`.
fn source(platform: &str, kind: Kind, field: &str, id: serde_json::Value) -> ValidSource {
    source_with(platform, kind, json!({ field: id }))
}

/// A source with these fields. Panics only if they don't fit the platform's shape in its
/// extension, which is a bug in the layout.
fn source_with(platform: &str, kind: Kind, data: serde_json::Value) -> ValidSource {
    let new = NewSource {
        platform: platform.to_string(),
        source_id: None,
        captured_at: None,
        data,
    };
    sources::validate(kind, new).expect("layouts only build sources that fit their shape")
}
