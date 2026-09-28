//! What's wrong with a track, from the catalog kept here (so it's instant, for the whole
//! library): no title or one that looks like a file name, no artists, not on an album, no
//! cover, no release date. The workbench (the Review page) lists tracks by these and says
//! them on each; which ones count is the user's setting.

use super::model::Track;

/// A problem a track can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Problem {
    NoTitle,
    /// The title looks like a file name: an extension, underscores for spaces, a track
    /// number in front, the artist in front ("Artist - Title"), a long number from a
    /// download site, or the file's own name.
    FileNameTitle,
    NoArtists,
    NoAlbum,
    NoCover,
    NoDate,
}

impl Problem {
    pub const ALL: [Problem; 6] =
        [Problem::NoTitle, Problem::FileNameTitle, Problem::NoArtists, Problem::NoAlbum, Problem::NoCover, Problem::NoDate];

    /// As the page says it ("Missing: title, artists").
    pub fn name(self) -> &'static str {
        match self {
            Problem::NoTitle => "title",
            Problem::FileNameTitle => "a real title (it looks like a file name)",
            Problem::NoArtists => "artists",
            Problem::NoAlbum => "album",
            Problem::NoCover => "cover",
            Problem::NoDate => "release date",
        }
    }

    /// Short, for filter chips.
    pub fn label(self) -> &'static str {
        match self {
            Problem::NoTitle => "No title",
            Problem::FileNameTitle => "File-name title",
            Problem::NoArtists => "No artists",
            Problem::NoAlbum => "No album",
            Problem::NoCover => "No cover",
            Problem::NoDate => "No date",
        }
    }
}

/// The problems of a track, in `Problem::ALL`'s order. `file_name`: the file's name, if
/// known (a title equal to it came from it).
pub fn of(track: &Track, file_name: Option<&str>) -> Vec<Problem> {
    let mut problems = Vec::new();
    let title = track.title.trim();
    if title.is_empty() {
        problems.push(Problem::NoTitle);
    } else if looks_like_file_name(title, &track.artist_display, file_name) {
        problems.push(Problem::FileNameTitle);
    }
    if track.credits.is_empty() {
        problems.push(Problem::NoArtists);
    }
    if track.albums.is_empty() {
        problems.push(Problem::NoAlbum);
    }
    if track.cover_id.is_none() {
        problems.push(Problem::NoCover);
    }
    if track.release_date.as_deref().is_none_or(|d| d.trim().is_empty()) {
        problems.push(Problem::NoDate);
    }
    problems
}

const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus", "aiff", "aif", "wma", "alac"];

/// Whether `title` looks like it was taken from a file name rather than written.
pub fn looks_like_file_name(title: &str, artist: &str, file_name: Option<&str>) -> bool {
    let lower = title.to_lowercase();
    // "Song.mp3"
    if let Some((_, extension)) = lower.rsplit_once('.')
        && AUDIO_EXTENSIONS.contains(&extension)
    {
        return true;
    }
    // "my_song_final"
    if title.contains('_') && !title.contains(' ') {
        return true;
    }
    // "01 Song", "01. Song", "1-02 - Song" (two digits, or disc-track, then a separator).
    let bytes = title.as_bytes();
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit() || **b == b'-').count();
    if digits >= 2 && bytes.get(digits).is_some_and(|b| matches!(b, b' ' | b'.' | b'_')) && bytes[..digits].iter().filter(|b| b.is_ascii_digit()).count() >= 2 {
        return true;
    }
    // "Artist - Song"
    let artist = artist.trim().to_lowercase();
    if !artist.is_empty() && lower.starts_with(&format!("{artist} - ")) {
        return true;
    }
    // "Song - 1000001110 - 4TrPq527LMzx" (a download site's ids).
    if title.split(" - ").any(|part| part.len() >= 7 && part.bytes().all(|b| b.is_ascii_digit())) {
        return true;
    }
    // The file's own name, without its extension.
    if let Some(name) = file_name {
        let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
        if stem.trim().eq_ignore_ascii_case(title) {
            return true;
        }
    }
    false
}

/// The file name in a path ("/music/a/Song.mp3" → "Song.mp3").
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The folder of a path ("/music/a/Song.mp3" → "/music/a").
pub fn folder(path: &str) -> &str {
    path.rfind(['/', '\\']).map_or("", |i| &path[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_titles() {
        let yes = |title: &str| assert!(looks_like_file_name(title, "OKTVA", None), "{title}");
        let no = |title: &str| assert!(!looks_like_file_name(title, "OKTVA", None), "{title}");
        yes("REMEDY.mp3");
        yes("my_song_final");
        yes("01 Intro");
        yes("01. Intro");
        yes("1-02 Intro");
        yes("OKTVA - REMEDY");
        yes("Late night tape - 1000001110 - 4TrPq527LMzx");
        no("REMEDY");
        no("2 Chainz type beat");
        no("1999");
        no("Mr. Brightside");
        no("Song - Remix");
        no("Don't_Stop Me Now");
        assert!(looks_like_file_name("Explosive", "OKTVA", Some("Explosive.mp3")));
        assert!(!looks_like_file_name("Explosive explosion", "OKTVA", Some("Explosive.mp3")));
    }

    #[test]
    fn paths() {
        assert_eq!(file_name("/music/a/Song.mp3"), "Song.mp3");
        assert_eq!(folder("/music/a/Song.mp3"), "/music/a");
        assert_eq!(folder("Song.mp3"), "");
    }
}
