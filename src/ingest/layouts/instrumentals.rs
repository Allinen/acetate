//! Instrumentals, one folder per producer, then one per place they came from:
//!
//! ```text
//! instrumentals/                  run on this folder (`acetate ingest <it> --layout instrumentals`)
//! OKTVA/                          the producer: the track's artist (whatever the tags say)
//!   tt@oktva/REMEDY.mp3         Traktrain (found in the Traktrain lookup, see below)
//!   yt@oktva/… [dQw4w9WgXcQ].opus YouTube (yt-dlp's "[id]")
//!   sc@oktva/… [100001332].m4a    SoundCloud (yt-dlp's "[id]")
//!   bs@oktva/… - 10001147.wav     BeatStars ("- <track id>")
//!   netease@10001221/… - 100001295.flac   NetEase (artist id, song id)
//!   tg@channel/….mp3              a Telegram channel (custom source, with its link)
//!   various/….flac                anything else (custom source)
//! ```
//!
//! * The producer folder's name is the artist, over the tags: a download's ARTIST is
//!   usually just the uploading channel. The first revision says so.
//! * Each file's origin is the most exact its folder allows; the account (`<site>@<account>`)
//!   gives the producer an id on that site where there is one.
//! * A file without a title tag gets its file name as the title (said in the revision).
//! * Traktrain downloads have no tags and no id in their names: the lookup
//!   (`--id-map`, made by `traktrain_lookup.py`) gives each one's track id, title, BPM,
//!   tags, cover and the producer's Traktrain id. Files it doesn't have get a custom
//!   "traktrain" origin.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{folder_names, source_with, ytdlp, PathInfo};
use crate::ingest::describe::GivenValues;
use crate::model::sources::Kind;

/// Reads `path` (below `root`, which is the instrumentals folder or anywhere in it).
pub fn read(root: &Path, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    // Folders from the root (the instrumentals folder) down: [root, producer, account...].
    let folders = folder_names(root, path);
    let producer = folders.get(1).cloned();
    let below = folders.get(2).cloned();
    let (site, account) = match below.as_deref().and_then(|f| f.split_once('@')) {
        Some((site, account)) => (site.to_lowercase(), account.to_string()),
        // "various", "pxlse.com"...: a custom origin by that name; a file right in the
        // producer's folder: by the producer's.
        None => (String::new(), below.clone().or_else(|| producer.clone()).unwrap_or_default()),
    };
    let Some(producer) = producer else {
        info.warnings.push("no producer folder above it".into());
        return info;
    };
    info.given = GivenValues {
        title: Some(stem.clone()),
        title_from: "the file name".into(),
        artists: vec![producer.clone()],
        artists_from: format!("the producer folder \"{producer}\""),
        artists_over_tags: true,
    };

    let custom = |label: &str, description: Option<String>, url: Option<String>| {
        let mut data = json!({ "label": label });
        if let Some(d) = description {
            data["description"] = json!(d);
        }
        if let Some(u) = url {
            data["url"] = json!(u);
        }
        source_with("custom", Kind::Track, data)
    };
    match site.as_str() {
        "yt" | "sc" => {
            let platform = if site == "yt" { ytdlp::Platform::Youtube } else { ytdlp::Platform::Soundcloud };
            let from_name = ytdlp::read(platform, path);
            info.warnings.extend(from_name.warnings);
            info.track = from_name.track;
            if site == "yt" {
                info.first_artist_sources = vec![source_with("youtube", Kind::Artist, json!({ "channel": account.trim_start_matches('@') }))];
            } else {
                info.first_artist_sources =
                    vec![source_with("soundcloud", Kind::Artist, json!({ "artist_permalinks": [account.clone()] }))];
            }
            if info.track.is_none() {
                let label = if site == "yt" { "youtube" } else { "soundcloud" };
                info.track = Some(custom(label, Some(format!("{site}@{account}, no id in the file name")), None));
            }
        }
        "bs" => {
            info.first_artist_sources = vec![source_with("beatstars", Kind::Artist, json!({ "slug": account }))];
            info.track = match stem.rsplit_once(" - ").and_then(|(_, id)| id.trim().parse::<i64>().ok()) {
                Some(id) => Some(source_with("beatstars", Kind::Track, json!({ "track_id": id }))),
                None => Some(custom("beatstars", Some(format!("bs@{account}, no id in the file name")), None)),
            };
        }
        "netease" | "163" => {
            if let Ok(artist_id) = account.parse::<i64>() {
                info.first_artist_sources = vec![source_with("netease", Kind::Artist, json!({ "artist_id": artist_id }))];
            }
            info.track = match super::id_folders::split_id(&stem) {
                Some((_, id)) => Some(source_with("netease", Kind::Track, json!({ "song_id": id }))),
                None => Some(custom("netease", Some(format!("{site}@{account}, no id in the file name")), None)),
            };
        }
        "tt" => {
            // The lookup fills this in (`TraktrainMap::apply`); without it, a custom origin.
            info.track = Some(custom(
                "traktrain",
                Some(format!("tt@{account}, not in the Traktrain lookup")),
                Some(format!("https://traktrain.com/{account}")),
            ));
        }
        "tg" => {
            info.track = Some(custom("telegram", Some(format!("@{account}")), Some(format!("https://t.me/{account}"))));
        }
        "" => {
            info.track = Some(custom(&account, None, None));
        }
        other => {
            info.track = Some(custom(other, Some(format!("{other}@{account}")), None));
        }
    }
    info
}

/// The Traktrain lookup (`traktrain_lookup.py`'s output): by file, relative to the
/// instrumentals folder ("<producer>/tt@<user>/<name>.mp3").
#[derive(Debug, Deserialize)]
pub struct TraktrainMap {
    files: HashMap<String, TraktrainFile>,
}

#[derive(Debug, Deserialize)]
struct TraktrainFile {
    track_id: i64,
    name: Option<String>,
    bpm: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    genres: Vec<Value>,
    status: Option<String>,
    audio_key: Option<String>,
    cover: Option<String>,
    producer: TraktrainProducer,
}

#[derive(Debug, Deserialize)]
struct TraktrainProducer {
    user_id: Option<i64>,
    slug: Option<String>,
    brand: Option<String>,
}

impl TraktrainMap {
    pub fn load(path: &Path) -> anyhow::Result<TraktrainMap> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
    }

    /// For a file in a `tt@` folder: its track, title, cover and producer id, if the
    /// lookup has it (by its last three path parts).
    pub fn apply(&self, path: &Path, info: &mut PathInfo) {
        let parts: Vec<String> = path.iter().rev().take(3).map(|p| p.to_string_lossy().to_string()).collect();
        if parts.len() < 3 || !parts[1].starts_with("tt@") {
            return;
        }
        let key = format!("{}/{}/{}", parts[2], parts[1], parts[0]);
        let Some(file) = self.files.get(&key) else {
            info.warnings.push("not in the Traktrain lookup: a custom \"traktrain\" origin".into());
            return;
        };
        let mut data = json!({ "track_id": file.track_id });
        let mut put = |field: &str, value: Value| {
            data[field] = value;
        };
        if let Some(name) = &file.name {
            put("name", json!(name));
        }
        if let Some(bpm) = file.bpm {
            put("bpm", json!(bpm));
        }
        if !file.tags.is_empty() {
            put("tags", json!(file.tags));
        }
        if !file.genres.is_empty() {
            let ids: Vec<String> = file.genres.iter().map(|g| g.to_string().trim_matches('"').to_string()).collect();
            put("genre_ids", json!(ids));
        }
        if let Some(status) = &file.status {
            put("status", json!(status));
        }
        if let Some(key) = &file.audio_key {
            put("audio_key", json!(key));
        }
        info.track = Some(source_with("traktrain", Kind::Track, data));
        if let Some(name) = file.name.as_ref().filter(|n| !n.trim().is_empty()) {
            info.given.title = Some(name.trim().to_string());
            info.given.title_from = "Traktrain".into();
        }
        if let Some(user_id) = file.producer.user_id {
            let mut artist = json!({ "user_id": user_id });
            if let Some(slug) = &file.producer.slug {
                artist["slug"] = json!(slug);
            }
            if let Some(brand) = &file.producer.brand {
                artist["brand"] = json!(brand);
            }
            info.first_artist_sources = vec![source_with("traktrain", Kind::Artist, artist)];
        }
        if let Some(cover) = &file.cover {
            info.track_cover_urls = vec![cover.clone()];
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/music/instrumentals")
    }

    #[test]
    fn each_site_gives_its_origin_and_the_producer_is_the_artist() {
        let yt = read(&root(), &root().join("07embers/yt@07embers15/FREE ascent type beat [mopqrT5s7-z].opus"));
        let track = yt.track.unwrap();
        assert_eq!((track.platform.as_str(), track.external_id.as_deref()), ("youtube", Some("mopqrT5s7-z")));
        assert_eq!(yt.first_artist_sources[0].external_id.as_deref(), Some("07embers15"));
        assert_eq!(yt.given.artists, ["07embers"]);
        assert!(yt.given.artists_over_tags);

        let bs = read(&root(), &root().join("$CATTERED/bs@scatteredx/INDEX - 150-rally - 10001147.wav"));
        assert_eq!(bs.track.unwrap().external_id.as_deref(), Some("10001147"));
        assert_eq!(bs.given.artists, ["$CATTERED"]);

        let ne = read(&root(), &root().join("Corvid/netease@10001221/Petal - 100001258.flac"));
        assert_eq!(ne.track.unwrap().external_id.as_deref(), Some("100001258"));
        assert_eq!(ne.first_artist_sources[0].external_id.as_deref(), Some("10001221"));

        let tg = read(&root(), &root().join("2holdyou/tg@toholdyourself/a beat.mp3"));
        let track = tg.track.unwrap();
        assert_eq!(track.platform, "custom");
        assert_eq!(track.data["url"], "https://t.me/toholdyourself");
        assert_eq!(tg.given.title.as_deref(), Some("a beat"));

        let various = read(&root(), &root().join("Corvid/various/Corvid - CorvidRapperContest.flac"));
        assert_eq!(various.track.unwrap().data["label"], "various");
        assert_eq!(various.given.artists, ["Corvid"]);
    }

    #[test]
    fn the_traktrain_lookup_gives_the_track_title_cover_and_producer() {
        let map: TraktrainMap = serde_json::from_value(json!({"files": {"OKTVA/tt@oktva/REMEDY.mp3": {
            "track_id": 101184, "name": "REMEDY ", "bpm": 150, "tags": [], "genres": [1, 15], "status": "published",
            "audio_key": "80960/x.mp3", "cover": "https://example.com/c.jpg",
            "producer": {"user_id": 80960, "slug": "oktva", "brand": "OKTVA", "display": "OKTVA"}}}}))
        .unwrap();
        let path = root().join("OKTVA/tt@oktva/REMEDY.mp3");
        let mut info = read(&root(), &path);
        assert_eq!(info.track.as_ref().unwrap().platform, "custom", "without the lookup");
        map.apply(&path, &mut info);
        let track = info.track.unwrap();
        assert_eq!((track.platform.as_str(), track.external_id.as_deref()), ("traktrain", Some("101184")));
        assert_eq!(track.data["genre_ids"], json!(["1", "15"]));
        assert_eq!((info.given.title.as_deref(), info.given.title_from.as_str()), (Some("REMEDY"), "Traktrain"));
        assert_eq!(info.first_artist_sources[0].external_id.as_deref(), Some("80960"));
        assert_eq!(info.track_cover_urls, ["https://example.com/c.jpg"]);

        let mut other = read(&root(), &root().join("OKTVA/tt@oktva/gone.mp3"));
        map.apply(&root().join("OKTVA/tt@oktva/gone.mp3"), &mut other);
        assert!(other.warnings.iter().any(|w| w.contains("not in the Traktrain lookup")));
    }
}
