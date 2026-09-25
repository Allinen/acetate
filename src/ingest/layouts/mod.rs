//! Folder layouts: what a file's path says about it, for `acetate ingest`.
//!
//! Downloaders put ids in paths, not in tags: NetEase downloads look like
//! `Ravel. - 10000037/FLACs/Albums/2020 - 100000074/01 - Right Now - 1000000111.flac`, and
//! yt-dlp names files `Title [id].ext`. A layout reads those ids and turns them into
//! hints (see `ingest::describe`); the metadata itself still comes from the tags.
//!
//! Layouts never guess: a path that doesn't fit gives no ids (and a warning), rather than
//! ids read from the wrong part of a name. To support another downloader, add a layout.

pub mod netease;
pub mod ytdlp;

use std::path::Path;

use serde_json::json;

use super::describe::ArtistHint;
use crate::model::sources::{self, Kind, NewSource, ValidSource};

/// Which layout to read paths with (`--layout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// NetEase downloads (see `netease`).
    Netease,
    /// yt-dlp's `Title [id].ext`, from one platform (`--platform`).
    Ytdlp(ytdlp::Platform),
    /// Nothing from the path: only the tags (and `--origin`).
    Tags,
}

/// What a layout read from one file's path.
#[derive(Debug, Default)]
pub struct PathInfo {
    /// The track on its platform (e.g. the NetEase song). It's where the file came from,
    /// so it becomes the origin.
    pub track: Option<ValidSource>,
    /// Ids for artists, by the name the path gives them (matched against the tags).
    pub artists: Vec<ArtistHint>,
    /// Where the album is on its platform.
    pub album_sources: Vec<ValidSource>,
    /// Finds the album's cover in a `--covers` folder (for NetEase: the album id).
    pub album_cover_key: Option<String>,
    /// The track's position on the album, when the layout knows it better than the tags
    /// (from a NetEase album map, see `netease::AlbumMap`).
    pub album_track_number: Option<i16>,
    /// The album is a single (NetEase: the file is in a `Singles` folder).
    pub album_is_single: bool,
    pub warnings: Vec<String>,
}

impl Layout {
    /// Reads `path` (a file somewhere below `root`). Only folders from `root` down count:
    /// what's above it (e.g. `/home/me/Music - 2024`) isn't part of the layout.
    pub fn read(&self, root: &Path, path: &Path) -> PathInfo {
        match self {
            Layout::Netease => netease::read(root, path),
            Layout::Ytdlp(platform) => ytdlp::read(*platform, path),
            Layout::Tags => PathInfo::default(),
        }
    }
}

/// A source with a single id field, e.g. `netease`, `{"song_id": 1000000111}`.
fn source(platform: &str, kind: Kind, field: &str, id: serde_json::Value) -> ValidSource {
    source_with(platform, kind, json!({ field: id }))
}

/// A source with these fields. Panics only if they don't fit the platform's shape in
/// `SHAPES`, which is a bug in the layout.
fn source_with(platform: &str, kind: Kind, data: serde_json::Value) -> ValidSource {
    let new = NewSource {
        platform: platform.to_string(),
        source_id: None,
        captured_at: None,
        data,
    };
    sources::validate(kind, new).expect("layouts only build sources that fit their shape")
}

/// The folders from `root` (itself included) down to `path`'s folder, as names.
fn folder_names(root: &Path, path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = root.file_name() {
        names.push(name.to_string_lossy().to_string());
    }
    if let Some(parent) = path.parent() {
        if let Ok(below) = parent.strip_prefix(root) {
            for part in below.components() {
                names.push(part.as_os_str().to_string_lossy().to_string());
            }
        }
    }
    names
}
