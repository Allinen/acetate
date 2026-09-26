//! SoundCloud discographies, one folder per uploader, named by their user id:
//!
//! ```text
//! 100000851/                                        the uploader's id (no name)
//!   streams/Singles/night-drive-livexll - 1000001073 - CxLm73Rq58Wd.m4a
//!                   <song permalink>     - <song id>  - <song uid>
//!   FLACs/Singles/plan_c - 1000000999 - 29KdQ4vbNmTe.wav
//!                 <original file name> - <song id> - <song uid>
//!   FLACs/Albums/1000000888/01 - fade slow ( loremyas, kiraboii) - 1000000925 - Hk2Wp8Rt5Ys1.wav
//!                <album id>/<position> - <original file name> - <song id> - <song uid>
//!   Covers/{Singles,Albums}/<artwork key>.jpg      saved artworks (see `Artworks`)
//! ```
//!
//! * The song id and uid (the uploaded file's id: a song whose audio was replaced has
//!   several) end the file name. The song on SoundCloud is the file's origin.
//! * The uploader's id goes to the first artist in the ARTIST tag, whatever its name: the
//!   folder has no name to match. Other artists come from the tags alone.
//! * `Albums/<album id>/` is the album, and the number the file name starts with is the
//!   track's position on it. `Singles` and `Albums` say single or album, as everywhere.
//!
//! The lookup (`--id-map`, see `ScMap`) adds what SoundCloud says about each song, album
//! and uploader. Its release date and status win over the tags. Covers come from its
//! artwork URLs instead of the embedded pictures (those are resized copies).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};

use super::PathInfo;
use crate::model::sources::{self, Kind, NewSource, ValidSource};

const PLATFORM: &str = "soundcloud";

/// A SoundCloud source with these fields (and when they were captured, if known). Panics
/// only if they don't fit the shape in `SHAPES`, which is a bug here.
fn sc_source(kind: Kind, data: Value, captured_at: Option<DateTime<Utc>>) -> ValidSource {
    let new = NewSource { platform: PLATFORM.to_string(), source_id: None, captured_at, data };
    sources::validate(kind, new).expect("the SoundCloud layout only builds sources that fit their shape")
}

/// Reads `path` (below `root`). See the module docs for the layout.
pub fn read(root: &Path, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();
    let file_name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

    // "<…> - <song id> - <song uid>"
    let mut parts = file_name.rsplitn(3, " - ");
    let (uid, song_id, rest) = (parts.next(), parts.next(), parts.next());
    let song_id = song_id.and_then(|id| id.parse::<i64>().ok());
    let uid = uid.filter(|u| !u.is_empty() && u.chars().all(|c| c.is_ascii_alphanumeric()));
    match (song_id, uid, rest) {
        (Some(song_id), Some(uid), Some(rest)) => {
            info.track = Some(sc_source(Kind::Track, json!({ "song_id": song_id, "song_uids": [uid] }), None));

            // In an album folder: "<position> - …".
            let parent = path.parent();
            let album_id = parent
                .filter(|p| p.parent().and_then(|pp| pp.file_name()).is_some_and(|n| n.eq_ignore_ascii_case("albums")))
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .and_then(|n| n.parse::<i64>().ok());
            if let Some(album_id) = album_id {
                info.album_sources = vec![sc_source(Kind::Album, json!({ "album_id": album_id }), None)];
                let position = rest.split_once(" - ").and_then(|(n, _)| n.trim().parse::<i16>().ok());
                match position {
                    Some(n) => info.album_track_number = Some(n),
                    None => info.warnings.push("no position (\"05 - …\") at the start of the name in an album folder".into()),
                }
            }
        }
        _ => info
            .warnings
            .push("the name doesn't end in \" - <song id> - <song uid>\", so there's no SoundCloud id".into()),
    }

    // The uploader: the folder holding `FLACs` or `streams`, if it's an id, from `root` down.
    let uploader = path.ancestors().skip(1).find_map(|folder| {
        let name = folder.file_name()?.to_str()?;
        if !(name.eq_ignore_ascii_case("flacs") || name.eq_ignore_ascii_case("streams")) {
            return None;
        }
        let artist_folder = folder.parent()?;
        if !artist_folder.starts_with(root) && artist_folder != root {
            return None;
        }
        let id = artist_folder.file_name()?.to_str()?.parse::<i64>().ok()?;
        Some((id, artist_folder.to_path_buf()))
    });
    match uploader {
        Some((artist_id, folder)) => {
            info.first_artist_sources = vec![sc_source(Kind::Artist, json!({ "artist_id": artist_id }), None)];
            info.cover_folder = Some(folder.join("Covers"));
        }
        None => info.warnings.push("no \"<user id>/FLACs\" or \"<user id>/streams\" folder above it, so no uploader id".into()),
    }
    info
}

/// `null` in the lookup is the same as leaving the field out.
fn null_is_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The lookup for `--layout soundcloud` (`--id-map`): what SoundCloud says about each song,
/// album and uploader, gathered over time. Every list is deduplicated, oldest capture
/// first. Permalinks are the part after "soundcloud.com/".
///
/// ```json
/// {"artists": {"<user id>": {"permalinks": [...]}},
///  "albums":  {"<album id>": {"permalinks": [...], "release_dates": [...], "artworks": [...]}},
///  "songs":   {"<song id>": {"song_uids": [...], "permalinks": [...], "descriptions": [...],
///                            "release_dates": [...], "created_at": [...], "artworks": [...],
///                            "captured_at": "...", "deleted": true | false | null}}}
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ScMap {
    #[serde(default, deserialize_with = "null_is_default")]
    artists: HashMap<i64, MapArtist>,
    #[serde(default, deserialize_with = "null_is_default")]
    albums: HashMap<i64, MapAlbum>,
    #[serde(default, deserialize_with = "null_is_default")]
    songs: HashMap<i64, MapSong>,
}

#[derive(Debug, Default, Deserialize)]
struct MapArtist {
    #[serde(default, deserialize_with = "null_is_default")]
    permalinks: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MapAlbum {
    #[serde(default, deserialize_with = "null_is_default")]
    permalinks: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    release_dates: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    artworks: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MapSong {
    #[serde(default, deserialize_with = "null_is_default")]
    song_uids: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    permalinks: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    descriptions: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    release_dates: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    created_at: Vec<String>,
    #[serde(default, deserialize_with = "null_is_default")]
    artworks: Vec<String>,
    #[serde(default)]
    captured_at: Option<String>,
    #[serde(default)]
    deleted: Option<bool>,
    /// The original files' names, by uid (only for original downloads, `FLACs/`). null:
    /// none was saved.
    #[serde(default, deserialize_with = "null_is_default")]
    original_filenames: HashMap<String, Option<String>>,
}

/// The original filename for a file whose download saved none (the lookup has its uid,
/// with `null`). So it's clear the name is unknown, not forgotten.
pub const NO_FILENAME_PROVIDED: &str = "NO_FILENAME_PROVIDED";

/// Adds `values` to `data` as `field`, unless there are none.
fn put_list(data: &mut Map<String, Value>, field: &str, values: &[String]) {
    if !values.is_empty() {
        data.insert(field.to_string(), json!(values));
    }
}

/// The oldest of these dates. Timestamps ("2015-06-20T15:33:32Z") and plain dates are
/// compared as points in time; ones that are neither are left out, unless nothing else
/// is there (then the first).
fn oldest(dates: &[String]) -> Option<String> {
    let when = |d: &str| -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(d)
            .map(|t| t.with_timezone(&Utc))
            .ok()
            .or_else(|| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0).map(|t| t.and_utc()))
    };
    dates
        .iter()
        .filter_map(|d| when(d).map(|t| (t, d)))
        .min_by_key(|(t, _)| *t)
        .map(|(_, d)| d.clone())
        .or_else(|| dates.first().cloned())
}

impl ScMap {
    pub fn load(path: &Path) -> anyhow::Result<ScMap> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }

    /// Puts what the lookup says about the file's song, album and uploader into `info`.
    pub fn apply(&self, info: &mut PathInfo) {
        info.skip_embedded_covers = true;
        // The uploader and the album folder's album, whether the song is in the lookup or not.
        self.apply_uploader(info);
        self.apply_album(info);
        let Some(track) = info.track.as_ref().filter(|t| t.platform == PLATFORM) else {
            return;
        };
        let Some(song_id) = track.external_id.as_deref().and_then(|id| id.parse::<i64>().ok()) else {
            return;
        };
        let file_uid = track.data["song_uids"][0].as_str().unwrap_or_default().to_string();
        let Some(song) = self.songs.get(&song_id) else {
            info.warnings.push(format!("SoundCloud song {song_id} isn't in the lookup"));
            return;
        };

        // The song: its source (the file's origin), date, status and covers.
        let mut uids = song.song_uids.clone();
        if !uids.contains(&file_uid) {
            info.warnings.push(format!("the lookup doesn't list this file's uid {file_uid} for song {song_id}; added it"));
            uids.push(file_uid.clone());
        }
        let mut data = Map::new();
        data.insert("song_id".into(), json!(song_id));
        put_list(&mut data, "song_uids", &uids);
        put_list(&mut data, "song_permalinks", &song.permalinks);
        put_list(&mut data, "song_descriptions", &song.descriptions);
        put_list(&mut data, "release_dates", &song.release_dates);
        put_list(&mut data, "created_at", &song.created_at);
        let captured_at = song.captured_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
        info.track = Some(sc_source(Kind::Track, Value::Object(data), captured_at));

        info.release_date = oldest(&song.release_dates);
        info.release_date_over_tags = true;
        match song.deleted {
            Some(true) => info.status = Some("deleted".into()),
            Some(false) => info.status = Some("released".into()),
            None => info.warnings.push(format!("the lookup doesn't say whether song {song_id} is deleted; used --status")),
        }
        info.track_cover_urls = song.artworks.clone();
        // An original download's name (the tags' wins if they have one, see `ingest`).
        if let Some(name) = song.original_filenames.get(&file_uid) {
            info.original_filename = Some(name.clone().unwrap_or_else(|| NO_FILENAME_PROVIDED.to_string()));
        }

        // A single's release is the song itself: same date and covers.
        if info.album_sources.is_empty() {
            info.album_release_date = info.release_date.clone();
            info.album_cover_urls = song.artworks.clone();
        }
    }

    /// The album folder's album: its permalinks, date and covers.
    fn apply_album(&self, info: &mut PathInfo) {
        let Some(album_id) = info
            .album_sources
            .first()
            .and_then(|s| s.external_id.as_deref())
            .and_then(|id| id.parse::<i64>().ok())
        else {
            return;
        };
        let mut data = Map::new();
        data.insert("album_id".into(), json!(album_id));
        match self.albums.get(&album_id) {
            Some(album) => {
                put_list(&mut data, "album_permalinks", &album.permalinks);
                put_list(&mut data, "release_dates", &album.release_dates);
                info.album_release_date = oldest(&album.release_dates);
                info.album_cover_urls = album.artworks.clone();
            }
            None => info.warnings.push(format!("SoundCloud album {album_id} isn't in the lookup")),
        }
        info.album_sources = vec![sc_source(Kind::Album, Value::Object(data), None)];
    }

    /// The uploader's permalinks.
    fn apply_uploader(&self, info: &mut PathInfo) {
        for source in &mut info.first_artist_sources {
            let Some(artist_id) = source.external_id.as_deref().and_then(|id| id.parse::<i64>().ok()) else {
                continue;
            };
            let mut data = Map::new();
            data.insert("artist_id".into(), json!(artist_id));
            match self.artists.get(&artist_id) {
                Some(artist) => put_list(&mut data, "artist_permalinks", &artist.permalinks),
                None => info.warnings.push(format!("SoundCloud user {artist_id} isn't in the lookup")),
            }
            *source = sc_source(Kind::Artist, Value::Object(data), None);
        }
    }
}

/// Fetches cover images by URL, for `--layout soundcloud`: from a saved copy in the
/// artist's `Covers/` folder if there is one (only read, never written), else downloaded
/// into memory. SoundCloud artwork URLs
/// are usually for a small size ("…-large.jpg"), so the original is tried first
/// ("-original.jpg", then ".png", as originals keep their format), then the URL as given.
/// Images are only held in memory until they're stored. The last `KEEP` are kept for
/// reuse (an album's tracks follow each other, so its covers are fetched once), and URLs
/// that failed are remembered so they aren't tried again.
pub struct Artworks {
    client: reqwest::Client,
    /// The most recently fetched images, oldest first.
    recent: VecDeque<(String, Vec<u8>)>,
    failed: HashSet<String>,
    /// Per `Covers/` folder: artwork key → saved file.
    saved: HashMap<PathBuf, HashMap<String, PathBuf>>,
}

/// How many fetched images `Artworks` keeps for reuse.
const KEEP: usize = 32;

/// The artwork key in a SoundCloud artwork URL: "…/artworks-000111222333-k7xqpa-large.jpg"
/// → "000111222333-k7xqpa". Saved copies are named by it.
fn artwork_key(url: &str) -> Option<&str> {
    let name = url.rsplit('/').next()?;
    let name = name.strip_prefix("artworks-")?;
    let (stem, _ext) = name.rsplit_once('.')?;
    let (key, _size) = stem.rsplit_once('-')?;
    Some(key)
}

/// The URLs to try for an artwork, best first (see `Artworks`).
fn candidates(url: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some((base, _size_and_ext)) = url.rsplit_once('-').filter(|_| artwork_key(url).is_some()) {
        if !url.contains("-original.") {
            urls.push(format!("{base}-original.jpg"));
            urls.push(format!("{base}-original.png"));
        }
    }
    urls.push(url.to_string());
    urls
}

impl Artworks {
    pub fn new() -> anyhow::Result<Artworks> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("setting up the HTTP client")?;
        Ok(Artworks { client, recent: VecDeque::new(), failed: HashSet::new(), saved: HashMap::new() })
    }

    /// The images for these URLs, in order. One that can't be had is left out, with a
    /// warning.
    pub async fn get_all(&mut self, urls: &[String], folder: Option<&Path>, warnings: &mut Vec<String>) -> Vec<Vec<u8>> {
        let mut images = Vec::new();
        for url in urls {
            if let Some(bytes) = self.get(url, folder, warnings).await {
                images.push(bytes);
            }
        }
        images
    }

    async fn get(&mut self, url: &str, folder: Option<&Path>, warnings: &mut Vec<String>) -> Option<Vec<u8>> {
        if self.failed.contains(url) {
            return None;
        }
        if let Some((_, bytes)) = self.recent.iter().find(|(u, _)| u == url) {
            return Some(bytes.clone());
        }
        let result = match self.saved_copy(url, folder) {
            Some(path) => match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(err) => {
                    warnings.push(format!("cover {url}: couldn't read the saved copy {}: {err}", path.display()));
                    self.download(url, warnings).await
                }
            },
            None => self.download(url, warnings).await,
        };
        let result = result.filter(|bytes| {
            let ok = crate::model::image_info(bytes).is_some();
            if !ok {
                warnings.push(format!("cover {url}: not a JPEG, PNG, WebP or GIF image; left out"));
            }
            ok
        });
        match &result {
            Some(bytes) => {
                self.recent.push_back((url.to_string(), bytes.clone()));
                if self.recent.len() > KEEP {
                    self.recent.pop_front();
                }
            }
            None => {
                self.failed.insert(url.to_string());
            }
        }
        result
    }

    /// A saved copy of the artwork in `folder` (any subfolder), named by its key.
    fn saved_copy(&mut self, url: &str, folder: Option<&Path>) -> Option<PathBuf> {
        let key = artwork_key(url)?;
        let folder = folder?;
        let index = self.saved.entry(folder.to_path_buf()).or_insert_with(|| index_folder(folder));
        index.get(key).cloned()
    }

    async fn download(&self, url: &str, warnings: &mut Vec<String>) -> Option<Vec<u8>> {
        let mut last_error = String::new();
        for candidate in candidates(url) {
            match self.client.get(&candidate).send().await {
                Ok(response) if response.status().is_success() => match response.bytes().await {
                    Ok(bytes) => return Some(bytes.to_vec()),
                    Err(err) => last_error = format!("{candidate}: {err}"),
                },
                Ok(response) => last_error = format!("{candidate}: {}", response.status()),
                Err(err) => last_error = format!("{candidate}: {err}"),
            }
        }
        warnings.push(format!("cover {url}: couldn't download it ({last_error}); left out"));
        None
    }
}

/// The files below `folder`, by the part of their name before the first dot
/// ("000111222333-k7xqpa.jpg" and "…-m4diui.jpg.jpg" alike).
fn index_folder(folder: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let key = name.split('.').next().unwrap_or(name).to_string();
                index.entry(key).or_insert(path);
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/music/sc/discographies")
    }

    #[test]
    fn reads_songs_albums_and_the_uploader() {
        let single = read(&root(), &root().join("100000851/FLACs/Singles/plan_c - 1000000999 - 29KdQ4vbNmTe.wav"));
        assert!(single.warnings.is_empty(), "{:?}", single.warnings);
        let track = single.track.unwrap();
        assert_eq!(track.external_id.as_deref(), Some("1000000999"));
        assert_eq!(track.data["song_uids"], json!(["29KdQ4vbNmTe"]));
        assert_eq!(single.first_artist_sources[0].external_id.as_deref(), Some("100000851"));
        assert_eq!(single.cover_folder, Some(root().join("100000851/Covers")));
        assert!(single.album_sources.is_empty());

        // A permalink with " - " in it, in a stream.
        let stream = read(&root(), &root().join("100000851/streams/Singles/a - b-c - 1000001073 - CxLm73Rq58Wd.m4a"));
        assert_eq!(stream.track.unwrap().external_id.as_deref(), Some("1000001073"));

        let on_album = read(
            &root(),
            &root().join("100000851/FLACs/Albums/1000000888/05 - outline ( daynn) - 1000000962 - Qa3Zt9LmXc2R.wav"),
        );
        assert!(on_album.warnings.is_empty(), "{:?}", on_album.warnings);
        assert_eq!(on_album.album_sources[0].external_id.as_deref(), Some("1000000888"));
        assert_eq!(on_album.album_track_number, Some(5));

        // Started from the artist's own folder.
        let artist_root = root().join("100000851");
        let inside = read(&artist_root, &artist_root.join("FLACs/Singles/plan_c - 1000000999 - 29KdQ4vbNmTe.wav"));
        assert_eq!(inside.first_artist_sources[0].external_id.as_deref(), Some("100000851"));

        let odd = read(&root(), &root().join("100000851/FLACs/Albums/1000000888/folder.jpg"));
        assert!(odd.track.is_none() && !odd.warnings.is_empty());
    }

    #[test]
    fn the_lookup_fills_in_the_song_album_and_uploader() {
        let map: ScMap = serde_json::from_value(json!({
            "artists": {"100000851": {"permalinks": ["mirrorrealm", "ih8winterr"]}},
            "albums": {"1000000888": {"permalinks": ["mirrorrealm/sets/deep-radio"], "release_dates": ["2025-09-14"],
                                      "artworks": ["https://i1.sndcdn.com/artworks-CrC1y2IbxgIqXmiS-1QyhPg-original.png"]}},
            "songs": {
                "1000000962": {"song_uids": ["old0uid", "Qa3Zt9LmXc2R"], "permalinks": ["mirrorrealm/outline"],
                               "descriptions": null, "release_dates": ["2025-09-15T10:00:00Z", "2025-09-14T20:09:00Z"],
                               "created_at": ["2025-09-14T20:09:00Z"],
                               "artworks": ["https://i1.sndcdn.com/artworks-000111222333-k7xqpa-large.jpg"],
                               "captured_at": "2026-01-02T03:04:05+00:00", "deleted": true,
                               "original_filenames": {"Qa3Zt9LmXc2R": null, "old0uid": "outline.wav"}}
            }
        }))
        .unwrap();
        let mut info = read(
            &root(),
            &root().join("100000851/FLACs/Albums/1000000888/05 - outline ( daynn) - 1000000962 - Qa3Zt9LmXc2R.wav"),
        );
        map.apply(&mut info);
        assert!(info.warnings.is_empty(), "{:?}", info.warnings);
        let track = info.track.as_ref().unwrap();
        assert_eq!(
            track.data,
            json!({"song_id": 1000000962_i64, "song_uids": ["old0uid", "Qa3Zt9LmXc2R"],
                   "song_permalinks": ["mirrorrealm/outline"],
                   "release_dates": ["2025-09-15T10:00:00Z", "2025-09-14T20:09:00Z"],
                   "created_at": ["2025-09-14T20:09:00Z"]})
        );
        assert_eq!(track.captured_at.unwrap().to_rfc3339(), "2026-01-02T03:04:05+00:00");
        assert_eq!(info.release_date.as_deref(), Some("2025-09-14T20:09:00Z"), "the oldest");
        assert!(info.release_date_over_tags && info.skip_embedded_covers);
        assert_eq!(info.status.as_deref(), Some("deleted"));
        assert_eq!(info.original_filename.as_deref(), Some(NO_FILENAME_PROVIDED), "this uid's, saved as none");
        assert_eq!(info.album_sources[0].data["album_permalinks"], json!(["mirrorrealm/sets/deep-radio"]));
        assert_eq!(info.album_release_date.as_deref(), Some("2025-09-14"));
        assert_eq!(info.album_cover_urls.len(), 1);
        assert_eq!(info.first_artist_sources[0].data["artist_permalinks"], json!(["mirrorrealm", "ih8winterr"]));

        // Not in the lookup: said so, the path's ids stay.
        let mut unknown = read(&root(), &root().join("100000851/FLACs/Singles/x - 1 - abc.wav"));
        map.apply(&mut unknown);
        assert!(unknown.warnings.iter().any(|w| w.contains("isn't in the lookup")), "{:?}", unknown.warnings);
        assert!(unknown.track.is_some());
        // The uploader is still filled in.
        assert_eq!(unknown.first_artist_sources[0].data["artist_permalinks"], json!(["mirrorrealm", "ih8winterr"]));
    }

    #[test]
    fn artwork_urls() {
        let small = "https://i1.sndcdn.com/artworks-000111222333-k7xqpa-large.jpg";
        assert_eq!(artwork_key(small), Some("000111222333-k7xqpa"));
        assert_eq!(
            candidates(small),
            [
                "https://i1.sndcdn.com/artworks-000111222333-k7xqpa-original.jpg",
                "https://i1.sndcdn.com/artworks-000111222333-k7xqpa-original.png",
                small,
            ]
        );
        let original = "https://i1.sndcdn.com/artworks-CrC1y2IbxgIqXmiS-1QyhPg-original.png";
        assert_eq!(artwork_key(original), Some("CrC1y2IbxgIqXmiS-1QyhPg"));
        assert_eq!(candidates(original), [original]);
        assert_eq!(candidates("https://example.com/cover.jpg"), ["https://example.com/cover.jpg"]);
    }

    #[test]
    fn oldest_date() {
        let dates = |d: &[&str]| d.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(oldest(&dates(&["2020-01-02T00:00:00Z", "2019-05-01"])).as_deref(), Some("2019-05-01"));
        assert_eq!(oldest(&dates(&["someday"])).as_deref(), Some("someday"));
        assert_eq!(oldest(&[]), None);
    }
}
