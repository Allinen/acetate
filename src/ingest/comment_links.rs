//! Where a file came from, by a link in its comment: download tools often put the page
//! they got it from there ("downloaded from lucida.to: https://soundcloud.com/a/b").
//! Used by `acetate ingest` for files whose path says nothing (see `origin_from_comment`).
//!
//! * A SoundCloud track link is the SoundCloud song, by permalink ("a/b"): the same source
//!   as SoundCloud downloads, found by their permalinks.
//! * Qobuz, Amazon Music, Tidal and Deezer links are custom sources labelled with the
//!   store's name, with the link as `url` (Acetate has no data shape of their own for
//!   them).
//!
//! Only one link counts: the first one of these in the comment. Anything else (a
//! SoundCloud playlist, another site) is no link.

use serde_json::json;

use crate::model::sources::{self, Kind, NewSource, ValidSource};

/// The source a link in `comment` names, if any (see the module docs).
pub fn origin_from_comment(comment: &str) -> Option<ValidSource> {
    comment.split_whitespace().find_map(origin_from_link)
}

/// The source one link names.
fn origin_from_link(word: &str) -> Option<ValidSource> {
    // Links in prose often end in punctuation: "(https://…)." or "https://…,".
    let link = word.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | ',' | '.' | ';' | '!'));
    let rest = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"))
        .unwrap_or(link);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');

    if host == "soundcloud.com" || host == "m.soundcloud.com" {
        // "user/track": exactly two parts, and not a playlist, likes, a user's tab...
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let reserved = ["sets", "likes", "reposts", "tracks", "albums", "popular-tracks", "followers", "following"];
        return match parts[..] {
            [user, track] if !reserved.contains(&track) && !path.is_empty() => {
                let permalink = format!("{user}/{track}");
                source("soundcloud", json!({ "song_permalinks": [permalink] }))
            }
            _ => None,
        };
    }
    let label = if host.ends_with("qobuz.com") {
        "qobuz"
    } else if host.starts_with("music.amazon.") || (host.starts_with("amazon.") && path.contains("music")) {
        "amazon"
    } else if host.ends_with("tidal.com") {
        "tidal"
    } else if host.ends_with("deezer.com") || host == "deezer.page.link" {
        "deezer"
    } else {
        return None;
    };
    let url = if link.contains("://") { link.to_string() } else { format!("https://{link}") };
    source("custom", json!({ "label": label, "url": url }))
}

fn source(platform: &str, data: serde_json::Value) -> Option<ValidSource> {
    let new = NewSource { platform: platform.to_string(), source_id: None, captured_at: None, data };
    sources::validate(Kind::Track, new).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(comment: &str) -> Option<(String, serde_json::Value)> {
        origin_from_comment(comment).map(|s| (s.platform, s.data))
    }

    #[test]
    fn soundcloud_tracks_by_permalink() {
        assert_eq!(
            origin("yea haha || downloaded from lucida.to: https://soundcloud.com/1kori/why-kay-feat-rxnin-prod-bowsy"),
            Some(("soundcloud".into(), json!({ "song_permalinks": ["1kori/why-kay-feat-rxnin-prod-bowsy"] })))
        );
        assert_eq!(
            origin("horny nigga laundry basket\n\nhttps://soundcloud.com/1kori/our-fuck-budd-song-what?utm_source=x"),
            Some(("soundcloud".into(), json!({ "song_permalinks": ["1kori/our-fuck-budd-song-what"] })))
        );
        assert_eq!(origin("(https://m.soundcloud.com/a/b)."), Some(("soundcloud".into(), json!({ "song_permalinks": ["a/b"] }))));
        // Not a track.
        assert_eq!(origin("https://soundcloud.com/1kori/sets/some-tape"), None);
        assert_eq!(origin("https://soundcloud.com/1kori"), None);
        assert_eq!(origin("https://soundcloud.com/1kori/likes"), None);
    }

    #[test]
    fn stores_are_custom_with_the_link() {
        let custom = |label: &str, url: &str| Some(("custom".to_string(), json!({ "label": label, "url": url })));
        assert_eq!(origin("https://open.qobuz.com/track/123456"), custom("qobuz", "https://open.qobuz.com/track/123456"));
        assert_eq!(origin("from https://tidal.com/browse/track/42."), custom("tidal", "https://tidal.com/browse/track/42"));
        assert_eq!(origin("www.deezer.com/en/track/7"), custom("deezer", "https://www.deezer.com/en/track/7"));
        assert_eq!(
            origin("https://music.amazon.com/albums/B0X?trackAsin=B0Y"),
            custom("amazon", "https://music.amazon.com/albums/B0X?trackAsin=B0Y")
        );
        // The first link counts; other sites aren't sources.
        assert_eq!(origin("https://youtube.com/watch?v=x https://tidal.com/track/1"), custom("tidal", "https://tidal.com/track/1"));
        assert_eq!(origin("prod by someone"), None);
        assert_eq!(origin("https://example.com/soundcloud.com/a/b"), None);
    }
}
