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

/// The platform the files are from: an extension that says its ids end yt-dlp's file names
/// (`[id]` with `filename = true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform(String);

impl Platform {
    /// The platform called `name`, if an extension reads its file names.
    pub fn named(name: &str) -> Result<Platform, String> {
        let registry = crate::extensions::registry();
        if registry.filename_ids().any(|(platform, _)| platform == name) {
            return Ok(Platform(name.to_string()));
        }
        let known: Vec<&str> = registry.filename_ids().map(|(platform, _)| platform).collect();
        Err(format!("no extension reads {name}'s file names (it would be one of: {})", known.join(", ")))
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

pub fn read(platform: &Platform, path: &Path) -> PathInfo {
    let mut info = PathInfo::default();
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let registry = crate::extensions::registry();
    let track = bracket_id(&stem).and_then(|id| {
        let rule = registry.id_rule(platform.name()).filter(|r| r.filename && r.is_id(id))?;
        let field = registry.shape(platform.name(), "track")?.fields.iter().find(|f| f.name == rule.field)?;
        let value = match field.kind {
            acetate_extensions::FieldType::Int => json!(id.parse::<i64>().ok()?),
            _ => json!(id),
        };
        Some(source(platform.name(), Kind::Track, &rule.field, value))
    });
    if track.is_none() {
        info.warnings.push(format!("no {} id at the end of the filename (\"… [<id>]\")", platform.name()));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn id(platform: Platform, name: &str) -> Option<String> {
        read(&platform, Path::new(name)).track.and_then(|s| s.external_id)
    }

    #[test]
    fn soundcloud_ids() {
        let sc = Platform::named("soundcloud").unwrap();
        assert_eq!(id(sc.clone(), "SOLSTICE (FT. VANTA) [1000001406].m4a").as_deref(), Some("1000001406"));
        // The last bracket, not the year before it.
        assert_eq!(id(sc.clone(), "VANTA & KR8 - CROWN ME [2019] [1000001443].m4a").as_deref(), Some("1000001443"));
        assert_eq!(id(sc.clone(), "SHAME+ PART II {W⧸VANTA}PROD.SENTRY [100001369].opus").as_deref(), Some("100001369"));
        assert_eq!(id(sc.clone(), "KR8 - Run Fast W_ VANTA [Prod. SOMNI].m4a"), None);
        assert_eq!(id(sc.clone(), "Vellum - ZEPHYR FT. VANTA (FREE DOWNLOAD).m4a"), None);
    }

    #[test]
    fn youtube_ids() {
        let yt = Platform::named("youtube").unwrap();
        assert_eq!(id(yt.clone(), "VANTA - SELAH [KfP2xq7RtLm].m4a").as_deref(), Some("KfP2xq7RtLm"));
        assert_eq!(id(yt.clone(), "VANTA - TEST ME [Z_QbNw4RsUo].m4a").as_deref(), Some("Z_QbNw4RsUo"));
        assert_eq!(
            id(yt.clone(), "steam engine freestyle [ prod. @kemoli ] [VeRpk8ssLqB].m4a").as_deref(),
            Some("VeRpk8ssLqB")
        );
        assert_eq!(id(yt.clone(), "VANTA - rust [prod. dale the duke].m4a"), None);
        assert_eq!(id(yt.clone(), "CHOP SUEY.m4a"), None);
        // A SoundCloud-looking number isn't a YouTube id.
        assert_eq!(id(yt.clone(), "Unhinged [100001480].opus"), None);
    }
}
