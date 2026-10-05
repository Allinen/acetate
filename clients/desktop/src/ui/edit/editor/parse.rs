//! The editor's keys: the same keys (and abbreviations) as a filter (`catalog::query`)
//! plus the editor's own (`comment:`, `related:`, `why:`, `role:`, `as:`, `along:`,
//! `cover:`, `source:`, `origin:`, `single:`, `suffix:`, `parse:` with `split:`, `whole:`,
//! `keep:`, `word:`; `flag:` is the filter's). The syntax is the commandline's
//! (`crate::commandline`): `|` between changes, quotes, `-key`, `#…`. What a value means is
//! worked out by the editor, which knows the library.
//!
//! * Arguments: `role:` and `as:` go with an artist (`artist:Ann role:featured as:Annie
//!   #2`), `kind:` with an album, grouping or related track, `split:`, `whole:`, `keep:`,
//!   `word:` with `parse:`. After `source:`, any other `name:` is one of its fields
//!   (`source:distrokid album_uuid:… index:1`).
//! * Anything else in a value is the value: `title:No: Way`, `keep:cdq -cover`.
//! * `#…` after an artist or album: its place (`#2`) or numbers (`#2-5` disc 2 track 5,
//!   `#1+` in order from 1). After a cover or source, which one (`-cover:#2`), and where
//!   it goes (`cover:#3 #1`: the third made the main one).

use crate::catalog::query::Key as QueryKey;
use crate::commandline::{self, Keys};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Title,
    Date,
    Comment,
    Status,
    Genre,
    Artist,
    Featured,
    Album,
    Disc,
    Number,
    Related,
    Group,
    Why,
    Role,
    As,
    Kind,
    Along,
    Cover,
    Source,
    Origin,
    Single,
    Flag,
    /// `suffix:` adds to the end of the title, `-suffix:` takes off it.
    Suffix,
    /// `parse:` the credits in the title, with the rules typed (`crate::title_credits`).
    Parse,
    Split,
    Whole,
    Keep,
    Word,
    /// One of a source's fields, after `source:<platform>` (`album_uuid:`, `index:`): its
    /// name is the part's `field`.
    Field,
    /// A filter's key that isn't something to change (`format:`, `og:`…).
    Other(QueryKey),
}

impl K {
    /// Its name, as written in chips.
    pub fn name(self) -> &'static str {
        match self {
            K::Title => "title",
            K::Date => "date",
            K::Comment => "comment",
            K::Status => "status",
            K::Genre => "genre",
            K::Artist => "artist",
            K::Featured => "feat",
            K::Album => "album",
            K::Disc => "disc",
            K::Number => "track",
            K::Related => "related",
            K::Group => "group",
            K::Why => "why",
            K::Role => "role",
            K::As => "as",
            K::Kind => "kind",
            K::Along => "along",
            K::Cover => "cover",
            K::Source => "source",
            K::Origin => "origin",
            K::Single => "single",
            K::Suffix => "suffix",
            K::Flag => "flag",
            K::Parse => "parse",
            K::Split => "split",
            K::Whole => "whole",
            K::Keep => "keep",
            K::Word => "word",
            K::Field => "field",
            K::Other(k) => k.name(),
        }
    }

    /// Whether `-key` alone means "clear it".
    pub fn is_list(self) -> bool {
        matches!(self, K::Genre | K::Artist | K::Featured | K::Album | K::Related | K::Group | K::Cover | K::Source)
    }
}

/// The key a typed name means: the editor's own names exactly, then a filter's key (any
/// start of its names, see `catalog::query::Key::parse`).
pub fn key(name: &str) -> Option<K> {
    let own = match name.to_lowercase().as_str() {
        "comment" => Some(K::Comment),
        "related" | "relation" => Some(K::Related),
        "why" | "reason" => Some(K::Why),
        "role" => Some(K::Role),
        "as" => Some(K::As),
        "along" => Some(K::Along),
        "cover" | "covers" => Some(K::Cover),
        "source" | "sources" => Some(K::Source),
        "origin" => Some(K::Origin),
        "single" => Some(K::Single),
        "suffix" => Some(K::Suffix),
        "parse" => Some(K::Parse),
        "split" => Some(K::Split),
        "whole" => Some(K::Whole),
        "keep" => Some(K::Keep),
        "word" => Some(K::Word),
        _ => None,
    };
    if own.is_some() {
        return own;
    }
    Some(match QueryKey::parse(name)? {
        QueryKey::Artist => K::Artist,
        QueryKey::Featured => K::Featured,
        QueryKey::Album => K::Album,
        QueryKey::Title => K::Title,
        QueryKey::Year => K::Date,
        QueryKey::Genre => K::Genre,
        QueryKey::Status => K::Status,
        QueryKey::Disc => K::Disc,
        QueryKey::Number => K::Number,
        QueryKey::Group => K::Group,
        QueryKey::Kind => K::Kind,
        QueryKey::Flag => K::Flag,
        other => K::Other(other),
    })
}

pub type Part = commandline::Part<K>;
pub type Line = commandline::Line<K>;

impl Keys for K {
    fn key(name: &str) -> Option<K> {
        key(name)
    }

    fn name(self) -> &'static str {
        K::name(self)
    }

    fn is_list(self) -> bool {
        K::is_list(self)
    }

    fn takes_hash(self) -> bool {
        matches!(self, K::Artist | K::Featured | K::Album | K::Role | K::As | K::Kind | K::Cover | K::Source | K::Origin)
    }

    fn takes(main: K, arg: K) -> bool {
        match main {
            K::Artist | K::Featured => matches!(arg, K::Role | K::As),
            K::Album | K::Group | K::Related => arg == K::Kind,
            K::Parse => matches!(arg, K::Split | K::Whole | K::Keep | K::Word),
            _ => false,
        }
    }

    fn field(main: K) -> Option<K> {
        (main == K::Source).then_some(K::Field)
    }
}

pub fn parse(text: &str) -> Line {
    commandline::parse::<K>(text)
}

/// `#…` as numbers: (disc, track, in order). `2` → track 2; `2-5` → disc 2, track 5;
/// `1+` → in order from 1. None: not numbers.
pub fn numbers(hash: &str) -> Option<(Option<i16>, Option<i16>, bool)> {
    let (rest, in_order) = match hash.strip_suffix('+') {
        Some(rest) => (rest, true),
        None => (hash, false),
    };
    if rest.is_empty() {
        return if in_order { None } else { Some((None, None, false)) };
    }
    match rest.split_once('-') {
        Some((disc, track)) => Some((Some(disc.parse().ok()?), Some(track.parse().ok()?), in_order)),
        None => Some((None, Some(rest.parse().ok()?), in_order)),
    }
}

/// Quotes a value where it needs it, to be typed back (see `commandline::quoted`).
pub fn quoted(value: &str) -> String {
    commandline::quoted::<K>(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<(K, bool, String, Option<String>)> {
        parse(text).parts.into_iter().map(|p| (p.key, p.minus, p.value, p.hash)).collect()
    }

    #[test]
    fn changes_are_apart_by_bars() {
        assert_eq!(
            parts("artist:lil quill | year:2017 | status:demo"),
            [(K::Artist, false, "lil quill".into(), None), (K::Date, false, "2017".into(), None), (K::Status, false, "demo".into(), None)]
        );
        // Without one, another key is part of the value.
        assert_eq!(parts("status:demo date:2020"), [(K::Status, false, "demo date:2020".into(), None)]);
        assert_eq!(parts("title:Intro: Part 2")[0].2, "Intro: Part 2");
        assert_eq!(parts("title:No: Way | year:2020")[0].2, "No: Way");
        // Words before any key.
        assert_eq!(parse("no date").words, ["no", "date"]);
        assert_eq!(parse("no date").parts, []);
    }

    #[test]
    fn quotes_keep_bars() {
        assert_eq!(parts("title:\"guy | more\" | status:demo")[0].2, "guy | more");
        assert_eq!(parts("-suffix:\" | Prod\"")[0].2, " | Prod", "spaces in quotes kept");
        assert_eq!(quoted("guy|more"), "\"guy|more\"");
        assert_eq!(quoted("No: Way"), "\"No: Way\"");
        assert_eq!(quoted("Plain song"), "Plain song");
    }

    #[test]
    fn minus_hash_and_arguments() {
        assert_eq!(parts("-artist | artist:A"), [(K::Artist, true, "".into(), None), (K::Artist, false, "A".into(), None)]);
        assert!(!parse("-artist").parts[0].colon);
        assert_eq!(parts("-genre:rap"), [(K::Genre, true, "rap".into(), None)]);
        // `-title` alone isn't anything: a plain word.
        assert_eq!(parse("-title").words, ["-title"]);
        let line = parse("artist:Ann Lee role:feat as:Annie #2");
        assert_eq!(line.parts.iter().map(|p| (p.key, p.arg)).collect::<Vec<_>>(), [(K::Artist, false), (K::Role, true), (K::As, true)]);
        assert_eq!(line.parts[2].hash.as_deref(), Some("2"));
        assert_eq!(parts("album:Some LP #2-5"), [(K::Album, false, "Some LP".into(), Some("2-5".into()))]);
        assert_eq!(parts("title:Track #1")[0].2, "Track #1", "a title keeps its #");
        assert_eq!(numbers("2-5"), Some((Some(2), Some(5), false)));
        assert_eq!(numbers("1+"), Some((None, Some(1), true)));
        assert_eq!(numbers(""), Some((None, None, false)));
        assert_eq!(numbers("x"), None);
        // `role:` after a bar: a change of its own (which the editor says is wrong).
        assert!(!parse("status:demo | role:x").parts[1].arg);
    }

    #[test]
    fn covers_and_sources_take_hashes() {
        let line = parse("cover:#3 #1 | -cover:#2 | cover:Long Player #2 | source:https://youtu.be/x | origin:#1");
        let p = &line.parts;
        assert_eq!((p[0].key, p[0].value.as_str(), p[0].hashes.clone()), (K::Cover, "", vec!["3".to_string(), "1".to_string()]));
        assert_eq!((p[1].minus, p[1].hashes.clone()), (true, vec!["2".to_string()]));
        assert_eq!((p[2].value.as_str(), p[2].hash.as_deref()), ("Long Player", Some("2")));
        assert_eq!((p[3].key, p[3].value.as_str()), (K::Source, "https://youtu.be/x"));
        assert_eq!((p[4].key, p[4].hash.as_deref()), (K::Origin, Some("1")));
        assert!(!parse("-cover").parts[0].colon);
    }

    #[test]
    fn a_sources_fields() {
        let line = parse("source:distrokid album_uuid:x-1 index:2 url:https://a.b/c status:demo");
        let p: Vec<(K, &str, &str)> = line.parts.iter().map(|p| (p.key, p.name(), p.value.as_str())).collect();
        assert_eq!(p, [(K::Source, "source", "distrokid"), (K::Field, "album_uuid", "x-1"), (K::Field, "index", "2"), (K::Field, "url", "https://a.b/c"), (K::Field, "status", "demo")]);
        // A link's `https:` isn't a field; elsewhere a field is part of the value.
        assert_eq!(parts("source:custom vk, see https://a.b")[0].2, "custom vk, see https://a.b");
        assert_eq!(parts("title:a index:1")[0].2, "a index:1");
        assert_eq!(parts("-source:#1 index:1")[0].0, K::Source);
        assert_eq!(parse("-source:#1 index:1").parts.len(), 1);
        // A change after the fields: after a bar.
        assert_eq!(parse("source:distrokid index:1 | status:demo").parts[2].key, K::Status);
    }

    #[test]
    fn keys_are_the_filters_plus_the_editors() {
        assert_eq!(key("art"), Some(K::Artist));
        assert_eq!(key("yr"), None);
        assert_eq!(key("released"), Some(K::Date));
        assert_eq!(key("no"), Some(K::Number));
        assert_eq!(key("related"), Some(K::Related));
        assert_eq!(key("rel"), Some(K::Date), "a start of a filter's key (released)");
        assert_eq!(key("why"), Some(K::Why));
        assert_eq!(key("format"), Some(K::Other(QueryKey::Format)));
    }

    #[test]
    fn parse_and_what_goes_with_it() {
        assert_eq!(
            parts("parse:feat prod [prod] split:-x -and keep:cdq -cover word:vs=featured"),
            [
                (K::Parse, false, "feat prod [prod]".into(), None),
                (K::Split, false, "-x -and".into(), None),
                (K::Keep, false, "cdq -cover".into(), None),
                (K::Word, false, "vs=featured".into(), None),
            ]
        );
        // After a bar `-cover` is still "no covers".
        assert_eq!(parts("status:demo | -cover")[1].0, K::Cover);
    }

    #[test]
    fn where_a_choice_goes() {
        let line = "status:demo | artist:an";
        let p = &parse(line).parts[1];
        assert_eq!(&line[p.value_start..p.end], "an");
    }
}
