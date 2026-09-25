//! NetEase downloads:
//!
//! ```text
//! Ravel. - 10000037/                      artist name - NetEase artist id
//!   FLACs/Albums/2020 - 100000074/        album title - NetEase album id
//!     01 - Right Now (feat. Moss Keeper) - 1000000111.flac      ... - NetEase song id
//!   FLACs/Singles/Pause - 1000000185.flac
//! ```
//!
//! * The song id is at the end of the filename. The NetEase song is the file's origin.
//! * The file's own folder, if it ends in ` - <id>`, is its album.
//! * A file directly in a `Singles` folder is on a single (its album credits differ,
//!   see `db::ingest::create_album`).
//! * The top-most folder (from where the import started) ending in ` - <id>`, other than
//!   the album's, is the artist. The name before the id says which of the tags' artists it
//!   is: it's only used if the ARTIST or ALBUM ARTIST tag has exactly that name.
//!
//! Album covers can come from a folder of images named `<title> - <album id>.jpg`.
//!
//! Songs outside album folders (e.g. `Singles/`) still have an album on NetEase. An album
//! map (`--netease-albums`, see `AlbumMap`) gives it, and each song's position on it.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};
use serde_json::json;

use super::{folder_names, source, source_with, PathInfo};
use crate::ingest::describe::ArtistHint;
use crate::model::sources::Kind;

pub fn read(root: &Path, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();

    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match split_id(&stem) {
        Some((_, song_id)) => {
            info.track = Some(source("netease", Kind::Track, "song_id", json!(song_id)));
        }
        None => info
            .warnings
            .push("no NetEase song id at the end of the filename (\"… - <id>\")".to_string()),
    }

    let folders = folder_names(root, path);
    let Some((album_folder, others)) = folders.split_last() else {
        return info;
    };
    info.album_is_single = album_folder == "Singles";
    if let Some((_, album_id)) = split_id(album_folder) {
        info.album_sources
            .push(source("netease", Kind::Album, "album_id", json!(album_id)));
        info.album_cover_key = Some(album_id.to_string());
    }
    // The top-most folder with an id, not counting the album's.
    if let Some((name, artist_id)) = others.iter().find_map(|f| split_id(f)) {
        info.artists.push(ArtistHint {
            name: name.to_string(),
            id: None,
            sources: vec![source("netease", Kind::Artist, "artist_id", json!(artist_id))],
        });
    }
    info
}

/// Where each NetEase song is on its NetEase album, from a JSON file mapping song ids to
/// `{"album_id": 100000074, "song_index": 3}`:
///
/// * `album_id` -1 (or 0, which isn't a real id either) means NetEase has no album for the
///   song: it gets no NetEase album id at all.
/// * `song_index` is inconsistent (numbers skipped or used twice), so the track's position
///   on the album isn't taken from it directly: an album's songs are sorted by it, ties
///   going to whichever song comes first in the file, and numbered 1, 2, 3... in that
///   order. The index itself is kept, verbatim, in the song's NetEase source
///   (`song_index`), so nothing is lost and the map can be rebuilt from the library.
pub struct AlbumMap {
    songs: HashMap<i64, Placement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    NoAlbum,
    Album {
        album_id: i64,
        /// The clean position: 1, 2, 3...
        track_number: i16,
        /// As NetEase has it.
        song_index: i64,
    },
}

#[derive(Deserialize)]
struct MapEntry {
    album_id: i64,
    song_index: i64,
}

impl AlbumMap {
    pub fn load(path: &Path) -> anyhow::Result<AlbumMap> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        AlbumMap::from_json(&text).with_context(|| format!("reading {}", path.display()))
    }

    pub fn from_json(text: &str) -> anyhow::Result<AlbumMap> {
        let InOrder(entries) = serde_json::from_str(text)?;

        // Each album's songs: (song_index, place in the file, song id).
        let mut albums: HashMap<i64, Vec<(i64, usize, i64)>> = HashMap::new();
        let mut songs = HashMap::new();
        for (place, (song_id, entry)) in entries.into_iter().enumerate() {
            let song_id: i64 = song_id
                .parse()
                .with_context(|| format!("\"{song_id}\" isn't a NetEase song id"))?;
            if entry.album_id <= 0 {
                songs.insert(song_id, Placement::NoAlbum);
            } else {
                albums.entry(entry.album_id).or_default().push((entry.song_index, place, song_id));
            }
        }
        for (album_id, mut tracks) in albums {
            // Tuples sort by their first field, then the second: by index, ties by place.
            tracks.sort();
            for (n, (song_index, _, song_id)) in tracks.into_iter().enumerate() {
                let track_number = i16::try_from(n + 1).unwrap_or(i16::MAX);
                songs.insert(song_id, Placement::Album { album_id, track_number, song_index });
            }
        }
        Ok(AlbumMap { songs })
    }

    pub fn get(&self, song_id: i64) -> Option<Placement> {
        self.songs.get(&song_id).copied()
    }

    /// Puts what the map says about the file's NetEase song into `info`, over what the
    /// folders said (with a warning if they disagree).
    pub fn apply(&self, info: &mut PathInfo) {
        let Some(song_id) = info
            .track
            .as_ref()
            .filter(|t| t.platform == "netease")
            .and_then(|t| t.external_id.as_ref())
            .and_then(|id| id.parse::<i64>().ok())
        else {
            return;
        };
        let folder_album = info.album_sources.first().and_then(|s| s.external_id.clone());
        match self.get(song_id) {
            None => info
                .warnings
                .push(format!("NetEase song {song_id} isn't in the album map")),
            Some(Placement::NoAlbum) => {
                if let Some(folder) = folder_album {
                    info.warnings.push(format!(
                        "the album map says NetEase has no album for this song; the folder's \
                         album id {folder} wasn't used"
                    ));
                }
                info.album_sources.clear();
                info.album_cover_key = None;
            }
            Some(Placement::Album { album_id, track_number, song_index }) => {
                if let Some(folder) = folder_album.filter(|f| *f != album_id.to_string()) {
                    info.warnings.push(format!(
                        "the folder says NetEase album {folder}, the album map {album_id}; used the map's"
                    ));
                }
                info.album_sources = vec![source("netease", Kind::Album, "album_id", json!(album_id))];
                info.album_cover_key = Some(album_id.to_string());
                info.album_track_number = Some(track_number);
                // The song's NetEase source (also the file's origin) keeps the raw index.
                info.track = Some(source_with(
                    "netease",
                    Kind::Track,
                    json!({ "song_id": song_id, "song_index": song_index }),
                ));
            }
        }
    }
}

/// A JSON object's entries in the order they're written. (serde_json's own map sorts
/// them by key, and the order matters here: it breaks ties.) Deserializing by hand like
/// this is serde's way of reading a format differently from the default: a `Visitor` is
/// handed the object's entries one by one. `'de` is the lifetime of the text being read.
struct InOrder(Vec<(String, MapEntry)>);

impl<'de> Deserialize<'de> for InOrder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;
        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = InOrder;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object of NetEase song ids")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<InOrder, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(InOrder(entries))
            }
        }
        deserializer.deserialize_map(EntriesVisitor)
    }
}

/// `"Title - 1000000111"` → `("Title", 1000000111)`. None unless the name ends in
/// ` - ` followed by only digits.
pub fn split_id(name: &str) -> Option<(&str, i64)> {
    let (before, id) = name.rsplit_once(" - ")?;
    if before.is_empty() || id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((before, id.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ids(info: &PathInfo) -> (Option<String>, Option<String>, Vec<(String, String)>) {
        let id = |s: &crate::model::sources::ValidSource| s.external_id.clone().unwrap();
        (
            info.track.as_ref().map(id),
            info.album_sources.first().map(id),
            info.artists
                .iter()
                .map(|a| (a.name.clone(), id(&a.sources[0])))
                .collect(),
        )
    }

    #[test]
    fn split_id_needs_a_trailing_number() {
        assert_eq!(split_id("2020 - 100000074"), Some(("2020", 100000074)));
        assert_eq!(split_id("Ravel. - 10000037"), Some(("Ravel.", 10000037)));
        assert_eq!(
            split_id("01 - Right Now (feat. Moss Keeper) - 1000000111"),
            Some(("01 - Right Now (feat. Moss Keeper)", 1000000111))
        );
        assert_eq!(split_id("Singles"), None);
        assert_eq!(split_id("Tidal, Vol. 2"), None);
        assert_eq!(split_id("Take - 12a"), None);
        assert_eq!(split_id("- 123"), None);
    }

    #[test]
    fn album_track_from_the_artist_folder_down() {
        let root = PathBuf::from("/music/netease");
        let path = root.join(
            "Ravel. - 10000037/FLACs/Albums/2020 - 100000074/01 - Right Now (feat. Moss Keeper) - 1000000111.flac",
        );
        let info = read(&root, &path);
        assert_eq!(
            ids(&info),
            (
                Some("1000000111".into()),
                Some("100000074".into()),
                vec![("Ravel.".into(), "10000037".into())]
            )
        );
        assert_eq!(info.album_cover_key.as_deref(), Some("100000074"));
        assert!(info.warnings.is_empty());
    }

    #[test]
    fn single_and_starting_inside_the_artist_folder() {
        let root = PathBuf::from("/music/netease/Ravel. - 10000037");
        let path = root.join("FLACs/Singles/Pause - 1000000185.flac");
        assert_eq!(
            ids(&read(&root, &path)),
            (Some("1000000185".into()), None, vec![("Ravel.".into(), "10000037".into())])
        );
    }

    #[test]
    fn starting_inside_an_album_folder() {
        // The album folder is the root: there's no artist folder to read.
        let root = PathBuf::from("/music/netease/Ravel. - 10000037/FLACs/Albums/2020 - 100000074");
        let path = root.join("03 - Late Friday - 1000000148.flac");
        assert_eq!(
            ids(&read(&root, &path)),
            (Some("1000000148".into()), Some("100000074".into()), vec![])
        );
    }

    #[test]
    fn album_map_orders_by_index_then_file_order() {
        // Album 5: index 2 twice (the first in the file wins the tie), index 1, and a gap.
        // Song 30 has no album (-1), song 31 neither (0).
        let map = AlbumMap::from_json(
            r#"{"20": {"album_id": 5, "song_index": 2}, "10": {"album_id": 5, "song_index": 7},
                "21": {"album_id": 5, "song_index": 2}, "22": {"album_id": 5, "song_index": 1},
                "30": {"album_id": -1, "song_index": 1}, "31": {"album_id": 0, "song_index": 0}}"#,
        )
        .unwrap();
        let at = |n, song_index| Some(Placement::Album { album_id: 5, track_number: n, song_index });
        assert_eq!(map.get(22), at(1, 1));
        assert_eq!(map.get(20), at(2, 2));
        assert_eq!(map.get(21), at(3, 2));
        assert_eq!(map.get(10), at(4, 7));
        assert_eq!(map.get(30), Some(Placement::NoAlbum));
        assert_eq!(map.get(31), Some(Placement::NoAlbum));
        assert_eq!(map.get(99), None);
    }

    #[test]
    fn album_map_gives_singles_their_album() {
        let map = AlbumMap::from_json(
            r#"{"1000000185": {"album_id": 100001036, "song_index": 1},
                "1000000222": {"album_id": 0, "song_index": 0},
                "1000000111": {"album_id": 999, "song_index": 1}}"#,
        )
        .unwrap();
        let root = PathBuf::from("/music/netease");
        let artist = root.join("Ravel. - 10000037/FLACs");

        let mut single = read(&root, &artist.join("Singles/Pause - 1000000185.flac"));
        assert!(single.album_is_single);
        map.apply(&mut single);
        assert_eq!(ids(&single).1.as_deref(), Some("100001036"));
        assert_eq!(single.album_track_number, Some(1));
        assert_eq!(single.track.as_ref().unwrap().data, json!({"song_id": 1000000185, "song_index": 1}));
        assert!(single.warnings.is_empty());

        let mut no_album = read(&root, &artist.join("Singles/Holidy Freestyle - 1000000222.flac"));
        map.apply(&mut no_album);
        assert_eq!(ids(&no_album).1, None);

        // The map wins over the folder, with a warning.
        let mut disagree = read(&root, &artist.join("Albums/2020 - 100000074/01 - Right Now - 1000000111.flac"));
        map.apply(&mut disagree);
        assert_eq!(ids(&disagree).1.as_deref(), Some("999"));
        assert_eq!(disagree.warnings.len(), 1);

        let mut unknown = read(&root, &artist.join("Singles/Keep Low - 1000000259.flac"));
        map.apply(&mut unknown);
        assert!(unknown.warnings[0].contains("isn't in the album map"));
    }

    #[test]
    fn folders_without_ids() {
        let root = PathBuf::from("test-audio/VANTA");
        let path = root.join("Albums/Tidal, Vol. 2/10 - Go Further! (Bonus Track) - 1000000296.flac");
        assert_eq!(ids(&read(&root, &path)), (Some("1000000296".into()), None, vec![]));

        let info = read(&root, &root.join("whiteout.flac"));
        assert_eq!(ids(&info), (None, None, vec![]));
        assert_eq!(info.warnings.len(), 1);
    }
}
