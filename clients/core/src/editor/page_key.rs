//! The track page's own keys, in the editor's line (`play:`, `star:`, `lyrics`…): their names
//! and what they take. What they do is the page's.

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
    /// The file: opened; `file:2` an earlier file played again.
    File,
    Note,
    Extra,
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

use crate::commandline::schema::{Kind, Minus, Next, Spec, Word};

/// `extra:`'s arguments (on any page that has extras).
pub const EXTRA_ARGS: &[Spec] = &[
    Spec::new("kind", Kind::Text, "vocals, stems, sample… (any word)").aliases(&["type"]),
    Spec::new("file", Kind::Path, "a path (Tab completes it), paste, or choose").aliases(&["files"]),
    Spec::new("description", Kind::Text, "what it is").aliases(&["desc"]),
    Spec::new("source", Kind::Text, "where it came from"),
    Spec::new("visibility", Kind::Text, "who sees it: everyone, or groups (Group A, Group B)").aliases(&["vis", "seen"]),
];

/// `source:`, as an artist's or album's command: any `name:` after a platform is one of its
/// fields. (The track editor has its own, with `=`.)
pub const SOURCE: Spec = Spec::new("source", Kind::Text, "a link, a platform and its fields, or custom (-source:N: to the trash)").minus(Minus::Off).next(Next::Fields(&[]));
/// `extra:Name` and its arguments; `-extra:Name`.
pub const EXTRA: Spec = Spec::new("extra", Kind::Text, "a new one, one of its own to change, or one in the library").minus(Minus::Off).next(Next::Args(EXTRA_ARGS));
/// `note:Title`, `note:paste`; `-note:Title`.
pub const NOTE: Spec = Spec::new("note", Kind::Text, "a new one, one of its notes to edit, or paste").minus(Minus::Off);

const QUEUE: &[Word] = &[Word { word: "end", help: "at the end of the queue" }, Word { word: "next", help: "before what's queued" }];
const STAR: &[Word] = &[Word { word: "yes", help: "starred" }, Word { word: "no", help: "not starred" }];

/// The track page's commands, in the editor's line after the editor's own.
pub const PAGE: &[Spec] = &[
    Spec::new("play", Kind::Nothing, "it, then the rest of its album"),
    Spec::new("queue", Kind::Maybe(&Kind::Words(QUEUE)), "at the end, or next").aliases(&["enqueue"]),
    Spec::new("star", Kind::Maybe(&Kind::Words(STAR)), "yours (alone: the other way; -star: not starred)").aliases(&["starred"]).minus(Minus::OffOrClear),
    Spec::new("rating", Kind::Text, "yours: 0 to 5 stars, 3.5 too (-rating clears it)").aliases(&["rate", "stars"]).minus(Minus::OffOrClear),
    Spec::new("credits", Kind::Nothing, "opened and scrolled to"),
    Spec::new("notes", Kind::Nothing, "opened and scrolled to"),
    Spec::new("extras", Kind::Nothing, "opened and scrolled to"),
    Spec::new("versions", Kind::Nothing, "opened and scrolled to"),
    Spec::new("merges", Kind::Nothing, "opened and scrolled to").aliases(&["merged"]),
    Spec::new("tags", Kind::Nothing, "opened and scrolled to"),
    Spec::new("history", Kind::Nothing, "opened and scrolled to").aliases(&["revisions"]),
    Spec::new("lyrics", Kind::Maybe(&Kind::Text), "opened; an entry's number or language shows it"),
    Spec::new("file", Kind::Maybe(&Kind::Number), "opened; an earlier file's number plays it again (edit mode)").aliases(&["files"]),
    NOTE,
    EXTRA,
    Spec::new("og", Kind::Maybe(&Kind::Text), "its file is an original file (named so); -og: it isn't").aliases(&["original"]).minus(Minus::OffOrClear),
    Spec::new("version", Kind::Text, "edit another version, or a new one by that name (as: renames it)").next(Next::Args(&[Spec::new("as", Kind::Text, "its new name")])),
    Spec::new("unmerge", Kind::Nothing, "the newest merge into it undone"),
    Spec::new("restore", Kind::Number, "one of its revisions restored (the list has them, newest first)"),
    Spec::new("sync", Kind::Nothing, "time the lyrics (the sync popup)"),
];

impl PageKey {
    /// The page key a command's key is (`spec.key`), if it's one.
    pub fn of(key: &str) -> Option<PageKey> {
        Some(match key {
            "play" => PageKey::Play,
            "queue" => PageKey::Queue,
            "star" => PageKey::Star,
            "rating" => PageKey::Rating,
            "credits" => PageKey::Reveal(Reveal::Credits),
            "notes" => PageKey::Reveal(Reveal::Notes),
            "extras" => PageKey::Reveal(Reveal::Extras),
            "versions" => PageKey::Reveal(Reveal::Versions),
            "merges" => PageKey::Reveal(Reveal::Merges),
            "tags" => PageKey::Reveal(Reveal::Tags),
            "history" => PageKey::Reveal(Reveal::History),
            "lyrics" => PageKey::Lyrics,
            "file" => PageKey::File,
            "note" => PageKey::Note,
            "extra" => PageKey::Extra,
            "og" => PageKey::Og,
            "version" => PageKey::Version,
            "unmerge" => PageKey::Unmerge,
            "restore" => PageKey::Restore,
            "sync" => PageKey::Sync,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        PAGE.iter().map(|s| s.key).find(|k| PageKey::of(k) == Some(self)).unwrap_or("")
    }
}
