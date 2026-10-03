//! The editor's line, as typed: `key:value` parts, the same keys (and abbreviations) as a
//! filter (`catalog::query`) plus the editor's own (`comment:`, `related:`, `why:`,
//! `role:`, `as:`, `along:`, `cover:`, `source:`, `origin:`, `single:`; `flag:` is the
//! filter's). Only the syntax: what a value means is worked out by the
//! editor, which knows the library.
//!
//! * A value runs until the next key: `artist:lil quill year:2017`.
//! * `"…"` quotes a value that has a key-like word or a colon in it: `title:"No: Way"`
//!   (`no` is a key: track number).
//! * `-key:value` takes one off; `-key` alone clears the list (`-artist`, `-genre`…).
//! * `#…` after an artist or album: its place (`#2`) or numbers (`#2-5` disc 2 track 5,
//!   `#1+` in order from 1). After a cover or source, which one (`-cover:#2`), and where
//!   it goes (`cover:#3 #1`: the third made the main one); right after the colon too.
//! * `role:` and `as:` go with the artist before them, `kind:` with the album, grouping or
//!   related track before it.

use crate::catalog::query::Key as QueryKey;

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
            K::Flag => "flag",
            K::Other(k) => k.name(),
        }
    }

    /// Whether `-key` alone means "clear it".
    pub fn is_list(self) -> bool {
        matches!(self, K::Genre | K::Artist | K::Featured | K::Album | K::Related | K::Group | K::Cover | K::Source)
    }

    /// Whether `#…` after its value is its place or numbers.
    fn takes_hash(self) -> bool {
        matches!(self, K::Artist | K::Featured | K::Album | K::Role | K::As | K::Kind | K::Cover | K::Source | K::Origin)
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

#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    pub key: K,
    /// `-key…`: off, or (without a colon) clear.
    pub minus: bool,
    /// `key:` (vs `-key` alone).
    pub colon: bool,
    /// The value, trimmed, without its quotes.
    pub value: String,
    pub quoted: bool,
    /// What followed the last `#` (artist, album…), if there was one.
    pub hash: Option<String>,
    /// Every `#…`, in order (a cover moved: which, and where to).
    pub hashes: Vec<String>,
    /// Where the value starts in the line (bytes): a choice from the list replaces the
    /// value from here to `end`.
    pub value_start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    /// Words before any key (finding what to change by its name).
    pub words: Vec<String>,
    pub parts: Vec<Part>,
}

pub fn parse(text: &str) -> Line {
    let mut line = Line::default();
    // Tokens with where they start.
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    let mut start = None;
    for (i, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (true, Some(s)) => {
                tokens.push((s, &text[s..i]));
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push((s, &text[s..]));
    }

    let mut in_quote = false;
    for (at, token) in tokens {
        let end = at + token.len();
        if !in_quote && let Some(part) = key_token(at, token) {
            in_quote = part.quoted && !closes(&text[part.value_start..end]);
            line.parts.push(part);
            if let Some(p) = line.parts.last_mut() {
                p.end = end;
            }
            continue;
        }
        let Some(part) = line.parts.last_mut() else {
            line.words.push(token.to_string());
            continue;
        };
        if !in_quote && part.key.takes_hash() && is_hash(token) {
            part.hash = Some(token[1..].to_string());
            part.hashes.push(token[1..].to_string());
        } else if in_quote {
            in_quote = !token.ends_with('"');
        }
        part.end = end;
    }
    for part in &mut line.parts {
        let raw = text[part.value_start..part.end].trim();
        // The `#…`s aren't part of the value.
        let raw = if part.hash.is_some() && !part.quoted { without_hashes(raw) } else { raw };
        part.value = if part.quoted {
            let inner = raw.strip_prefix('"').unwrap_or(raw);
            let (inner, after) = inner.split_once('"').unwrap_or((inner, ""));
            let after = after.trim();
            if after.is_empty() { inner.to_string() } else { format!("{inner} {after}") }
        } else {
            raw.to_string()
        };
    }
    line
}

/// A token that starts a part: `key:…`, `-key:…`, or `-key` (a list key).
fn key_token(at: usize, token: &str) -> Option<Part> {
    let (minus, rest) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (name, value, colon) = match rest.split_once(':') {
        Some((name, value)) => (name, value, true),
        None => (rest, "", false),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    let k = key(name)?;
    if !colon && !(minus && k.is_list()) {
        return None;
    }
    let value_start = at + token.len() - value.len();
    // `cover:#3`: a `#` right after the colon.
    let hashes: Vec<String> = if k.takes_hash() && !value.is_empty() && is_hash(value) { vec![value[1..].to_string()] } else { Vec::new() };
    let hash = hashes.last().cloned();
    Some(Part { key: k, minus, colon, value: String::new(), quoted: value.starts_with('"'), hash, hashes, value_start, end: at + token.len() })
}

/// A value without the `#…` words at its end.
fn without_hashes(mut raw: &str) -> &str {
    loop {
        let start = raw.rfind(char::is_whitespace).map_or(0, |i| i + 1);
        if start < raw.len() && is_hash(&raw[start..]) {
            raw = raw[..start].trim_end();
        } else {
            return raw;
        }
    }
}

/// Whether a quoted value (from its opening quote) is closed.
fn closes(value: &str) -> bool {
    value.len() > 1 && value[1..].contains('"')
}

/// `#`, `#2`, `#2-5`, `#1+`.
fn is_hash(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('#') else { return false };
    let rest = rest.strip_suffix('+').unwrap_or(rest);
    rest.split('-').count() <= 2 && rest.split('-').all(|n| n.chars().all(|c| c.is_ascii_digit()))
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

/// Quotes a value where it needs it, to be typed back: one with a colon, a quote, or a
/// word that's a key.
pub fn quoted(value: &str) -> String {
    let needs = value.contains(':') || value.contains('"') || value.split_whitespace().any(|w| w.starts_with('#') || (w.starts_with('-') && key(&w[1..]).is_some()));
    if needs { format!("\"{}\"", value.replace('"', "'")) } else { value.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str) -> Vec<(K, bool, String, Option<String>)> {
        parse(text).parts.into_iter().map(|p| (p.key, p.minus, p.value, p.hash)).collect()
    }

    #[test]
    fn values_run_until_the_next_key() {
        assert_eq!(
            parts("artist:lil quill year:2017 status:demo"),
            [(K::Artist, false, "lil quill".into(), None), (K::Date, false, "2017".into(), None), (K::Status, false, "demo".into(), None)]
        );
        // A word with a colon that isn't a key is part of the value.
        assert_eq!(parts("title:Intro: Part 2")[0].2, "Intro: Part 2");
        // Words before any key.
        assert_eq!(parse("no date").words, ["no", "date"]);
        assert_eq!(parse("no date").parts, []);
    }

    #[test]
    fn quotes_keep_key_like_words() {
        assert_eq!(parts("title:\"No: Way\" year:2020"), [(K::Title, false, "No: Way".into(), None), (K::Date, false, "2020".into(), None)]);
        assert_eq!(parts("title:\"one no:two\"")[0].2, "one no:two");
        // Unclosed: the rest of the line.
        assert_eq!(parts("title:\"No: Way")[0].2, "No: Way");
        assert_eq!(quoted("No: Way"), "\"No: Way\"");
        assert_eq!(quoted("Plain song"), "Plain song");
    }

    #[test]
    fn minus_hash_and_stages() {
        assert_eq!(parts("-artist artist:A"), [(K::Artist, true, "".into(), None), (K::Artist, false, "A".into(), None)]);
        assert!(!parse("-artist").parts[0].colon);
        assert_eq!(parts("-genre:rap"), [(K::Genre, true, "rap".into(), None)]);
        // `-title` alone isn't anything: a plain word.
        assert_eq!(parse("-title").words, ["-title"]);
        assert_eq!(
            parts("artist:Ann Lee role:feat as:Annie #2"),
            [(K::Artist, false, "Ann Lee".into(), None), (K::Role, false, "feat".into(), None), (K::As, false, "Annie".into(), Some("2".into()))]
        );
        assert_eq!(parts("album:Some LP #2-5"), [(K::Album, false, "Some LP".into(), Some("2-5".into()))]);
        assert_eq!(parts("title:Track #1")[0].2, "Track #1", "a title keeps its #");
        assert_eq!(numbers("2-5"), Some((Some(2), Some(5), false)));
        assert_eq!(numbers("1+"), Some((None, Some(1), true)));
        assert_eq!(numbers(""), Some((None, None, false)));
        assert_eq!(numbers("x"), None);
    }

    #[test]
    fn covers_and_sources_take_hashes() {
        let line = parse("cover:#3 #1 -cover:#2 cover:Long Player #2 source:https://youtu.be/x origin:#1");
        let p = &line.parts;
        assert_eq!((p[0].key, p[0].value.as_str(), p[0].hashes.clone()), (K::Cover, "", vec!["3".to_string(), "1".to_string()]));
        assert_eq!((p[1].minus, p[1].hashes.clone()), (true, vec!["2".to_string()]));
        assert_eq!((p[2].value.as_str(), p[2].hash.as_deref()), ("Long Player", Some("2")));
        assert_eq!((p[3].key, p[3].value.as_str()), (K::Source, "https://youtu.be/x"));
        assert_eq!((p[4].key, p[4].hash.as_deref()), (K::Origin, Some("1")));
        assert!(!parse("-cover").parts[0].colon);
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
    fn where_a_choice_goes() {
        let line = "status:demo artist:an";
        let p = &parse(line).parts[1];
        assert_eq!(&line[p.value_start..p.end], "an");
    }
}
