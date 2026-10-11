//! "Import metadata from": a platform's track JSON (a SoundCloud track's, a yt-dlp info
//! JSON) or a YouTube link, pasted into the editor. The server reads it with the platform's
//! extension (`POST /api/extensions/import`, `POST /api/extensions/{name}/metadata`) and
//! answers with the track's `Metadata`: a version named after where it came from, its
//! title, date, genre and artists, the uploader's account (when the artist is the
//! uploader), the track's source on that platform, and its artwork.
//!
//! Titles are taken as they are ("Artist - Title", "feat." and "prod." in them aren't
//! parsed: `parse:` in the editor does that).

use serde_json::Value;

pub use acetate_extensions::{Account, Metadata, SourceRef};

/// What was pasted, if it's something to import from.
#[derive(Debug, Clone, PartialEq)]
pub enum Pasted {
    Json(Value),
    /// A YouTube video's link: its metadata is asked for.
    YouTube(String),
    /// Text that starts like a JSON object but isn't one: why.
    Broken(String),
}

/// Whether `text` (pasted) is something to import from: a JSON object, or a YouTube link.
/// Line breaks are left out of JSON first: copied from a terminal, its line is cut where
/// the terminal wrapped it, words included (a JSON string can't have one, and JSON needs
/// no whitespace between its parts, so taking them out never changes what it says).
pub fn recognize(text: &str) -> Option<Pasted> {
    let text = text.trim();
    if text.starts_with('{') {
        let joined: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        return Some(match serde_json::from_str::<Value>(&joined) {
            Ok(json) if json.is_object() => Pasted::Json(json),
            Ok(_) => Pasted::Broken("it's JSON, but not an object".into()),
            Err(err) => Pasted::Broken(format!(
                "it isn't valid JSON ({err}; around “{}”). Copied from a terminal? Its wrapping can put spaces into words",
                joined.chars().skip(err.column().saturating_sub(15)).take(30).collect::<String>()
            )),
        });
    }
    // A YouTube video's link, as the YouTube extension reads links.
    let link = !text.contains(char::is_whitespace) && crate::extensions::current().match_link(text, "track", false).is_some_and(|m| m.platform == "youtube");
    link.then(|| Pasted::YouTube(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_is_pasted() {
        assert_eq!(recognize("  https://youtu.be/dQw4w9WgXcQ "), Some(Pasted::YouTube("https://youtu.be/dQw4w9WgXcQ".into())));
        assert!(matches!(recognize("{\"id\": 1}"), Some(Pasted::Json(_))));
        assert!(matches!(recognize("{not json"), Some(Pasted::Broken(_))));
        // Wrapped by a terminal (here every 20 characters): one line again.
        let wrapped = "{\"id\": 941058670, \"t\nitle\": \"Anime\", \"x\": nu\r\nll}";
        assert_eq!(recognize(wrapped), Some(Pasted::Json(json!({ "id": 941058670, "title": "Anime", "x": null }))));
        assert_eq!(recognize("artist:Ann"), None);
        assert_eq!(recognize("source:https://youtu.be/x"), None);
        assert_eq!(recognize("https://soundcloud.com/a/b"), None);
    }
}
