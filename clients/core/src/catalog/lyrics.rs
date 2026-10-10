//! Every track's lyrics, in the catalog so they're there offline: one row per lyric,
//! sorted by track, with the text in one buffer (see `text`). Kept in a snapshot like the
//! rest (mapped: no parsing at start, no heap), and found by a binary search on the track.

use uuid::Uuid;

use super::flat::{Col, Flat};
use super::text::{Str, Text, TextBuilder};

/// One lyric as stored.
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct LyricRow {
    pub(super) track: Uuid,
    pub(super) id: Uuid,
    pub(super) language: Str,
    pub(super) body: Str,
    /// `OFFICIAL`, `SYNCED`.
    pub(super) flags: u8,
    pub(super) _pad: [u8; 3],
}

// SAFETY: `repr(C)`, 52 bytes (16 + 16 + 8 + 8 + 1 + 3), no padding left (spelled out), any
// bytes are valid.
unsafe impl Flat for LyricRow {}

impl LyricRow {
    /// Whether its strings are within a text of `len` bytes (a snapshot read from disk).
    pub(super) fn in_text(&self, len: usize) -> bool {
        self.body.fits(len) && (self.language == Str::NONE || self.language.fits(len))
    }
}

const OFFICIAL: u8 = 1;
const SYNCED: u8 = 2;

/// A track's lyric, as the catalog has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Lyric<'a> {
    pub id: Uuid,
    pub language: Option<&'a str>,
    pub is_official: bool,
    pub is_synced: bool,
    pub body: &'a str,
}

#[derive(Default)]
pub struct Lyrics {
    pub(super) rows: Col<LyricRow>,
    pub(super) text: Text,
}

impl Lyrics {
    /// From (track, lyric) in any order; kept by track, official ones first.
    pub fn build<'a>(lyrics: impl IntoIterator<Item = (Uuid, Lyric<'a>)>) -> Lyrics {
        let mut all: Vec<(Uuid, Lyric)> = lyrics.into_iter().collect();
        all.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.is_official.cmp(&a.1.is_official)).then(a.1.id.cmp(&b.1.id)));
        let mut text = TextBuilder::default();
        let rows = all
            .into_iter()
            .map(|(track, l)| LyricRow {
                track,
                id: l.id,
                language: text.shared_opt(l.language),
                body: text.push(l.body),
                flags: if l.is_official { OFFICIAL } else { 0 } | if l.is_synced { SYNCED } else { 0 },
                _pad: [0; 3],
            })
            .collect::<Vec<_>>();
        Lyrics { rows: rows.into(), text: text.finish() }
    }

    /// A track's lyrics, official ones first.
    pub fn of(&self, track: Uuid) -> impl Iterator<Item = Lyric<'_>> {
        let start = self.rows.partition_point(|r| r.track < track);
        self.rows[start..].iter().take_while(move |r| r.track == track).map(|r| self.lyric(r))
    }

    /// Every lyric with its track (in track order).
    pub fn all(&self) -> impl Iterator<Item = (Uuid, Lyric<'_>)> {
        self.rows.iter().map(|r| (r.track, self.lyric(r)))
    }

    fn lyric(&self, r: &LyricRow) -> Lyric<'_> {
        Lyric {
            id: r.id,
            language: self.text.opt(r.language),
            is_official: r.flags & OFFICIAL != 0,
            is_synced: r.flags & SYNCED != 0,
            body: self.text.get(r.body),
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Bytes they take: rows and text (for measuring).
    pub fn bytes(&self) -> usize {
        self.rows.as_bytes().len() + self.text.buffer.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lyric(id: u128, official: bool, body: &str) -> Lyric<'_> {
        Lyric { id: Uuid::from_u128(id), language: Some("en"), is_official: official, is_synced: false, body }
    }

    #[test]
    fn a_tracks_lyrics_are_found_official_first() {
        let (a, b, c) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
        let lyrics = Lyrics::build([(b, lyric(10, false, "b one")), (a, lyric(11, false, "a one")), (a, lyric(12, true, "a official"))]);
        let of = |t| lyrics.of(t).map(|l| l.body.to_string()).collect::<Vec<_>>();
        assert_eq!(of(a), ["a official", "a one"]);
        assert_eq!(of(b), ["b one"]);
        assert!(of(c).is_empty());
        assert_eq!(lyrics.len(), 3);
    }
}
