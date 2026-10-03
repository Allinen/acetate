//! "Import metadata from": a SoundCloud track's JSON (the API's, a downloader's), a yt-dlp
//! info JSON, or a YouTube link (read by the server with yt-dlp, `POST
//! /api/metadata/youtube`), pasted into the editor. Turned into what the editor stages:
//! a version named after where it came from, its title, date, genre and artists, the
//! uploader's account (when the artist is the uploader), the track's source on that
//! platform, and its artwork.
//!
//! Titles are taken as they are ("Artist - Title", "feat." and "prod." in them aren't
//! parsed yet).

use serde_json::{json, Value};

/// What was pasted, if it's something to import from.
#[derive(Debug, Clone, PartialEq)]
pub enum Pasted {
    Json(Value),
    /// A YouTube video's link: its metadata is asked for.
    YouTube(String),
}

/// What a platform says about a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    /// Where it came from, as the version is named ("SoundCloud", "YouTube").
    pub from: &'static str,
    pub title: String,
    pub date: Option<String>,
    pub genre: Option<String>,
    /// The artists' names, primary, in order.
    pub artists: Vec<String>,
    /// The uploader's account, when the (first) artist is the uploader.
    pub account: Option<Account>,
    /// The track's source there: (platform, its id, its data in the platform's shape).
    pub source: (String, String, Value),
    /// The artwork's links, best first.
    pub artwork: Vec<String>,
}

/// An account on a platform, as an artist's source.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub platform: String,
    /// The ids it may be known by (an artist's source's id field): looked up.
    pub ids: Vec<String>,
    /// How it's said ("soundcloud someone").
    pub label: String,
    /// Its data, in the platform's artist shape.
    pub data: Value,
}

/// Whether `text` (pasted) is something to import from: a JSON object, or a YouTube link.
pub fn recognize(text: &str) -> Option<Pasted> {
    let text = text.trim();
    if text.starts_with('{') {
        return serde_json::from_str::<Value>(text).ok().filter(Value::is_object).map(Pasted::Json);
    }
    let lower = text.to_lowercase();
    let starts = ["http://", "https://", "youtu.be/", "youtube.com/", "www.youtube.com/", "m.youtube.com/", "music.youtube.com/"].iter().any(|p| lower.starts_with(p));
    let link = starts && ["youtube.com/", "youtu.be/"].iter().any(|h| lower.contains(h)) && !text.contains(char::is_whitespace);
    link.then(|| Pasted::YouTube(text.to_string()))
}

/// The metadata in a SoundCloud track's JSON or a yt-dlp info JSON.
pub fn parse(json: &Value) -> Result<Metadata, String> {
    let extractor = json.get("extractor_key").and_then(Value::as_str).unwrap_or("");
    if extractor.eq_ignore_ascii_case("youtube") || str_of(json, "webpage_url").is_some_and(|u| u.contains("youtube.com/")) {
        return youtube(json);
    }
    if !extractor.is_empty() {
        return Err(format!("a yt-dlp JSON from {extractor}: only YouTube's is read (for SoundCloud, paste the SoundCloud track's own JSON)"));
    }
    if json.get("waveform_url").is_some() || json.get("waveformKey").is_some() || str_of(json, "permalink_url").is_some_and(|u| u.contains("soundcloud.com/")) {
        return soundcloud(json);
    }
    Err("not a SoundCloud track's JSON or a yt-dlp JSON of a YouTube video".into())
}

fn str_of<'a>(json: &'a Value, key: &str) -> Option<&'a str> {
    json.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A SoundCloud track (the API's shape). The source's fields:
/// * `song_id`: `id`;
/// * `song_uids`: the uploaded file's id, from the waveform (`waveform_url`'s last part
///   before `_`, else `waveformKey`'s); refused without one;
/// * `song_permalinks`: `permalink_url` after "soundcloud.com/";
/// * `release_dates`: `display_date`, else `created_at`; `created_at`;
/// * `song_descriptions`: `description`, if it has one.
fn soundcloud(json: &Value) -> Result<Metadata, String> {
    let id = json.get("id").and_then(Value::as_i64).ok_or("the SoundCloud JSON has no track id (`id`)")?;
    let uid = match (json.get("waveform_url"), json.get("waveformKey")) {
        (Some(url), _) => {
            let url = url.as_str().filter(|u| !u.is_empty()).ok_or("couldn't tell the song's waveform: its waveform_url is null")?;
            url.rsplit('/').next().unwrap_or(url).split('_').next().unwrap_or("").to_string()
        }
        (None, Some(key)) => key.as_str().unwrap_or("").split('_').next().unwrap_or("").to_string(),
        (None, None) => return Err("couldn't tell the song's waveform (no waveform_url or waveformKey)".into()),
    };
    if uid.is_empty() {
        return Err("couldn't tell the song's waveform: it's empty".into());
    }
    let title = str_of(json, "title").ok_or("the SoundCloud JSON has no title")?.to_string();
    let created = str_of(json, "created_at").map(str::to_string);
    let date = str_of(json, "display_date").map(str::to_string).or(created.clone());
    let permalink = str_of(json, "permalink_url").and_then(|u| u.split_once("soundcloud.com/")).map(|(_, p)| p.trim_end_matches('/').to_string()).filter(|p| !p.is_empty());

    let mut data = json!({ "song_id": id, "song_uids": [uid] });
    if let Some(p) = &permalink {
        data["song_permalinks"] = json!([p]);
    }
    if let Some(d) = &date {
        data["release_dates"] = json!([d]);
    }
    if let Some(c) = &created {
        data["created_at"] = json!([c]);
    }
    if let Some(d) = str_of(json, "description") {
        data["song_descriptions"] = json!([d]);
    }

    // The artist: the publisher's (labels, distributors), else the uploader.
    let user = json.get("user").unwrap_or(&Value::Null);
    let uploader = str_of(user, "username").map(str::to_string);
    let publisher = json.get("publisher_metadata").and_then(|p| str_of(p, "artist")).map(str::to_string);
    let artist = publisher.clone().or(uploader.clone()).ok_or("the SoundCloud JSON says no artist (no publisher artist, no uploader)")?;
    let is_uploader = uploader.as_deref().is_some_and(|u| u.eq_ignore_ascii_case(&artist));
    let account = match (is_uploader, user.get("id").and_then(Value::as_i64)) {
        (true, Some(user_id)) => {
            let mut data = json!({ "artist_id": user_id });
            let handle = str_of(user, "permalink");
            if let Some(h) = handle {
                data["artist_permalinks"] = json!([h]);
            }
            Some(Account { platform: "soundcloud".into(), ids: vec![user_id.to_string()], label: format!("soundcloud {}", handle.map(str::to_string).unwrap_or(user_id.to_string())), data })
        }
        _ => None,
    };
    let artwork = str_of(json, "artwork_url").map(artwork_links).unwrap_or_default();
    Ok(Metadata {
        from: "SoundCloud",
        title,
        date,
        genre: str_of(json, "genre").map(str::to_string),
        artists: vec![artist],
        account,
        source: ("soundcloud".into(), id.to_string(), data),
        artwork: dedup(artwork),
    })
}

/// A YouTube video, as yt-dlp reads it. The artist: yt-dlp's music fields (`artists` /
/// `artist`, `creators` / `creator`), else the channel (whose account it is then: matched
/// by its handle or id, stored as its id). The title: `track`, else the video's.
fn youtube(json: &Value) -> Result<Metadata, String> {
    let id = str_of(json, "id").ok_or("the yt-dlp JSON has no video id")?.to_string();
    let title = str_of(json, "track").or(str_of(json, "title")).ok_or("the yt-dlp JSON has no title")?.to_string();
    let list = |key: &str| -> Vec<String> { json.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default() };
    let one = |key: &str| str_of(json, key).map(|s| vec![s.to_string()]).unwrap_or_default();
    let music = [list("artists"), one("artist"), list("creators"), one("creator")].into_iter().find(|l| !l.is_empty());
    let channel = str_of(json, "channel").or(str_of(json, "uploader")).map(str::to_string);
    let (artists, account) = match music {
        Some(names) => (names, None),
        None => {
            let name = channel.ok_or("the yt-dlp JSON says no artist and no channel")?;
            let channel_id = str_of(json, "channel_id").map(str::to_string);
            let handle = str_of(json, "uploader_id").map(|h| h.trim_start_matches('@').to_string()).filter(|h| !h.is_empty());
            let ids: Vec<String> = channel_id.iter().chain(handle.iter()).cloned().collect();
            let account = channel_id.or(handle.clone()).map(|stored| Account {
                platform: "youtube".into(),
                label: format!("youtube {}", handle.as_ref().map(|h| format!("@{h}")).unwrap_or(stored.clone())),
                data: json!({ "channel": stored }),
                ids,
            });
            (vec![name], account)
        }
    };
    let iso = |d: &str| if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) { format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]) } else { d.to_string() };
    let uploaded = str_of(json, "upload_date").map(iso);
    let date = str_of(json, "release_date").map(iso).or(uploaded.clone());
    let mut data = json!({ "song_id": id });
    if let Some(d) = str_of(json, "description") {
        data["song_description"] = json!(d);
    }
    if let Some(d) = &uploaded {
        data["song_release_date"] = json!(d);
    }
    Ok(Metadata {
        from: "YouTube",
        title,
        date,
        genre: None,
        artists,
        account,
        source: ("youtube".into(), id, data),
        artwork: str_of(json, "thumbnail").map(|t| vec![t.to_string()]).unwrap_or_default(),
    })
}

/// A SoundCloud artwork's links, best first: its original (a JPEG, else a PNG: it's kept
/// as uploaded), whatever size `url` is ("…-large.jpg", "…-t500x500.jpg"), then `url`.
fn artwork_links(url: &str) -> Vec<String> {
    let (path, query) = url.split_once('?').map_or((url, ""), |(p, q)| (p, q));
    let mut links = Vec::new();
    // "…/artworks-abc-def-t500x500.jpg": the size is after the last "-" of the file's name.
    let name_at = path.rfind('/').map_or(0, |i| i + 1);
    if let Some(dash) = path[name_at..].rfind('-') {
        let base = &path[..name_at + dash];
        links.push(format!("{base}-original.jpg"));
        links.push(format!("{base}-original.png"));
    }
    links.push(if query.is_empty() { path.to_string() } else { format!("{path}?{query}") });
    dedup(links)
}

fn dedup(mut list: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    list.retain(|x| {
        let new = !seen.contains(x);
        seen.push(x.clone());
        new
    });
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc() -> Value {
        json!({
            "kind": "track", "id": 5550123, "title": "Glass Harbor (feat. Bo)",
            "permalink_url": "https://soundcloud.com/miravale/glass-harbor",
            "waveform_url": "https://wave.sndcdn.com/AbCdEf123_m.json",
            "created_at": "2021-06-04T11:11:11Z", "display_date": "2021-06-05T00:00:00Z",
            "description": "out now", "genre": "Electronic",
            "artwork_url": "https://i1.sndcdn.com/artworks-xyz-large.jpg",
            "user": { "id": 42, "username": "Mira Vale", "permalink": "miravale" },
            "publisher_metadata": { "artist": "" }
        })
    }

    #[test]
    fn a_soundcloud_track() {
        let m = parse(&sc()).unwrap();
        assert_eq!((m.from, m.title.as_str(), m.date.as_deref(), m.genre.as_deref()), ("SoundCloud", "Glass Harbor (feat. Bo)", Some("2021-06-05T00:00:00Z"), Some("Electronic")));
        assert_eq!(m.artists, ["Mira Vale"]);
        assert_eq!(
            m.source,
            (
                "soundcloud".to_string(),
                "5550123".to_string(),
                json!({ "song_id": 5550123, "song_uids": ["AbCdEf123"], "song_permalinks": ["miravale/glass-harbor"],
                        "release_dates": ["2021-06-05T00:00:00Z"], "created_at": ["2021-06-04T11:11:11Z"], "song_descriptions": ["out now"] })
            )
        );
        let account = m.account.unwrap();
        assert_eq!((account.ids, account.data), (vec!["42".to_string()], json!({ "artist_id": 42, "artist_permalinks": ["miravale"] })));
        assert_eq!(m.artwork, ["https://i1.sndcdn.com/artworks-xyz-original.jpg", "https://i1.sndcdn.com/artworks-xyz-original.png", "https://i1.sndcdn.com/artworks-xyz-large.jpg"]);
        let links = artwork_links("https://i1.sndcdn.com/artworks-000123-AbC-t500x500.png?x=1");
        assert_eq!(links, ["https://i1.sndcdn.com/artworks-000123-AbC-original.jpg", "https://i1.sndcdn.com/artworks-000123-AbC-original.png", "https://i1.sndcdn.com/artworks-000123-AbC-t500x500.png?x=1"]);
    }

    #[test]
    fn soundcloud_details() {
        // The publisher's artist, not the uploader (a label): no account.
        let mut label = sc();
        label["publisher_metadata"]["artist"] = json!("Ann");
        let m = parse(&label).unwrap();
        assert_eq!((m.artists, m.account), (vec!["Ann".to_string()], None));
        // The waveform from `waveformKey`; no display date: created_at.
        let mut keyed = sc();
        let map = keyed.as_object_mut().unwrap();
        map.remove("waveform_url");
        map.remove("display_date");
        map.insert("waveformKey".into(), json!("XyZ_m"));
        let m = parse(&keyed).unwrap();
        assert_eq!((m.source.2["song_uids"].clone(), m.date.as_deref()), (json!(["XyZ"]), Some("2021-06-04T11:11:11Z")));
        // No waveform: refused, as the author's script does.
        let mut null = sc();
        null["waveform_url"] = Value::Null;
        assert!(parse(&null).unwrap_err().contains("waveform_url is null"));
        keyed.as_object_mut().unwrap().remove("waveformKey");
        assert!(parse(&keyed).is_err());
    }

    #[test]
    fn a_youtube_video() {
        let video = json!({
            "id": "dQw4w9WgXcQ", "title": "Mira Vale - Glass Harbor (Official Video)", "extractor_key": "Youtube",
            "channel": "Mira Vale", "channel_id": "UCabc", "uploader_id": "@miravale",
            "upload_date": "20210604", "description": "", "thumbnail": "https://i.ytimg.com/vi/x/maxresdefault.jpg"
        });
        let m = parse(&video).unwrap();
        assert_eq!((m.from, m.title.as_str(), m.date.as_deref()), ("YouTube", "Mira Vale - Glass Harbor (Official Video)", Some("2021-06-04")));
        assert_eq!(m.artists, ["Mira Vale"]);
        assert_eq!(m.source.2, json!({ "song_id": "dQw4w9WgXcQ", "song_release_date": "2021-06-04" }));
        let account = m.account.unwrap();
        assert_eq!((account.ids, account.data, account.label.as_str()), (vec!["UCabc".to_string(), "miravale".to_string()], json!({ "channel": "UCabc" }), "youtube @miravale"));

        // YouTube Music's fields: those artists, no account; its track name.
        let mut music = video.clone();
        music["artists"] = json!(["Mira Vale", "Bo"]);
        music["track"] = json!("Glass Harbor");
        music["release_date"] = json!("20210101");
        let m = parse(&music).unwrap();
        assert_eq!((m.title.as_str(), m.artists.len(), m.account.is_none(), m.date.as_deref()), ("Glass Harbor", 2, true, Some("2021-01-01")));
    }

    #[test]
    fn what_is_pasted() {
        assert_eq!(recognize("  https://youtu.be/dQw4w9WgXcQ "), Some(Pasted::YouTube("https://youtu.be/dQw4w9WgXcQ".into())));
        assert!(matches!(recognize("{\"id\": 1}"), Some(Pasted::Json(_))));
        assert_eq!(recognize("{not json"), None);
        assert_eq!(recognize("artist:Ann"), None);
        assert_eq!(recognize("source:https://youtu.be/x"), None);
        assert_eq!(recognize("https://soundcloud.com/a/b"), None);
        let soundcloud_by_ytdlp = json!({ "id": "123", "extractor_key": "Soundcloud", "title": "x" });
        assert!(parse(&soundcloud_by_ytdlp).unwrap_err().contains("only YouTube"));
    }
}
