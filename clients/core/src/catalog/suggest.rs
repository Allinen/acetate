//! Completion for a search line (the query language of `query`), for a keyboard without
//! Tab: what could come next, as a few chips. The line is completed at its end.
//!
//! It's the search's list (`commandline::schema::rows` over `query::SEARCH`), each row as
//! the line it makes:
//!
//! * Nothing typed, or a finished value: the keys (`artist:`, `year:`…).
//! * A word that starts a key (`ar`): the keys it could be.
//! * After a key (`artist:li`): values from the library that fit (artists, albums, years,
//!   genres…), the more used ones first. A value runs until the next key, so a word of it
//!   that starts a key offers that key too.

use std::collections::HashMap;

use super::Catalog;
use super::query::{Key, SEARCH};
use crate::commandline::schema::{self, Lists, Node, Row, Spec};

/// One chip: what it says, and the whole line as it is once chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub label: String,
    pub line: String,
}

/// The keys worth offering for a search.
const OFFERED: &[Key] = &[
    Key::Artist,
    Key::Album,
    Key::Title,
    Key::Year,
    Key::Genre,
    Key::Kind,
    Key::Format,
    Key::Status,
    Key::Group,
    Key::Source,
    Key::Og,
    Key::Bitrate,
    Key::SampleRate,
    Key::Flag,
];

const LIMIT: usize = 12;

/// A search's list: the library's values for the key being typed.
struct Values<'a>(&'a Catalog);

impl Lists for Values<'_> {
    fn choices(&self, _kind: schema::Kind, typed: &str, node: &Node, _command: &Node) -> Vec<Row> {
        Key::of(node.spec.key).map(|k| values(self.0, k, typed)).unwrap_or_default().into_iter().map(|v| Row::value(v, "")).collect()
    }

    fn offers(&self, spec: &Spec) -> bool {
        Key::of(spec.key).is_some_and(|k| OFFERED.contains(&k))
    }
}

pub fn suggest(catalog: &Catalog, text: &str) -> Vec<Suggestion> {
    let line = schema::parse(&SEARCH, text);
    let ended = text.ends_with(char::is_whitespace);
    // Inside quotes: the user is writing it out.
    if line.current().is_some_and(|c| c.typed().quoted) && !ended {
        return Vec::new();
    }
    let keys_alone = matches!(schema::at(&line, text), schema::At::Command { word: None });
    let mut pins = schema::Pins::default();
    schema::rows(&SEARCH, &line, text, &Values(catalog))
        .into_iter()
        .take(if keys_alone { 8 } else { LIMIT })
        .map(|row| Suggestion { line: schema::take(&SEARCH, text, &row, true, &mut pins), label: row.label })
        .collect()
}

/// Values for `key` that fit what's typed of one: starts first, then words inside, then
/// letters in order; within each, the ones with most tracks.
fn values(catalog: &Catalog, key: Key, typed: &str) -> Vec<String> {
    let mut found: HashMap<String, i64> = HashMap::new();
    let mut add = |name: &str, weight: i64| {
        if !name.is_empty() {
            *found.entry(name.to_string()).or_default() += weight;
        }
    };
    match key {
        Key::Artist | Key::Featured => catalog.artists.iter().for_each(|a| add(&a.name, a.track_count)),
        Key::Album => catalog.albums().for_each(|a| add(a.title(), a.track_count())),
        Key::Kind => catalog.albums().for_each(|a| add(a.kind(), 1)),
        Key::Genre => catalog.tracks().for_each(|t| t.genres().for_each(|g| add(g, 1))),
        Key::Status => catalog.tracks().for_each(|t| add(t.status(), 1)),
        Key::Format => catalog.tracks().for_each(|t| add(t.codec().unwrap_or(""), 1)),
        Key::Source => catalog.tracks().for_each(|t| t.platforms().for_each(|p| add(p, 1))),
        Key::Group => catalog.groupings.iter().for_each(|g| add(&g.name, g.track_count)),
        // Newest first.
        Key::Year => catalog.tracks().for_each(|t| {
            if let Some(y) = t.year() {
                found.insert(y.to_string(), y as i64);
            }
        }),
        Key::Og => {
            add("yes", 2);
            add("no", 1);
        }
        _ => {}
    }
    let typed = typed.trim().to_lowercase();
    let mut ranked: Vec<(u8, i64, String)> = found
        .into_iter()
        .filter_map(|(name, weight)| {
            let lower = name.to_lowercase();
            let rank = if lower.starts_with(&typed) {
                0
            } else if lower.contains(&typed) {
                1
            } else if typed.chars().count() >= 2 && super::query::loosely_in(&typed, &name) {
                2
            } else {
                return None;
            };
            Some((rank, weight, name))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then_with(|| a.2.cmp(&b.2)));
    // Exactly what's typed isn't a suggestion.
    ranked.into_iter().map(|(_, _, name)| name).filter(|n| n.to_lowercase() != typed).take(LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::fixture;

    fn labels(text: &str) -> Vec<String> {
        suggest(&fixture::sample(), text).into_iter().map(|s| s.label).collect()
    }

    fn lines(text: &str) -> Vec<String> {
        suggest(&fixture::sample(), text).into_iter().map(|s| s.line).collect()
    }

    #[test]
    fn an_empty_line_offers_the_keys() {
        assert_eq!(&labels("")[..3], ["artist:", "album:", "title:"]);
    }

    #[test]
    fn the_start_of_a_key_is_completed() {
        assert_eq!(lines("ye"), ["year:"]);
        assert_eq!(lines("dark ar"), ["dark artist:"]);
        assert!(labels("gen").contains(&"genre:".to_string()));
    }

    #[test]
    fn a_value_is_completed_from_the_library() {
        assert_eq!(lines("artist:a"), ["artist:Ann "]);
        assert_eq!(lines("title:x artist:b")[0], "title:x artist:Bo ");
        // Everything of a key when nothing is typed after it, most used first.
        assert_eq!(&labels("artist:")[..2], ["Ann", "Bo"]);
        assert_eq!(labels("genre:"), ["ambient", "house"]);
        assert_eq!(labels("year:20"), ["2021", "2020"]);
        assert_eq!(labels("og:"), ["yes", "no"]);
    }

    #[test]
    fn what_is_typed_whole_is_not_offered_again() {
        assert!(labels("artist:ann").is_empty());
    }

    #[test]
    fn after_a_finished_value_the_keys_come_next() {
        assert_eq!(&labels("artist:Ann ")[..2], ["artist:", "album:"]);
        // A word of a value that starts a key is offered as that key too.
        assert!(lines("artist:Ann ye").contains(&"artist:Ann year:".to_string()));
    }

    #[test]
    fn inside_quotes_nothing_is_offered() {
        assert!(labels("artist:\"An").is_empty());
    }
}
