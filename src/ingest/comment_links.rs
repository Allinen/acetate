//! Where a file came from, by a link in its comment: download tools often put the page
//! they got it from there ("downloaded from lucida.to: https://soundcloud.com/a/b").
//! Used by `acetate ingest` for files whose path says nothing (see `origin_from_comment`).
//!
//! Which links count, and what source each is, is for the extensions to say (`[[link]]`
//! with `comment = true`): a SoundCloud track link is the SoundCloud song, by permalink
//! ("a/b"), the same source as SoundCloud downloads, found by their permalinks; Qobuz,
//! Amazon Music, Tidal and Deezer links are custom sources labelled with the store's name,
//! with the link as `url`.
//!
//! Only one link counts: the first one that is a source. Anything else (a SoundCloud
//! playlist, another site) is no link.

use crate::model::sources::{self, Kind, NewSource, ValidSource};

/// The source a link in `comment` names, if any (see the module docs).
pub fn origin_from_comment(comment: &str) -> Option<ValidSource> {
    comment.split_whitespace().find_map(origin_from_link)
}

/// The source one link names.
fn origin_from_link(word: &str) -> Option<ValidSource> {
    // Links in prose often end in punctuation: "(https://…)." or "https://…,".
    let link = word.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | ',' | '.' | ';' | '!'));
    let registry = crate::extensions::registry();
    let found = registry.match_link(link, "track", true)?;
    let data = registry.data_from_fields(&found.platform, "track", &found.fields).ok()?;
    let new = NewSource { platform: found.platform, source_id: None, captured_at: None, data };
    sources::validate(Kind::Track, new).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
