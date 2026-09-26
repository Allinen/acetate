//! Folder layouts: what a file's path says about it, for `acetate ingest`.
//!
//! Downloaders put ids in paths, not in tags: NetEase downloads look like
//! `Ravel. - 10000037/FLACs/Albums/2020 - 100000074/01 - Right Now - 1000000111.flac`, and
//! yt-dlp names files `Title [id].ext`. A layout reads those ids and turns them into
//! hints (see `ingest::describe`); the metadata itself still comes from the tags.
//!
//! Layouts never guess: a path that doesn't fit gives no ids (and a warning), rather than
//! ids read from the wrong part of a name. To support another downloader, add a layout.

pub mod id_folders;
pub mod soundcloud;
pub mod ytdlp;

use std::path::{Path, PathBuf};

use serde_json::json;

use super::describe::ArtistHint;
use crate::model::sources::{self, Kind, NewSource, ValidSource};

/// Which layout to read paths with (`--layout`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Downloads with platform ids in the folder and file names: NetEase, QQ Music (see
    /// `id_folders`).
    IdFolders(id_folders::Platform),
    /// SoundCloud discographies: `<artist id>/{FLACs|streams}/...` (see `soundcloud`).
    Soundcloud,
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
    /// (from an id map, see `id_folders::IdMap`).
    pub album_track_number: Option<i16>,
    /// Whether the album is a single, if the folders say (see `release_kind`). None: they
    /// don't, and the tags decide.
    pub album_is_single: Option<bool>,
    /// The release date, for files whose tags have none (from an id map), or over theirs
    /// with `release_date_over_tags`.
    pub release_date: Option<String>,
    pub release_date_over_tags: bool,
    /// The album's own release date (see `AlbumHint::release_date`).
    pub album_release_date: Option<String>,
    /// The track's status (e.g. "deleted"), when the layout knows it. None: `--status`.
    pub status: Option<String>,
    /// Ids for the first ARTIST, whatever its name (see `Hints::first_artist_sources`).
    pub first_artist_sources: Vec<ValidSource>,
    /// Covers to fetch for the track and for its album (see `soundcloud::Artworks`).
    pub track_cover_urls: Vec<String>,
    pub album_cover_urls: Vec<String>,
    /// Where saved copies of those covers may be (SoundCloud: the artist's `Covers/`).
    pub cover_folder: Option<PathBuf>,
    /// Leave out the embedded pictures (see `Hints::skip_embedded_covers`).
    pub skip_embedded_covers: bool,
    /// The original file's name, for files whose tags have none (see `Hints::original_filename`).
    pub original_filename: Option<String>,
    pub warnings: Vec<String>,
}

impl Layout {
    /// Reads `path` (a file somewhere below `root`). Only folders from `root` down count:
    /// what's above it (e.g. `/home/me/Music - 2024`) isn't part of the layout.
    pub fn read(&self, root: &Path, path: &Path) -> PathInfo {
        let mut info = match self {
            Layout::IdFolders(platform) => id_folders::read(*platform, root, path),
            Layout::Soundcloud => soundcloud::read(root, path),
            Layout::Ytdlp(platform) => ytdlp::read(*platform, path),
            Layout::Tags => PathInfo::default(),
        };
        // The same for every layout.
        info.album_is_single = release_kind(root, path);
        info
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

/// Whether a file is on a single or an album, by its folders: the nearest folder (from
/// `root` down) called `Singles` or `Albums`, ignoring case, however deep below it the
/// file is. Some(true) for a single, Some(false) for an album, None if neither.
///
/// Folders win over the tags; without one of these folders, the track count decides (see
/// `ingest`: "1/1" is a single).
pub fn release_kind(root: &Path, path: &Path) -> Option<bool> {
    folder_names(root, path).iter().rev().find_map(|name| {
        if name.eq_ignore_ascii_case("singles") {
            Some(true)
        } else if name.eq_ignore_ascii_case("albums") {
            Some(false)
        } else {
            None
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn singles_and_albums_by_folder() {
        let root = PathBuf::from("/music");
        let kind = |p: &str| release_kind(&root, &root.join(p));
        assert_eq!(kind("A - 1/FLACs/Singles/Break - 2.flac"), Some(true));
        // Any depth below, any case.
        assert_eq!(kind("A - 1/FLACs/albums/2020 - 3/CD1/01 - x.flac"), Some(false));
        assert_eq!(kind("SINGLES/x.mp3"), Some(true));
        // The nearest one counts.
        assert_eq!(kind("Albums/Singles Collection/Singles/x.flac"), Some(true));
        assert_eq!(kind("Singles/Albums/x.flac"), Some(false));
        assert_eq!(kind("vk/x.mp3"), None);
        // The root itself counts too.
        assert_eq!(release_kind(Path::new("/music/Singles"), Path::new("/music/Singles/x.mp3")), Some(true));
    }
}
