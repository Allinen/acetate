//! YouTube video ids, for a file that is a video (an extra with `Title [id].mp4`, see
//! `api::extras::to_track`). What YouTube says of a video is read by the YouTube
//! extension's program (`extensions/youtube/metadata.py`).

use super::layouts::ytdlp::bracket_id;

/// Whether this is a YouTube video id: any 11 of `A-Z a-z 0-9 - _` (so "helloworld1" is
/// one, on purpose), as the YouTube extension says.
pub fn is_id(id: &str) -> bool {
    crate::extensions::registry().id_rule("youtube").is_some_and(|rule| rule.is_id(id))
}

/// The video id at the end of a file's name (`Title [id].mp4`, or the name without its
/// extension), if there is one.
pub fn id_in_name(name: &str) -> Option<String> {
    let name = name.trim();
    let stem = name.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4 && ext.bytes().all(|b| b.is_ascii_alphanumeric())).map_or(name, |(stem, _)| stem);
    bracket_id(stem).filter(|id| is_id(id)).map(str::to_string)
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
}
