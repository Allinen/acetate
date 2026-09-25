//! yt-dlp's default filenames: `Title [id].ext`, e.g. `VANTA - SELAH [KfP2xq7RtLm].m4a`
//! (YouTube) or `SOLSTICE (FT. VANTA) [1000001406].m4a` (SoundCloud).
//!
//! Which platform the files are from is given (`--platform`), never guessed from what the
//! id looks like. The id is the last `[...]` at the very end of the name, and it has to
//! look like that platform's ids, so `rust [prod. dale the duke]` gives no id.

use std::path::Path;

use serde_json::json;

use super::{source, PathInfo};
use crate::model::sources::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Soundcloud,
    Youtube,
}

impl Platform {
    pub fn name(self) -> &'static str {
        match self {
            Platform::Soundcloud => "soundcloud",
            Platform::Youtube => "youtube",
        }
    }
}

pub fn read(platform: Platform, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let id = bracket_id(&stem);
    let track = match (platform, id) {
        // SoundCloud track ids are numbers.
        (Platform::Soundcloud, Some(id)) => id
            .parse::<i64>()
            .ok()
            .filter(|_| id.bytes().all(|b| b.is_ascii_digit()))
            .map(|n| source("soundcloud", Kind::Track, "song_id", json!(n))),
        // YouTube video ids are 11 characters of letters, digits, `-` and `_`.
        (Platform::Youtube, Some(id)) if is_youtube_id(id) => {
            Some(source("youtube", Kind::Track, "song_id", json!(id)))
        }
        _ => None,
    };
    if track.is_none() {
        info.warnings.push(format!(
            "no {} id at the end of the filename (\"… [<id>]\")",
            platform.name()
        ));
    }
    info.track = track;
    info
}

/// The text in the last `[...]`, if the name ends with it.
pub fn bracket_id(stem: &str) -> Option<&str> {
    let inner = stem.strip_suffix(']')?;
    let start = inner.rfind('[')?;
    let id = &inner[start + 1..];
    (!id.is_empty()).then_some(id)
}

fn is_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(platform: Platform, name: &str) -> Option<String> {
        read(platform, Path::new(name)).track.and_then(|s| s.external_id)
    }

    #[test]
    fn soundcloud_ids() {
        let sc = Platform::Soundcloud;
        assert_eq!(id(sc, "SOLSTICE (FT. VANTA) [1000001406].m4a").as_deref(), Some("1000001406"));
        // The last bracket, not the year before it.
        assert_eq!(id(sc, "VANTA & KR8 - CROWN ME [2019] [1000001443].m4a").as_deref(), Some("1000001443"));
        assert_eq!(id(sc, "SHAME+ PART II {W⧸VANTA}PROD.SENTRY [100001369].opus").as_deref(), Some("100001369"));
        assert_eq!(id(sc, "KR8 - Run Fast W_ VANTA [Prod. SOMNI].m4a"), None);
        assert_eq!(id(sc, "Vellum - ZEPHYR FT. VANTA (FREE DOWNLOAD).m4a"), None);
    }

    #[test]
    fn youtube_ids() {
        let yt = Platform::Youtube;
        assert_eq!(id(yt, "VANTA - SELAH [KfP2xq7RtLm].m4a").as_deref(), Some("KfP2xq7RtLm"));
        assert_eq!(id(yt, "VANTA - TEST ME [Z_QbNw4RsUo].m4a").as_deref(), Some("Z_QbNw4RsUo"));
        assert_eq!(
            id(yt, "steam engine freestyle [ prod. @kemoli ] [VeRpk8ssLqB].m4a").as_deref(),
            Some("VeRpk8ssLqB")
        );
        assert_eq!(id(yt, "VANTA - rust [prod. dale the duke].m4a"), None);
        assert_eq!(id(yt, "CHOP SUEY.m4a"), None);
        // A SoundCloud-looking number isn't a YouTube id.
        assert_eq!(id(yt, "Unhinged [100001480].opus"), None);
    }
}
