//! What a YouTube video's yt-dlp JSON says about it, for making a track of a file that is
//! that video (an extra with `Title [id].mp4`, see `api::extras::to_track`). The track
//! editor reads the same JSON on the client (`clients/core`, editor/import.rs); the two
//! take the same fields the same way.
//!
//! The artist: yt-dlp's music fields (`artists` / `artist`, `creators` / `creator`), else
//! the channel (whose account it is then, kept as the artist's source). The title: `track`,
//! else the video's.

use serde_json::{json, Value};

use super::layouts::ytdlp::{bracket_id, is_youtube_id};

/// A video, as far as a track needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    /// The channel, when it is the artist: stored as the first artist's source.
    pub channel: Option<String>,
    pub release_date: Option<String>,
    /// The track's source data (`song_id`, `song_description`, `song_release_date`).
    pub source: Value,
    pub thumbnail: Option<String>,
}

fn str_of<'a>(json: &'a Value, key: &str) -> Option<&'a str> {
    json.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub fn parse(json: &Value) -> Result<Video, String> {
    let id = str_of(json, "id").ok_or("the yt-dlp JSON has no video id")?.to_string();
    let title = str_of(json, "track").or(str_of(json, "title")).ok_or("the yt-dlp JSON has no title")?.to_string();
    let list = |key: &str| -> Vec<String> {
        json.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default()
    };
    let one = |key: &str| str_of(json, key).map(|s| vec![s.to_string()]).unwrap_or_default();
    let music = [list("artists"), one("artist"), list("creators"), one("creator")].into_iter().find(|l| !l.is_empty());
    let (artists, channel) = match music {
        Some(names) => (names, None),
        None => {
            let name = str_of(json, "channel").or(str_of(json, "uploader")).ok_or("the yt-dlp JSON says no artist and no channel")?.to_string();
            // Stored as the handle or the id, as a download folder names it.
            let stored = str_of(json, "channel_id").map(str::to_string).or_else(|| str_of(json, "uploader_id").map(|h| h.trim_start_matches('@').to_string()).filter(|h| !h.is_empty()));
            (vec![name], stored)
        }
    };
    let iso = |d: &str| if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) { format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]) } else { d.to_string() };
    let uploaded = str_of(json, "upload_date").map(iso);
    let release_date = str_of(json, "release_date").map(iso).or(uploaded.clone());
    let mut source = json!({ "song_id": id });
    if let Some(d) = str_of(json, "description") {
        source["song_description"] = json!(d);
    }
    if let Some(d) = &uploaded {
        source["song_release_date"] = json!(d);
    }
    Ok(Video { id, title, artists, channel, release_date, source, thumbnail: str_of(json, "thumbnail").map(str::to_string) })
}

/// The video id at the end of a file's name (`Title [id].mp4`, or the name without its
/// extension), if there is one.
pub fn id_in_name(name: &str) -> Option<String> {
    let name = name.trim();
    let stem = name.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4 && ext.bytes().all(|b| b.is_ascii_alphanumeric())).map_or(name, |(stem, _)| stem);
    bracket_id(stem).filter(|id| is_youtube_id(id)).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_in_names() {
        assert_eq!(id_in_name("Mira Vale - Glass Harbor [dQw4w9WgXcQ].mp4").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id_in_name("Glass Harbor [dQw4w9WgXcQ]").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id_in_name("Glass Harbor [prod. Bo].mp4"), None);
        assert_eq!(id_in_name("Glass Harbor.mp4"), None);
    }

    #[test]
    fn a_video() {
        let video = json!({
            "id": "dQw4w9WgXcQ", "title": "Mira Vale - Glass Harbor (Official Video)",
            "channel": "Mira Vale", "channel_id": "UCabc", "uploader_id": "@miravale",
            "upload_date": "20210604", "description": "", "thumbnail": "https://i.ytimg.com/vi/x/maxresdefault.jpg"
        });
        let v = parse(&video).unwrap();
        assert_eq!((v.title.as_str(), v.release_date.as_deref(), v.channel.as_deref()), ("Mira Vale - Glass Harbor (Official Video)", Some("2021-06-04"), Some("UCabc")));
        assert_eq!(v.artists, ["Mira Vale"]);
        assert_eq!(v.source, json!({ "song_id": "dQw4w9WgXcQ", "song_release_date": "2021-06-04" }));

        // YouTube Music's fields: those artists, no channel; its track name.
        let mut music = video.clone();
        music["artists"] = json!(["Mira Vale", "Bo"]);
        music["track"] = json!("Glass Harbor");
        music["release_date"] = json!("20210101");
        let v = parse(&music).unwrap();
        assert_eq!((v.title.as_str(), v.artists.len(), v.channel, v.release_date.as_deref()), ("Glass Harbor", 2, None, Some("2021-01-01")));
    }
}
