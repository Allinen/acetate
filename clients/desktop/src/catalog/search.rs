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
