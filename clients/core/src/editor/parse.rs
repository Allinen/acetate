//! The editor's line, declared once (`LINE`, the language is `crate::commandline::schema`'s):
//! the editor's commands, then the track page's (`page_key::PAGE`: `play:`, `note:`, `og:`…,
//! meaningful on a track's page only). What a value means is worked out by `resolve`, which
//! knows the library.
//!
//! * Arguments: `role:`, `as:` and `n:` go with an artist (`artist:Ann role:featured
//!   as:Annie n:2`), `kind:` and `n:` with an album (`album:X n:2-5`: disc 2, track 5;
//!   `n:1+`: in order from 1), `kind:` with a grouping or related track, `n:` and `which:`
//!   with a cover, `split:`, `whole:`, `keep:`, `word:` with `parse:`. After `source:`, any
//!   other `name:` is one of its fields (`source:distrokid album_uuid:… index:1`).
//! * Places: the value is what, `n:` is where. `cover:3 n:1` makes the third cover the main
//!   one, `-cover:2` puts the second in the trash, `source:1` corrects the first source,
//!   `origin:2` says the file came from the second.
//! * The filter's keys (`art:`, `gen:`, `stat:`…) are typed by any start of their names,
//!   two letters at least; the editor's own by their names.
//! * Anything else in a value is the value: `title:No: Way`, `keep:cdq -cover`.

use super::page_key::PAGE;
use crate::commandline::schema::{self, Kind, Minus, Next, Schema, Spec, Word};

const ALONG: &[Word] = &[Word { word: "on", help: "albums it's alone on made to match it, changed or not" }, Word { word: "off", help: "only the tracks" }];
const WHOLE: &[Word] = &[Word { word: "on", help: "names that are an artist's kept whole" }, Word { word: "off", help: "split them by the rules" }];

/// An artist's arguments (`artist:` and `feat:`).
const CREDIT: &[Spec] = &[
    Spec::new("role", Kind::Text, "its role: primary, featured, producer… (another role: a credit of its own)").with_minus(),
    Spec::new("as", Kind::Or(&[Word { word: "own", help: "its own name" }], &Kind::AliasOf(&[])), "the name it's credited under"),
    Spec::new("n", Kind::Place, "its place among the artists"),
];

/// The editor's commands, in the order the list offers them.
const COMMANDS: &[Spec] = &[
    Spec::new("title", Kind::Text, "its title").aliases(&["name"]).prefix(),
    Spec::new("artist", Kind::Artist, "credit one, or change one's name or place (-artist:Name takes one off; -artist: all)").aliases(&["artists", "by"]).prefix().minus(Minus::OffOrClear).next(Next::Args(CREDIT)),
    Spec::new("feat", Kind::Artist, "credit one as featured").aliases(&["featuring", "ft", "featured"]).prefix().minus(Minus::Off).next(Next::Args(CREDIT)),
    Spec::new("album", Kind::Album, "on it as well (n: its numbers; -album:X off one; -album: all)").aliases(&["on"]).prefix().minus(Minus::OffOrClear).next(Next::Args(&[
        Spec::new("n", Kind::Place, "its numbers: 5, 2-5 (disc 2, track 5), or 1+ (in order from 1)"),
        Spec::new("kind", Kind::Text, "a new album's kind: album, ep, single…"),
    ])),
    Spec::new("date", Kind::Text, "released, as the source wrote it (none: no date)").aliases(&["year", "released"]).prefix().minus(Minus::OffOrClear),
    Spec::new("genre", Kind::Text, "genres to add, by commas (-genre:x takes one off; -genre: all)").aliases(&["genres"]).prefix().minus(Minus::OffOrClear),
    Spec::new("status", Kind::Text, "released, demo…").prefix(),
    Spec::new("flag", Kind::Text, "a flag raised by hand (-flag:x settles that kind)").aliases(&["missing", "issue"]).prefix().minus(Minus::Off),
    Spec::new("disc", Kind::Number, "its disc on the album").aliases(&["disk", "cd"]).prefix(),
    Spec::new("track", Kind::Text, "its track number on the album (N+: in order from N)").aliases(&["number", "numbers", "no", "renumber"]).prefix(),
    Spec::new("group", Kind::Grouping, "into a grouping, or a new one (kind:)").aliases(&["grouping"]).prefix().minus(Minus::Off).next(Next::Args(&[Spec::new("kind", Kind::Text, "a new grouping's kind: collective, era, project…")])),
    Spec::new("comment", Kind::Text, "its comment (none: no comment)").minus(Minus::Off),
    Spec::new("related", Kind::Track, "a related track (-related:X takes one off; -related: all)").aliases(&["relation"]).minus(Minus::OffOrClear).next(Next::Args(&[Spec::new("kind", Kind::Text, "how: a remix of it, an instrumental…")])),
    Spec::new("cover", Kind::Text, "paste, file, another album's or track's (n: where it goes); a number: that one moved (-cover:N to the trash; -cover: all)")
        .aliases(&["covers"])
        .minus(Minus::OffOrClear)
        .next(Next::Args(&[Spec::new("n", Kind::Place, "where it goes (1: the main one)"), Spec::new("which", Kind::Number, "which of that album's or track's covers")])),
    Spec::new("source", Kind::Text, "a link, a platform and its fields, or custom; = its only one; a number: correct that one (-source:N to the trash)").aliases(&["sources"]).minus(Minus::OffOrClear).next(Next::Fields(&[])),
    Spec::new("origin", Kind::Number, "the file came from its Nth source"),
    Spec::new("suffix", Kind::Text, "added to the end of the title (-suffix:x takes from the last x on off; alone: put back)").minus(Minus::Off),
    Spec::new("parse", Kind::Text, "the credits in the title, by the rules typed (feat prod…)").next(Next::Args(&[
        Spec::new("split", Kind::Text, "split names at these too"),
        Spec::new("whole", Kind::Words(WHOLE), "names that are an artist's kept whole"),
        Spec::new("keep", Kind::Text, "words kept in the title"),
        Spec::new("word", Kind::Text, "a word that credits: \"beat by\"=prod"),
    ])),
    Spec::new("single", Kind::Nothing, "a single of its own, for each"),
    Spec::new("along", Kind::Words(ALONG), "albums it's alone on: made to match it, or left as they are"),
    Spec::new("why", Kind::Text, "kept with the change").aliases(&["reason"]),
];

/// The editor's line: its commands, then the track page's.
pub static LINE: Schema = Schema { commands: COMMANDS, also: PAGE };

/// The editor's commands alone, and the track page's alone (for the help and the docs).
pub static EDITOR: Schema = Schema { commands: COMMANDS, also: &[] };
pub static TRACK_PAGE: Schema = Schema { commands: PAGE, also: &[] };

pub type Line = schema::Line;
pub type Node = schema::Node;

pub fn parse(text: &str) -> Line {
    schema::parse(&LINE, text)
}

/// A value as it is to be typed in the line (quoted where it needs it).
pub fn quoted(value: &str) -> String {
    LINE.quoted(value)
}

/// Whether a command is one of the track page's (not the editor's own).
pub fn is_page(spec: &Spec) -> bool {
    LINE.also.iter().any(|s| std::ptr::eq(s, spec))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(text: &str) -> Vec<(String, Vec<(String, String)>)> {
        parse(text).commands.iter().map(|c| (format!("{}{}={}", if c.minus { "-" } else { "" }, c.name(), c.value), c.args.iter().map(|a| (a.name().to_string(), a.value.clone())).collect())).collect()
    }

    #[test]
    fn commands_their_arguments_and_starts() {
        let s = |k: &str, v: &str, args: &[(&str, &str)]| (format!("{k}={v}"), args.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>());
        assert_eq!(shape("art:Ann role:featured as:Annie n:2 | title:No: Way"), [s("artist", "Ann", &[("role", "featured"), ("as", "Annie"), ("n", "2")]), s("title", "No: Way", &[])]);
        assert_eq!(shape("album:Long Player n:2-5 | -genre"), [s("album", "Long Player", &[("n", "2-5")]), s("-genre", "", &[])]);
        assert_eq!(shape("parse:feat split:-x keep:tape -cover"), [s("parse", "feat", &[("split", "-x"), ("keep", "tape -cover")])]);
        assert_eq!(shape("source:distrokid album_uuid:x index:1"), [s("source", "distrokid", &[("album_uuid", "x"), ("index", "1")])]);
        assert_eq!(shape("re:2020"), [s("date", "2020", &[])], "released: a start of the date's names, as a filter's");
        assert_eq!(shape("rate:4 | play:"), [s("rating", "4", &[]), s("play", "", &[])], "the page's, exactly");
        assert_eq!(shape("extra:Stems kind:stems source:a friend"), [s("extra", "Stems", &[("kind", "stems"), ("source", "a friend")])]);
        assert!(parse("format:flac").commands.is_empty(), "a filter's key that isn't a change: words");
        assert!(parse("c:x").commands.is_empty(), "one letter isn't a start");
    }

    #[test]
    fn values_are_quoted_where_they_would_split() {
        assert_eq!(quoted("No: Way"), "\"No: Way\"");
        assert_eq!(quoted("a | b"), "\"a | b\"");
        assert_eq!(quoted("Plain"), "Plain");
    }
}
