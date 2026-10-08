//! A catalog's text, kept compactly: every string in one buffer, and a string is where it
//! is in it (`Str`: 8 bytes, no allocation of its own). At 160k tracks that's hundreds of
//! thousands fewer allocations (each `String` is 24 bytes plus its own block on the heap).
//!
//! `Texts` is a list of strings the same way (the lowercase text to search), one after
//! another in one buffer.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use super::flat::Col;

/// A column of bytes that is always valid UTF-8: built from `&str`s, or checked once when
/// a snapshot is opened.
#[derive(Default)]
pub(super) struct Utf8(pub(super) Col<u8>);

impl Utf8 {
    pub(super) fn as_str(&self) -> &str {
        // SAFETY: only ever built from strings, or from bytes `checked` validated.
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    /// Bytes from a file, if they are text.
    pub(super) fn checked(col: Col<u8>) -> Option<Utf8> {
        std::str::from_utf8(&col).ok()?;
        Some(Utf8(col))
    }

    fn push(&mut self, s: &str) {
        self.0.make_mut().extend_from_slice(s.as_bytes());
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

/// A string in a `Text`, or none (`NONE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Str {
    start: u32,
    len: u32,
}

impl Str {
    pub const NONE: Str = Str { start: 0, len: u32::MAX };
}

#[derive(Default)]
pub struct Text {
    pub(super) buffer: Utf8,
}

impl Text {
    /// The string (`""` for `Str::NONE`).
    pub fn get(&self, s: Str) -> &str {
        self.opt(s).unwrap_or("")
    }

    pub fn opt(&self, s: Str) -> Option<&str> {
        if s == Str::NONE {
            return None;
        }
        Some(&self.buffer.as_str()[s.start as usize..s.start as usize + s.len as usize])
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.buffer.0.len()
    }
}

/// Builds a `Text`. `shared` strings are kept once however often they come (an artist's
/// name on each of their tracks, a status, a date), others as they come.
#[derive(Default)]
pub struct TextBuilder {
    text: Text,
    /// Shared strings by their hash (a collision only means one is kept twice).
    seen: HashMap<u64, Str>,
    hasher: RandomState,
}

impl TextBuilder {
    pub fn push(&mut self, s: &str) -> Str {
        let start = u32::try_from(self.text.buffer.len()).expect("a catalog's text under 4 GB");
        self.text.buffer.push(s);
        Str { start, len: s.len() as u32 }
    }

    pub fn push_opt(&mut self, s: Option<&str>) -> Str {
        s.map(|s| self.push(s)).unwrap_or(Str::NONE)
    }

    pub fn shared(&mut self, s: &str) -> Str {
        let hash = self.hasher.hash_one(s);
        if let Some(&kept) = self.seen.get(&hash)
            && self.text.get(kept) == s
        {
            return kept;
        }
        let new = self.push(s);
        self.seen.insert(hash, new);
        new
    }

    pub fn shared_opt(&mut self, s: Option<&str>) -> Str {
        s.map(|s| self.shared(s)).unwrap_or(Str::NONE)
    }

    pub fn finish(mut self) -> Text {
        self.text.buffer.0.make_mut().shrink_to_fit();
        self.text
    }
}

/// A list of strings, one after another in one buffer.
#[derive(Default)]
pub struct Texts {
    pub(super) buffer: Utf8,
    /// Where each one ends.
    pub(super) ends: Col<u32>,
}

impl Texts {
    pub fn with_capacity(n: usize) -> Texts {
        Texts { buffer: Utf8::default(), ends: Col::Owned(Vec::with_capacity(n)) }
    }

    /// The next one: `parts` lowercase, each after a `\0` (so a word can't match across two).
    pub fn push_lowercase(&mut self, parts: &[&str]) {
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                self.buffer.push("\0");
            }
            self.buffer.push(&part.to_lowercase());
        }
        let end = u32::try_from(self.buffer.len()).expect("search text under 4 GB");
        self.ends.make_mut().push(end);
    }

    pub fn get(&self, i: u32) -> &str {
        let start = if i == 0 { 0 } else { self.ends[i as usize - 1] as usize };
        &self.buffer.as_str()[start..self.ends[i as usize] as usize]
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        (0..self.ends.len() as u32).map(|i| self.get(i))
    }

    pub fn finish(mut self) -> Texts {
        self.buffer.0.make_mut().shrink_to_fit();
        self.ends.make_mut().shrink_to_fit();
        self
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.buffer.len() + self.ends.len() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_come_back_and_shared_ones_are_kept_once() {
        let mut b = TextBuilder::default();
        let a = b.shared("Ann");
        let t = b.push("Title");
        let a2 = b.shared("Ann");
        let none = b.push_opt(None);
        let empty = b.push("");
        let text = b.finish();
        assert_eq!(a, a2);
        assert_eq!((text.get(a), text.get(t), text.opt(none), text.opt(empty)), ("Ann", "Title", None, Some("")));
        assert_eq!(text.buffer.as_str(), "AnnTitle");
    }

    #[test]
    fn a_list_of_lowercase_texts() {
        let mut texts = Texts::default();
        texts.push_lowercase(&["Ann", "B"]);
        texts.push_lowercase(&[]);
        texts.push_lowercase(&["ÄÖ"]);
        let texts = texts.finish();
        assert_eq!(texts.iter().collect::<Vec<_>>(), ["ann\0b", "", "äö"]);
        assert_eq!(texts.len(), 3);
    }
}
