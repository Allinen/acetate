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
            artists: self.best(&pattern, query, artists, limit),
            albums: self.best(&pattern, query, albums, limit),
            tracks: self.best(&pattern, query, tracks, limit),
            extras: self.best(&pattern, query, catalog.extra_texts(), limit),
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
        let rows = TrackRows { tracks: None, added: None, faded: None };
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
        self.best(&pattern, query, texts, limit)
    }

    /// The best `limit`, best first: a name that is exactly what was typed, then ones that
    /// start with it, then by score (shorter names before longer at the same score, so
    /// "dead" comes before the hundred "deadsomething"s).
    fn best(&mut self, pattern: &Pattern, query: &str, texts: &Texts, limit: usize) -> Vec<(u32, u32)> {
        if pattern.atoms.is_empty() {
            return Vec::new();
        }
        let typed = query.trim().to_lowercase();
        // (how well the name itself matches: 0 exact, 1 starts with it, 2 other; score; length)
        let mut hits: Vec<(u8, u32, u32, u32)> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let haystack = Utf32Str::new(text, &mut self.buffer);
            if let Some(score) = pattern.score(haystack, &mut self.matcher) {
                let name = text.split('\0').next().unwrap_or(text);
                let near = if text.split('\0').any(|part| part == typed) {
                    0
                } else if name.starts_with(&typed) {
                    1
                } else {
                    2
                };
                hits.push((near, score, name.len() as u32, i as u32));
            }
        }
        // The best `limit`, without sorting all of them: pick them out first.
        let order = |a: &(u8, u32, u32, u32), b: &(u8, u32, u32, u32)| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)).then(a.3.cmp(&b.3));
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit, order);
            hits.truncate(limit);
        }
        hits.sort_by(order);
        hits.into_iter().map(|(_, score, _, i)| (i, score)).collect()
    }
}

/// A match as listed: its name, and what tells it apart (an artist's disambiguation and
/// size; an album's artists, kind and year).
pub fn describe(catalog: &Catalog, part: Part, i: u32) -> (String, String) {
    match part {
        Part::Artists => {
            let a = &catalog.artists[i as usize];
            let mut sub = Vec::new();
            if !a.disambiguation.is_empty() {
                sub.push(a.disambiguation.clone());
            }
            sub.push(if a.track_count == 1 { "1 track".into() } else { format!("{} tracks", a.track_count) });
            (a.name.clone(), sub.join(" · "))
        }
        Part::Extras => {
            let e = &catalog.extras[i as usize];
            let files = if e.files == 1 { "1 file".to_string() } else { format!("{} files", e.files) };
            (e.name.clone(), format!("{} · {files}", e.kind))
        }
        Part::Tracks => {
            let t = catalog.track_at(i);
            let mut sub = vec![t.artist_display().to_string()];
            if let Some(album) = catalog.main_album(t) {
                sub.push(album.title().to_string());
            }
            if let Some(codec) = t.codec() {
                sub.push(codec.to_string());
            }
            (t.title().to_string(), sub.join(" · "))
        }
        Part::Albums => {
            let a = catalog.album_at(i);
            let mut sub = vec![a.artist_display().to_string(), a.kind().to_string()];
            if let Some(year) = a.year() {
                sub.push(year.to_string());
            }
            (a.title().to_string(), sub.join(" · "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_typed_comes_before_longer_names_that_start_with_it() {
        let mut texts = Texts::with_capacity(5);
        for name in ["deadwhatever", "deadone", "deadtwo", "dead", "a dead end"] {
            texts.push_lowercase(&[name]);
        }
        let texts = texts.finish();
        let mut search = Search::default();
        let pattern = Pattern::parse("dead", CaseMatching::Ignore, Normalization::Smart);
        let found = search.best(&pattern, "dead", &texts, 2);
        assert_eq!(found[0].0, 3, "the exact name first");
        assert_eq!(found.len(), 2);
        // An alias that is exactly it counts as well.
        let mut texts = Texts::with_capacity(2);
        texts.push_lowercase(&["deadwhatever"]);
        texts.push_lowercase(&["Something Else", "dead"]);
        let found = search.best(&pattern, "dead", &texts.finish(), 1);
        assert_eq!(found[0].0, 1);
    }
}
