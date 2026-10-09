//! The track page's own keys, in the editor's line (`play:`, `star:`, `lyrics`…): their names
//! and what they take. What they do is the page's.

use super::parse::K;

/// The page's keys, in the editor's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PageKey {
    Play,
    Queue,
    Star,
    Rating,
    Reveal(Reveal),
    /// The lyrics: opened, or (a value) which entry is shown.
    Lyrics,
    /// The file: opened; `file:#2` an earlier file played again; after `extra:`, a path.
    File,
    Note,
    Extra,
    Description,
    Visibility,
    Og,
    Version,
    Unmerge,
    Restore,
    /// Edit mode: the lyrics' timing popup.
    Sync,
}

/// The parts of the page the commandline opens (or closes) and scrolls to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reveal {
    Lyrics,
    EditLyrics,
    Notes,
    Extras,
    Credits,
    Related,
    File,
    Versions,
    Sources,
    Merges,
    Covers,
    Tags,
    History,
}

impl Reveal {
    /// Its section's title (without the count).
    pub fn title(self) -> &'static str {
        match self {
            Reveal::Lyrics => "Lyrics",
            Reveal::EditLyrics => "Edit lyrics",
            Reveal::Notes => "Notes",
            Reveal::Extras => "Extras",
            Reveal::Credits => "Credits",
            Reveal::Related => "Related tracks",
            Reveal::File => "File",
            Reveal::Versions => "Versions",
            Reveal::Sources => "Sources",
            Reveal::Merges => "Merged into this track",
            Reveal::Covers => "Covers",
            Reveal::Tags => "Tags in the file",
            Reveal::History => "History",
        }
    }
}

/// Every page key's names: its own first. Exact names only. (`covers`, `sources` and
/// `related` are the editor's keys: words find those sections.)
const KEYS: &[(PageKey, &[&str])] = &[
    (PageKey::Play, &["play"]),
    (PageKey::Queue, &["queue", "enqueue"]),
    (PageKey::Star, &["star", "starred"]),
    (PageKey::Rating, &["rating", "rate", "stars"]),
    (PageKey::Reveal(Reveal::Credits), &["credits"]),
    (PageKey::Reveal(Reveal::Notes), &["notes"]),
    (PageKey::Reveal(Reveal::Extras), &["extras"]),
    (PageKey::Reveal(Reveal::Versions), &["versions"]),
    (PageKey::Reveal(Reveal::Merges), &["merges", "merged"]),
    (PageKey::Reveal(Reveal::Tags), &["tags"]),
    (PageKey::Reveal(Reveal::History), &["history", "revisions"]),
    (PageKey::Lyrics, &["lyrics"]),
    (PageKey::File, &["file", "files"]),
    (PageKey::Note, &["note"]),
    (PageKey::Extra, &["extra"]),
    (PageKey::Description, &["description", "desc"]),
    (PageKey::Visibility, &["visibility", "vis", "seen"]),
    (PageKey::Og, &["og", "original"]),
    (PageKey::Version, &["version"]),
    (PageKey::Unmerge, &["unmerge"]),
    (PageKey::Restore, &["restore"]),
    (PageKey::Sync, &["sync"]),
];

impl PageKey {
    /// The key a typed name means (exactly, any case).
    pub fn of(name: &str) -> Option<PageKey> {
        let name = name.to_lowercase();
        KEYS.iter().find(|(_, names)| names.contains(&name.as_str())).map(|(k, _)| *k)
    }

    pub fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }

    /// `-key` alone means something.
    pub fn is_list(self) -> bool {
        matches!(self, PageKey::Star | PageKey::Rating | PageKey::Og)
    }

    pub fn takes_hash(self) -> bool {
        matches!(self, PageKey::Lyrics | PageKey::File | PageKey::Restore)
    }

    /// The arguments it takes (in the editor's keys).
    pub fn takes(self, arg: K) -> bool {
        match self {
            PageKey::Extra => matches!(arg, K::Kind | K::Source | K::Page(PageKey::File | PageKey::Description | PageKey::Visibility)),
            PageKey::Version => arg == K::As,
            _ => false,
        }
    }
}
