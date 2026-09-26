//! Downloads laid out by platform ids: NetEase, QQ Music and Kuwo (another platform with
//! the same layout needs a shape in `SHAPES` and a `Platform` here):
//!
//! ```text
//! Ravel. - 10000037/                      artist name - artist id
//!   FLACs/Albums/2020 - 100000074/        album title - album id
//!     01 - Right Now (feat. Moss Keeper) - 1000000111.flac      ... - song id
//!   FLACs/Singles/Pause - 1000000185.flac
//! $ilver40k - 1000333/FLACs/Glass Floors Shatter(feat. NOCTURNO) - 100000370.flac
//! ```
//!
//! * The song id is at the end of the filename (`… - <id>`), or is the whole name
//!   (`<id>.flac`). The song on the platform is the file's origin.
//! * The file's own folder, if it ends in ` - <id>`, is its album.
//! * Whether it's a single or an album comes from the `Singles`/`Albums` folders, as for
//!   every layout (see `layouts::release_kind`); without them, the tags decide.
//! * The top-most folder (from where the import started) ending in ` - <id>`, other than
//!   the album's, is the artist. The name before the id says which of the tags' artists it
//!   is: it's only used if the ARTIST or ALBUM ARTIST tag has that name.
//!
//! Album covers can come from a folder of images named `<title> - <album id>.jpg` (NetEase).
//!
//! Songs outside album folders (e.g. `Singles/`) still have an album on the platform. An
//! id map (`--id-map`, see `IdMap`) gives it, each song's position on it, and (for QQ)
//! the other ids of the song, its album and all its artists.

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

/// Which platform's ids the folders and the id map hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Netease,
    Qq,
    Kuwo,
}

impl Platform {
    /// The platform's name in sources (`SHAPES`).
    pub fn name(self) -> &'static str {
        match self {
            Platform::Netease => "netease",
            Platform::Qq => "qq",
            Platform::Kuwo => "kuwo",
        }
    }

    /// For messages.
    fn shown(self) -> &'static str {
        match self {
            Platform::Netease => "NetEase",
            Platform::Qq => "QQ Music",
            Platform::Kuwo => "Kuwo",
        }
    }

    /// Whether its songs, albums and artists also have a "mid" (QQ's second, textual id).
    fn has_mids(self) -> bool {
        self == Platform::Qq
    }
}

pub fn read(platform: Platform, root: &Path, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();
    let name = platform.name();

    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // Only a file's name may be just the id: a folder called "2020" isn't an album id.
    let bare_id = stem.parse::<i64>().ok().filter(|_| stem.bytes().all(|b| b.is_ascii_digit()));
    match split_id(&stem).map(|(_, id)| id).or(bare_id) {
        Some(song_id) => {
            info.track = Some(source(name, Kind::Track, "song_id", json!(song_id)));
        }
        None => info.warnings.push(format!(
            "no {} song id at the end of the filename (\"… - <id>\")",
            platform.shown()
        )),
    }

    let folders = folder_names(root, path);
    let Some((album_folder, others)) = folders.split_last() else {
        return info;
    };
    if let Some((_, album_id)) = split_id(album_folder) {
        info.album_sources.push(source(name, Kind::Album, "album_id", json!(album_id)));
        info.album_cover_key = Some(album_id.to_string());
    }
    // The top-most folder with an id, not counting the album's.
    if let Some((artist, artist_id)) = others.iter().find_map(|f| split_id(f)) {
        info.artists.push(ArtistHint {
            name: artist.to_string(),
            id: None,
            sources: vec![source(name, Kind::Artist, "artist_id", json!(artist_id))],
            platform_name: None,
        });
    }
    info
}

/// What the platform says about each song, from a JSON file mapping song ids to entries:
///
/// ```text
/// NetEase: {"1000000111": {"album_id": 100000074, "song_index": 3}, ...}
/// Kuwo:    {"100000518": {"album_id": 10000592, "song_index": 6,
///           "song_artists": [{"artist_id": 1000555, "artist_name": "Tamsin", "role": "main"}]}, ...}
/// QQ:      {"100001591": {"song_mid": "0019aB357c2XYz", "album_id": 1001628,
///           "album_mid": "003DyAkp4gGLDw", "song_index": 1,
///           "song_artists": [{"artist_id": 1001665, "artist_mid": "003kRnbM5UF68m",
///                             "artist_name": "Ghost Mountain", "role": "main"}]}, ...}
/// ```
///
/// (Other fields, like names, are ignored.)
///
/// * `album_id` -1 (or 0, which isn't a real id either) means the platform has no album
///   for the song: it gets no album id at all.
/// * `song_index` is inconsistent (numbers skipped or used twice), so the track's position
///   on the album isn't taken from it directly: an album's songs are sorted by it, ties
///   going to whichever song comes first in the file, and numbered 1, 2, 3... in that
///   order. The index itself is kept, verbatim, in the song's source (`song_index`), so
///   nothing is lost and the map can be rebuilt from the library.
/// * `song_artists` (QQ, Kuwo): the ids of every artist of the song, featured ones too. Like
///   the artist folder, each is only used for the tag artist with that name. An artist
///   without a real id (0 or negative) is left out. `album_artists` (Kuwo, sometimes): the
///   same for the album's artists (matched against ARTIST and ALBUM ARTIST alike). Roles
///   aren't used: credits come from the tags.
pub struct IdMap {
    platform: Platform,
    songs: HashMap<i64, Song>,
}

/// One song in the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    /// As the platform has it (`song_name`), for `find_by_tags`.
    pub title: Option<String>,
    pub placement: Placement,
    pub song_mid: Option<String>,
    pub album_mid: Option<String>,
    pub artists: Vec<MapArtist>,
    /// The album's artists, when the map has them (Kuwo, sometimes).
    pub album_artists: Vec<MapArtist>,
    /// As the map has it (e.g. "2021-09-06"); only used if the tags have no date.
    pub release_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    NoAlbum,
    Album {
        album_id: i64,
        /// The clean position: 1, 2, 3...
        track_number: i16,
        /// As the platform has it.
        song_index: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapArtist {
    pub name: String,
    pub artist_id: i64,
    pub artist_mid: Option<String>,
}

/// An entry as written in the file (both formats: the QQ fields are optional).
#[derive(Deserialize)]
struct MapEntry {
    #[serde(default)]
    song_name: Option<String>,
    album_id: i64,
    song_index: i64,
    #[serde(default)]
    song_mid: Option<String>,
    #[serde(default)]
    album_mid: Option<String>,
    /// Missing or `null`: none.
    #[serde(default)]
    song_artists: Option<Vec<EntryArtist>>,
    /// The same for the album (no roles). Missing or `null`: none.
    #[serde(default)]
    album_artists: Option<Vec<EntryArtist>>,
    /// Missing, `null` or empty: none.
    #[serde(default)]
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct EntryArtist {
    artist_id: i64,
    #[serde(default)]
    artist_mid: Option<String>,
    artist_name: String,
}

/// An empty value (a mid, a date) is none.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|m| !m.trim().is_empty())
}

impl IdMap {
    pub fn load(platform: Platform, path: &Path) -> anyhow::Result<IdMap> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        IdMap::from_json(platform, &text).with_context(|| format!("reading {}", path.display()))
    }

    pub fn from_json(platform: Platform, text: &str) -> anyhow::Result<IdMap> {
        let InOrder(entries) = serde_json::from_str(text)?;

        // Each album's songs: (song_index, place in the file, song id).
        let mut albums: HashMap<i64, Vec<(i64, usize, i64)>> = HashMap::new();
        let mut songs = HashMap::new();
        for (place, (song_id, entry)) in entries.into_iter().enumerate() {
            let song_id: i64 = song_id
                .parse()
                .with_context(|| format!("\"{song_id}\" isn't a {} song id", platform.shown()))?;
            let has_album = entry.album_id > 0;
            if has_album {
                albums.entry(entry.album_id).or_default().push((entry.song_index, place, song_id));
            }
            // Artists without a real id (0 or negative) are left out.
            let real = |list: Option<Vec<EntryArtist>>| -> Vec<MapArtist> {
                list.unwrap_or_default()
                    .into_iter()
                    .filter(|a| a.artist_id > 0)
                    .map(|a| MapArtist { name: a.artist_name, artist_id: a.artist_id, artist_mid: non_empty(a.artist_mid) })
                    .collect()
            };
            let artists = real(entry.song_artists);
            let album_artists = real(entry.album_artists);
            songs.insert(
                song_id,
                Song {
                    title: entry.song_name,
                    placement: Placement::NoAlbum, // set below for songs on an album
                    song_mid: non_empty(entry.song_mid),
                    album_mid: if has_album { non_empty(entry.album_mid) } else { None },
                    artists,
                    album_artists,
                    release_date: non_empty(entry.release_date),
                },
            );
        }
        for (album_id, mut tracks) in albums {
            // Tuples sort by their first field, then the second: by index, ties by place.
            tracks.sort();
            for (n, (song_index, _, song_id)) in tracks.into_iter().enumerate() {
                let track_number = i16::try_from(n + 1).unwrap_or(i16::MAX);
                if let Some(song) = songs.get_mut(&song_id) {
                    song.placement = Placement::Album { album_id, track_number, song_index };
                }
            }
        }
        Ok(IdMap { platform, songs })
    }

    pub fn get(&self, song_id: i64) -> Option<&Song> {
        self.songs.get(&song_id)
    }

    /// For a file without a song id in its name (`--tag-fallback`): the one song in the
    /// map with the tags' title (ignoring case and an "(Explicit)" / "[Explicit]" marker)
    /// and one of the tags' artists among its artists (or no artists listed). None if
    /// there isn't exactly one.
    pub fn find_by_tags(&self, title: &str, artists: &[String]) -> Option<i64> {
        let plain = |t: &str| {
            let lower = t.to_lowercase();
            lower.replace("(explicit)", "").replace("[explicit]", "").trim().to_string()
        };
        let wanted = plain(title);
        let mut found = self.songs.iter().filter(|(_, song)| {
            song.title.as_deref().is_some_and(|t| plain(t) == wanted)
                && (song.artists.is_empty()
                    || song.artists.iter().any(|a| artists.iter().any(|t| t.to_lowercase() == a.name.to_lowercase())))
        });
        match (found.next(), found.next()) {
            (Some((id, _)), None) => Some(*id),
            _ => None,
        }
    }

    /// `--tag-fallback`, for a file whose name has no song id: finds its song by the tags
    /// (`find_by_tags`). Failing that, for a platform whose songs may lack an id (Kuwo),
    /// the file's origin is the platform without a song id. Either way it's said in the
    /// warnings.
    pub fn fall_back_to_tags(&self, info: &mut PathInfo, title: Option<&str>, artists: &[String]) {
        if info.track.is_some() {
            return;
        }
        let shown = self.platform.shown();
        info.warnings.clear(); // "no song id in the name": dealt with here
        if let Some(song_id) = title.and_then(|t| self.find_by_tags(t, artists)) {
            info.track = Some(source(self.platform.name(), Kind::Track, "song_id", json!(song_id)));
            info.warnings.push(format!(
                "no {shown} song id in the name; found song {song_id} in the id map by the tags' title and artist (--tag-fallback)"
            ));
        } else if self.platform == Platform::Kuwo {
            info.track = Some(source_with(self.platform.name(), Kind::Track, json!({})));
            info.warnings.push(format!(
                "no {shown} song id in the name or the id map (by the tags); the origin is {shown} without a song id (--tag-fallback)"
            ));
        } else {
            info.warnings.push(format!(
                "no {shown} song id in the name, and none in the id map by the tags' title and artist"
            ));
        }
    }

    /// Puts what the map says about the file's song into `info`, over what the folders
    /// said (with a warning if they disagree about the album).
    pub fn apply(&self, info: &mut PathInfo) {
        let name = self.platform.name();
        let Some(song_id) = info
            .track
            .as_ref()
            .filter(|t| t.platform == name)
            .and_then(|t| t.external_id.as_ref())
            .and_then(|id| id.parse::<i64>().ok())
        else {
            return;
        };
        let folder_album = info.album_sources.first().and_then(|s| s.external_id.clone());
        let Some(song) = self.get(song_id) else {
            info.warnings.push(format!("{} song {song_id} isn't in the id map", self.platform.shown()));
            return;
        };
        let mids = self.platform.has_mids();

        // The song's source (also the file's origin): its ids, and the raw index.
        let mut track = json!({ "song_id": song_id });
        if let Some(song_mid) = song.song_mid.as_ref().filter(|_| mids) {
            track["song_mid"] = json!(song_mid);
        }
        match song.placement {
            Placement::NoAlbum => {
                if let Some(folder) = folder_album {
                    info.warnings.push(format!(
                        "the id map says {} has no album for this song; the folder's \
                         album id {folder} wasn't used",
                        self.platform.shown()
                    ));
                }
                info.album_sources.clear();
                info.album_cover_key = None;
            }
            Placement::Album { album_id, track_number, song_index } => {
                if let Some(folder) = folder_album.filter(|f| *f != album_id.to_string()) {
                    info.warnings.push(format!(
                        "the folder says {} album {folder}, the id map {album_id}; used the map's",
                        self.platform.shown()
                    ));
                }
                let mut album = json!({ "album_id": album_id });
                if let Some(album_mid) = song.album_mid.as_ref().filter(|_| mids) {
                    album["album_mid"] = json!(album_mid);
                }
                info.album_sources = vec![source_with(name, Kind::Album, album)];
                info.album_cover_key = Some(album_id.to_string());
                info.album_track_number = Some(track_number);
                track["song_index"] = json!(song_index);
            }
        }
        info.track = Some(source_with(name, Kind::Track, track));
        info.release_date = song.release_date.clone();

        // The song's and album's artists. One already named (by the artist folder, or as
        // both) gets the map's fuller source (with its mid) instead.
        for artist in song.artists.iter().chain(&song.album_artists) {
            let mut data = json!({ "artist_id": artist.artist_id });
            if let Some(artist_mid) = artist.artist_mid.as_ref().filter(|_| mids) {
                data["artist_mid"] = json!(artist_mid);
            }
            let artist_source = source_with(name, Kind::Artist, data);
            let id = artist.artist_id.to_string();
            let known = info.artists.iter_mut().find(|hint| {
                hint.sources.iter().any(|s| s.platform == name && s.external_id.as_deref() == Some(id.as_str()))
            });
            match known {
                Some(hint) => hint.sources = vec![artist_source],
                None => info.artists.push(ArtistHint {
                    name: artist.name.clone(),
                    id: None,
                    sources: vec![artist_source],
                    platform_name: None,
                }),
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
                f.write_str("an object of song ids")
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
        let info = read(Platform::Netease, &root, &path);
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
            ids(&read(Platform::Netease, &root, &path)),
            (Some("1000000185".into()), None, vec![("Ravel.".into(), "10000037".into())])
        );
    }

    #[test]
    fn starting_inside_an_album_folder() {
        // The album folder is the root: there's no artist folder to read.
        let root = PathBuf::from("/music/netease/Ravel. - 10000037/FLACs/Albums/2020 - 100000074");
        let path = root.join("03 - Late Friday - 1000000148.flac");
        assert_eq!(
            ids(&read(Platform::Netease, &root, &path)),
            (Some("1000000148".into()), Some("100000074".into()), vec![])
        );
    }

    #[test]
    fn album_map_orders_by_index_then_file_order() {
        // Album 5: index 2 twice (the first in the file wins the tie), index 1, and a gap.
        // Song 30 has no album (-1), song 31 neither (0).
        let map = IdMap::from_json(
            Platform::Netease,
            r#"{"20": {"album_id": 5, "song_index": 2}, "10": {"album_id": 5, "song_index": 7},
                "21": {"album_id": 5, "song_index": 2}, "22": {"album_id": 5, "song_index": 1},
                "30": {"album_id": -1, "song_index": 1}, "31": {"album_id": 0, "song_index": 0}}"#,
        )
        .unwrap();
        let at = |n, song_index| Some(Placement::Album { album_id: 5, track_number: n, song_index });
        assert_eq!(map.get(22).map(|s| s.placement), at(1, 1));
        assert_eq!(map.get(20).map(|s| s.placement), at(2, 2));
        assert_eq!(map.get(21).map(|s| s.placement), at(3, 2));
        assert_eq!(map.get(10).map(|s| s.placement), at(4, 7));
        assert_eq!(map.get(30).map(|s| s.placement), Some(Placement::NoAlbum));
        assert_eq!(map.get(31).map(|s| s.placement), Some(Placement::NoAlbum));
        assert_eq!(map.get(99).map(|s| s.placement), None);
    }

    #[test]
    fn album_map_gives_singles_their_album() {
        let map = IdMap::from_json(
            Platform::Netease,
            r#"{"1000000185": {"album_id": 100001036, "song_index": 1},
                "1000000222": {"album_id": 0, "song_index": 0},
                "1000000111": {"album_id": 999, "song_index": 1}}"#,
        )
        .unwrap();
        let root = PathBuf::from("/music/netease");
        let artist = root.join("Ravel. - 10000037/FLACs");

        let mut single = read(Platform::Netease, &root, &artist.join("Singles/Pause - 1000000185.flac"));
        map.apply(&mut single);
        assert_eq!(ids(&single).1.as_deref(), Some("100001036"));
        assert_eq!(single.album_track_number, Some(1));
        assert_eq!(single.track.as_ref().unwrap().data, json!({"song_id": 1000000185, "song_index": 1}));
        assert!(single.warnings.is_empty());

        let mut no_album = read(Platform::Netease, &root, &artist.join("Singles/Holidy Freestyle - 1000000222.flac"));
        map.apply(&mut no_album);
        assert_eq!(ids(&no_album).1, None);

        // The map wins over the folder, with a warning.
        let mut disagree = read(Platform::Netease, &root, &artist.join("Albums/2020 - 100000074/01 - Right Now - 1000000111.flac"));
        map.apply(&mut disagree);
        assert_eq!(ids(&disagree).1.as_deref(), Some("999"));
        assert_eq!(disagree.warnings.len(), 1);

        let mut unknown = read(Platform::Netease, &root, &artist.join("Singles/Keep Low - 1000000259.flac"));
        map.apply(&mut unknown);
        assert!(unknown.warnings[0].contains("isn't in the id map"));
    }

    #[test]
    fn folders_without_ids() {
        let root = PathBuf::from("test-audio/VANTA");
        let path = root.join("Albums/Tidal, Vol. 2/10 - Go Further! (Bonus Track) - 1000000296.flac");
        assert_eq!(ids(&read(Platform::Netease, &root, &path)), (Some("1000000296".into()), None, vec![]));

        let info = read(Platform::Netease, &root, &root.join("whiteout.flac"));
        assert_eq!(ids(&info), (None, None, vec![]));
        assert_eq!(info.warnings.len(), 1);
    }

    #[test]
    fn qq_map_gives_all_ids_and_every_artist() {
        let map = IdMap::from_json(
            Platform::Qq,
            r#"{"100000370": {"song_id": 100000370, "song_mid": "001abc", "song_name": "x", "song_index": 4,
                              "album_id": 77, "album_mid": "002def", "album_name": "y",
                              "song_artists": [
                                {"artist_id": 1000333, "artist_mid": "003ghi", "artist_name": "$ilver40k", "role": "main"},
                                {"artist_id": 555, "artist_mid": "004jkl", "artist_name": "NOCTURNO", "role": "feature"},
                                {"artist_id": -1029, "artist_mid": "", "artist_name": "Nobody", "role": "feature"}]},
                "1": {"song_index": 0, "album_id": 0, "album_mid": "unknown_mid", "song_artists": []}}"#,
        )
        .unwrap();
        let root = PathBuf::from("/music/qq/discographies");
        let mut info = read(
            Platform::Qq,
            &root,
            &root.join("$ilver40k - 1000333/FLACs/Glass Floors Shatter(feat. NOCTURNO) - 100000370.flac"),
        );
        map.apply(&mut info);
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        let track = info.track.as_ref().unwrap();
        assert_eq!(track.platform, "qq");
        assert_eq!(track.data, json!({"song_id": 100000370, "song_mid": "001abc", "song_index": 4}));
        assert_eq!(info.album_sources[0].data, json!({"album_id": 77, "album_mid": "002def"}));
        assert_eq!(info.album_track_number, Some(1));
        // The folder's artist gets its mid; the featured one is added; the one without a
        // real id is left out.
        let artists: Vec<(String, serde_json::Value)> =
            info.artists.iter().map(|a| (a.name.clone(), a.sources[0].data.clone())).collect();
        assert_eq!(
            artists,
            vec![
                ("$ilver40k".to_string(), json!({"artist_id": 1000333, "artist_mid": "003ghi"})),
                ("NOCTURNO".to_string(), json!({"artist_id": 555, "artist_mid": "004jkl"})),
            ]
        );

        // No album (0): no album id, and its "unknown_mid" isn't kept.
        let mut no_album = read(Platform::Qq, &root, &root.join("X - 9/FLACs/Song - 1.flac"));
        map.apply(&mut no_album);
        assert!(no_album.album_sources.is_empty());
        assert_eq!(no_album.track.unwrap().data, json!({"song_id": 1}));
    }

    #[test]
    fn kuwo_map_without_mids_and_with_null_artists() {
        let map = IdMap::from_json(
            Platform::Kuwo,
            r#"{"100000518": {"song_id": 100000518, "song_index": 6, "album_id": 10000592,
                              "song_artists": [{"artist_id": 1000555, "artist_name": "Tamsin", "role": "main"},
                                               {"artist_id": 0, "artist_name": "Unknown Singer", "role": "feature"}]},
                "100000629": {"song_index": 1, "album_id": 10000666, "song_artists": null}}"#,
        )
        .unwrap();
        let root = PathBuf::from("/music/kuwo/discographies");
        let mut info = read(Platform::Kuwo, &root, &root.join("Tamsin - 1000555/FLACs/Lulu Mimi - 100000518.flac"));
        map.apply(&mut info);
        assert_eq!(info.track.as_ref().unwrap().data, json!({"song_id": 100000518, "song_index": 6}));
        assert_eq!(info.album_sources[0].data, json!({"album_id": 10000592}));
        assert_eq!(info.artists.len(), 1, "Unknown Singer (id 0) left out");
        assert_eq!(info.artists[0].sources[0].data, json!({"artist_id": 1000555}));

        let mut none = read(Platform::Kuwo, &root, &root.join("X - 1/FLACs/Y - 100000629.flac"));
        map.apply(&mut none);
        assert_eq!(none.album_sources[0].data, json!({"album_id": 10000666}));
    }

    #[test]
    fn tag_fallback_finds_one_song_by_title_and_artist() {
        let map = IdMap::from_json(
            Platform::Kuwo,
            r#"{"100000444": {"song_name": "counted the stars twice (Explicit)", "song_index": 4, "album_id": 9,
                              "song_artists": [{"artist_id": 10000481, "artist_name": "painluvr", "role": "main"}]},
                "100000407": {"song_name": "daydreaming", "song_index": 1, "album_id": 8, "song_artists": null},
                "5": {"song_name": "Intro", "song_index": 1, "album_id": 7, "song_artists": null},
                "6": {"song_name": "Intro", "song_index": 1, "album_id": 6, "song_artists": null}}"#,
        )
        .unwrap();
        let tagged = |a: &str| vec![a.to_string()];
        // Title without "(Explicit)", artist in any case.
        assert_eq!(map.find_by_tags("counted the stars twice", &tagged("PAINLUVR")), Some(100000444));
        // Another artist: no.
        assert_eq!(map.find_by_tags("counted the stars twice", &tagged("someone")), None);
        // No artists listed: the title is enough.
        assert_eq!(map.find_by_tags("Daydreaming", &tagged("verdigrix")), Some(100000407));
        // Two songs with that title: none.
        assert_eq!(map.find_by_tags("Intro", &tagged("x")), None);

        // Found: the map fills in the rest like for a named file.
        let root = PathBuf::from("/music/kuwo/discographies");
        let mut info = read(Platform::Kuwo, &root, &root.join("painluvr - counted the stars twice (Explicit).m4a"));
        map.fall_back_to_tags(&mut info, Some("counted the stars twice"), &tagged("painluvr"));
        map.apply(&mut info);
        assert_eq!(info.track.as_ref().unwrap().data, json!({"song_id": 100000444, "song_index": 4}));
        assert_eq!(info.album_sources[0].data, json!({"album_id": 9}));
        assert!(info.warnings[0].contains("found song 100000444"), "{:?}", info.warnings);

        // Not found: Kuwo without a song id.
        let mut unknown = read(Platform::Kuwo, &root, &root.join("wavy el - zug.flac"));
        map.fall_back_to_tags(&mut unknown, Some("zug"), &tagged("wavy el"));
        let track = unknown.track.unwrap();
        assert_eq!((track.platform.as_str(), track.external_id, track.data), ("kuwo", None, json!({})));
    }

    #[test]
    fn bare_id_file_names_album_artists_and_dates() {
        let map = IdMap::from_json(
            Platform::Kuwo,
            r#"{"10000703": {"song_index": 11, "album_id": 1000740,
                             "song_artists": [{"artist_id": 1000777, "artist_name": "Turnip", "role": "main"}],
                             "album_artists": [{"artist_id": 10000814, "artist_name": "E_STORM"},
                                               {"artist_id": 1000777, "artist_name": "Turnip"}],
                             "release_date": "2019-04-20"}}"#,
        )
        .unwrap();
        let root = PathBuf::from("/music/kuwo/discographies");
        let mut info = read(Platform::Kuwo, &root, &root.join("broken/10000703.flac"));
        map.apply(&mut info);
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        assert_eq!(info.track.as_ref().unwrap().external_id.as_deref(), Some("10000703"));
        let names: Vec<&str> = info.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Turnip", "E_STORM"], "Turnip once, though listed twice");
        assert_eq!(info.release_date.as_deref(), Some("2019-04-20"));
        // A folder that's just a number isn't an album id.
        let year = read(Platform::Kuwo, &root, &root.join("X - 1/2020/5.flac"));
        assert!(year.album_sources.is_empty());
    }
}
