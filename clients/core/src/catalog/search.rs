//! Fuzzy search over the whole library, for the command palette: letters in order,
//! not necessarily together ("gdspln" finds "God's Plan"), each word of the query
//! matched on its own, anywhere in a track's title, artists and album (or an album's
//! title and artists, an artist's names). Done with `nucleo-matcher`, the matcher of the
//! Helix editor.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::text::Texts;
use super::Catalog;

/// The best matches of each kind, best first: (position in the catalog, score).
#[derive(Default)]
pub struct Hits {
    pub artists: Vec<(u32, u32)>,
    pub albums: Vec<(u32, u32)>,
    pub tracks: Vec<(u32, u32)>,
    /// Extras (places in `Catalog::extras`).
    pub extras: Vec<(u32, u32)>,
}

/// One kind of thing to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Artists,
    Albums,
    Tracks,
    Extras,
}

pub struct Search {
    matcher: Matcher,
    buffer: Vec<char>,
}

impl Default for Search {
    fn default() -> Self {
        Search { matcher: Matcher::new(Config::DEFAULT), buffer: Vec::new() }
    }
}

impl Search {
    /// Up to `limit` of each kind.
    pub fn run(&mut self, catalog: &Catalog, query: &str, limit: usize) -> Hits {
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let (tracks, albums, artists) = catalog.texts();
        Hits {
            artists: self.best(&pattern, artists, limit),
            albums: self.best(&pattern, albums, limit),
            tracks: self.best(&pattern, tracks, limit),
            extras: self.best(&pattern, catalog.extra_texts(), limit),
        }
    }

    /// What a search line finds: plain words are matched fuzzily against everything; with
    /// `key:value` parts, `artist:` searches artists by that name, `album:` albums by
    /// title (both fuzzy), albums also by the keys that apply to albums (artist, year,
    /// kind), and the tracks matching all of it, newest first. Up to `limit` of each.
    pub fn run_query(&mut self, catalog: &Catalog, text: &str, limit: usize) -> Hits {
        use super::index::{AlbumSort, TrackRows, TrackSort};
        use super::query::{Key, Query};
        let query = Query::parse(text);
        if query.fields.is_empty() {
            return self.run(catalog, text, limit);
        }
        let artists = match query.get(Key::Artist).filter(|v| !v.is_empty()) {
            Some(name) => self.run_part(catalog, name, Part::Artists, limit),
            None => Vec::new(),
        };
        let albums = match query.get(Key::Album).filter(|v| !v.is_empty()) {
            Some(title) => self.run_part(catalog, title, Part::Albums, limit),
            // Only with a key albums have (a key only tracks have would list every album).
            None if query.fields.iter().any(|(k, _)| matches!(k, Key::Artist | Key::Year | Key::Kind | Key::Title)) => {
                catalog.album_list(text, AlbumSort::Added).into_iter().take(limit).map(|i| (i, 0)).collect()
            }
            None => Vec::new(),
        };
        let rows = TrackRows { tracks: None, added: None };
        let tracks = catalog.order_tracks(&rows, text, TrackSort::Added, false).into_iter().take(limit).map(|i| (i as u32, 0)).collect();
        // (Keys are about tracks, albums and artists: no extras.)
        Hits { artists, albums, tracks, extras: Vec::new() }
    }

    /// Up to `limit` of one kind only (a form's artist or album field).
    pub fn run_part(&mut self, catalog: &Catalog, query: &str, part: Part, limit: usize) -> Vec<(u32, u32)> {
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let (tracks, albums, artists) = catalog.texts();
        let texts = match part {
            Part::Artists => artists,
            Part::Albums => albums,
            Part::Tracks => tracks,
            Part::Extras => catalog.extra_texts(),
        };
        self.best(&pattern, texts, limit)
    }

    fn best(&mut self, pattern: &Pattern, texts: &Texts, limit: usize) -> Vec<(u32, u32)> {
        if pattern.atoms.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(u32, u32)> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let haystack = Utf32Str::new(text, &mut self.buffer);
            if let Some(score) = pattern.score(haystack, &mut self.matcher) {
                hits.push((i as u32, score));
            }
        }
        // The best `limit`, without sorting all of them: pick them out first.
        let by_score = |a: &(u32, u32), b: &(u32, u32)| b.1.cmp(&a.1).then(a.0.cmp(&b.0));
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit, by_score);
            hits.truncate(limit);
        }
        hits.sort_by(by_score);
        hits
    }
}
