//! Tracks and albums as the catalog keeps them: a small fixed-size row each, its strings
//! in the catalog's `Text` and its lists (credits, albums, genres…) in shared pools, all
//! referred to by place. `Track` and `Album` are how they're read: a row with its catalog,
//! copied freely, each field a method.
//!
//! The server's rows (`model::Track`, `model::Album`) are only what's parsed: each goes
//! into the pools as it comes (`Builder`), and isn't kept.

use uuid::Uuid;

use super::flat::{Col, Flat};
use super::model::{self, Cover, CoverKey, Credit, Membership};
use super::text::{Str, Text, TextBuilder};

/// A run of items in one of the pools.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub(super) struct Span {
    start: u32,
    len: u32,
}

impl Span {
    fn of<T>(self, pool: &[T]) -> &[T] {
        &pool[self.start as usize..(self.start + self.len) as usize]
    }
}

const RESTRICTED: u8 = 1;
const HAS_COVER_KEY: u8 = 2;

// The rows are written to a snapshot as they are in memory: `repr(C)`, with the padding
// spelled out so there's none left (a test checks the sizes).
#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct TrackRow {
    pub(super) id: Uuid,
    file_id: Uuid,
    /// Nil: none.
    cover_id: Uuid,
    /// Its file's SHA-256; all zeros: not known.
    sha256: [u8; 32],
    cover_key: u64,
    title: Str,
    artist_display: Str,
    release_date: Str,
    status: Str,
    codec: Str,
    original_filename: Str,
    credits: Span,
    albums: Span,
    genres: Span,
    groupings: Span,
    platforms: Span,
    /// `u32::MAX`: not known.
    duration_ms: u32,
    /// `i32::MIN`: not known.
    bitrate: i32,
    sample_rate: i32,
    /// `i16::MIN`: none.
    year: i16,
    flags: u8,
    _pad: u8,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub(super) struct AlbumRow {
    pub(super) id: Uuid,
    cover_id: Uuid,
    cover_key: u64,
    title: Str,
    kind: Str,
    artist_display: Str,
    release_date: Str,
    credits: Span,
    groupings: Span,
    duration_ms: i64,
    track_count: u32,
    /// `i32::MIN`: none.
    year: i32,
    flags: u8,
    _pad: [u8; 7],
}

// SAFETY: `repr(C)` of integers, `Uuid`s, `Str`s and `Span`s, no padding (sizes checked
// in the tests); any bytes are valid.
unsafe impl Flat for Span {}
unsafe impl Flat for TrackRow {}
unsafe impl Flat for AlbumRow {}

/// The pools rows' lists are kept in.
#[derive(Default)]
pub(super) struct Pools {
    pub(super) text: Text,
    pub(super) credits: Vec<Credit>,
    pub(super) memberships: Vec<Membership>,
    /// Groupings.
    pub(super) ids: Col<Uuid>,
    /// Genres and platforms.
    pub(super) strs: Col<Str>,
}

impl Pools {
    #[cfg(test)]
    pub(super) fn bytes(&self) -> Vec<(&'static str, usize)> {
        use std::mem::size_of;
        vec![
            ("text", self.text.bytes()),
            ("credits", self.credits.capacity() * size_of::<Credit>() + self.credits.iter().filter_map(|c| c.2.as_ref()).map(|a| (a.len() + 8).div_ceil(16) * 16).sum::<usize>()),
            ("albums tracks are on", self.memberships.capacity() * size_of::<Membership>()),
            ("groupings, genres, platforms", self.ids.len() * 16 + self.strs.len() * size_of::<Str>()),
        ]
    }
}

/// Fills the pools, one server row at a time.
#[derive(Default)]
pub(super) struct Builder {
    text: TextBuilder,
    credits: Vec<Credit>,
    memberships: Vec<Membership>,
    ids: Vec<Uuid>,
    strs: Vec<Str>,
}

fn span<T>(pool: &mut Vec<T>, items: impl IntoIterator<Item = T>) -> Span {
    let start = pool.len() as u32;
    pool.extend(items);
    Span { start, len: pool.len() as u32 - start }
}

impl Builder {
    pub(super) fn track(&mut self, t: model::Track) -> TrackRow {
        let text = &mut self.text;
        let genres = t.genres.iter().map(|g| text.shared(g)).collect::<Vec<_>>();
        let platforms = t.platforms.iter().map(|p| text.shared(p)).collect::<Vec<_>>();
        TrackRow {
            id: t.id,
            file_id: t.file_id,
            cover_id: t.cover_id.unwrap_or(Uuid::nil()),
            sha256: t.sha256.as_deref().and_then(unhex).unwrap_or([0; 32]),
            cover_key: t.cover_key.map(|k| k.0).unwrap_or(0),
            title: self.text.push(&t.title),
            artist_display: self.text.shared(&t.artist_display),
            release_date: self.text.shared_opt(t.release_date.as_deref()),
            status: self.text.shared(&t.status),
            codec: self.text.shared_opt(t.codec.as_deref()),
            original_filename: self.text.push_opt(t.original_filename.as_deref()),
            credits: span(&mut self.credits, t.credits),
            albums: span(&mut self.memberships, t.albums),
            genres: span(&mut self.strs, genres),
            groupings: span(&mut self.ids, t.groupings),
            platforms: span(&mut self.strs, platforms),
            duration_ms: t.duration_ms.and_then(|d| u32::try_from(d).ok()).filter(|&d| d != u32::MAX).unwrap_or(u32::MAX),
            bitrate: t.bitrate.unwrap_or(i32::MIN),
            sample_rate: t.sample_rate.unwrap_or(i32::MIN),
            year: t.year.unwrap_or(i16::MIN),
            flags: if t.is_restricted { RESTRICTED } else { 0 } | if t.cover_key.is_some() { HAS_COVER_KEY } else { 0 },
            _pad: 0,
        }
    }

    pub(super) fn album(&mut self, a: model::Album) -> AlbumRow {
        AlbumRow {
            id: a.id,
            cover_id: a.cover_id.unwrap_or(Uuid::nil()),
            cover_key: a.cover_key.map(|k| k.0).unwrap_or(0),
            title: self.text.push(&a.title),
            kind: self.text.shared(&a.kind),
            artist_display: self.text.shared(&a.artist_display),
            release_date: self.text.shared_opt(a.release_date.as_deref()),
            credits: span(&mut self.credits, a.credits),
            groupings: span(&mut self.ids, a.groupings),
            duration_ms: a.duration_ms,
            track_count: a.track_count.clamp(0, u32::MAX as i64) as u32,
            year: a.year.unwrap_or(i32::MIN),
            flags: if a.cover_key.is_some() { HAS_COVER_KEY } else { 0 },
            _pad: [0; 7],
        }
    }

    pub(super) fn finish(mut self) -> Pools {
        self.credits.shrink_to_fit();
        self.memberships.shrink_to_fit();
        self.ids.shrink_to_fit();
        self.strs.shrink_to_fit();
        Pools { text: self.text.finish(), credits: self.credits, memberships: self.memberships, ids: self.ids.into(), strs: self.strs.into() }
    }
}

/// 64 hex digits as 32 bytes.
fn unhex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn cover(id: Uuid, key: u64, flags: u8) -> Option<Cover> {
    (!id.is_nil() && flags & HAS_COVER_KEY != 0).then_some(Cover { id, key: CoverKey(key) })
}

/// A track in the catalog.
#[derive(Clone, Copy)]
pub struct Track<'a> {
    pools: &'a Pools,
    row: &'a TrackRow,
}

impl<'a> Track<'a> {
    pub(super) fn new(pools: &'a Pools, row: &'a TrackRow) -> Track<'a> {
        Track { pools, row }
    }

    pub fn id(self) -> Uuid {
        self.row.id
    }

    pub fn title(self) -> &'a str {
        self.pools.text.get(self.row.title)
    }

    pub fn artist_display(self) -> &'a str {
        self.pools.text.get(self.row.artist_display)
    }

    pub fn credits(self) -> &'a [Credit] {
        self.row.credits.of(&self.pools.credits)
    }

    /// Its main album first.
    pub fn albums(self) -> &'a [Membership] {
        self.row.albums.of(&self.pools.memberships)
    }

    pub fn duration_ms(self) -> Option<i64> {
        (self.row.duration_ms != u32::MAX).then_some(self.row.duration_ms as i64)
    }

    pub fn file_id(self) -> Uuid {
        self.row.file_id
    }

    pub fn cover_id(self) -> Option<Uuid> {
        (!self.row.cover_id.is_nil()).then_some(self.row.cover_id)
    }

    pub fn cover(self) -> Option<Cover> {
        cover(self.row.cover_id, self.row.cover_key, self.row.flags)
    }

    /// As written, never reformatted.
    pub fn release_date(self) -> Option<&'a str> {
        self.pools.text.opt(self.row.release_date)
    }

    pub fn year(self) -> Option<i16> {
        (self.row.year != i16::MIN).then_some(self.row.year)
    }

    pub fn status(self) -> &'a str {
        self.pools.text.get(self.row.status)
    }

    pub fn genres(self) -> impl Iterator<Item = &'a str> + Clone {
        let text = &self.pools.text;
        self.row.genres.of(&self.pools.strs).iter().map(move |s| text.get(*s))
    }

    pub fn is_restricted(self) -> bool {
        self.row.flags & RESTRICTED != 0
    }

    pub fn codec(self) -> Option<&'a str> {
        self.pools.text.opt(self.row.codec)
    }

    /// The groupings it's in (collectives, eras, projects…).
    pub fn groupings(self) -> &'a [Uuid] {
        self.row.groupings.of(&self.pools.ids)
    }

    /// Where its sources are ("youtube", "soundcloud"…), sorted, each once.
    pub fn platforms(self) -> impl Iterator<Item = &'a str> + Clone {
        let text = &self.pools.text;
        self.row.platforms.of(&self.pools.strs).iter().map(move |s| text.get(*s))
    }

    /// Set only when its file is an original file (an "og file"): the name the artist
    /// gave it.
    pub fn original_filename(self) -> Option<&'a str> {
        self.pools.text.opt(self.row.original_filename)
    }

    /// Its file's bitrate (kbps), where known.
    pub fn bitrate(self) -> Option<i32> {
        (self.row.bitrate != i32::MIN).then_some(self.row.bitrate)
    }

    /// Its file's SHA-256 (hex), where known: what it's kept under offline.
    pub fn sha256(self) -> Option<String> {
        let bytes = &self.row.sha256;
        bytes.iter().any(|b| *b != 0).then(|| bytes.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Its file's sample rate (Hz), where known.
    pub fn sample_rate(self) -> Option<i32> {
        (self.row.sample_rate != i32::MIN).then_some(self.row.sample_rate)
    }

    /// As the server sent it (for building a catalog again with a delta). `created_at`
    /// isn't kept (nothing shows it).
    pub(super) fn to_model(self) -> model::Track {
        model::Track {
            id: self.id(),
            title: self.title().to_string(),
            artist_display: self.artist_display().to_string(),
            credits: self.credits().to_vec(),
            albums: self.albums().to_vec(),
            duration_ms: self.duration_ms(),
            file_id: self.file_id(),
            cover_id: self.cover_id(),
            cover_key: (self.row.flags & HAS_COVER_KEY != 0).then_some(CoverKey(self.row.cover_key)),
            release_date: self.release_date().map(str::to_string),
            year: self.year(),
            status: self.status().to_string(),
            genres: self.genres().map(str::to_string).collect(),
            created_at: String::new(),
            is_restricted: self.is_restricted(),
            codec: self.codec().map(str::to_string),
            groupings: self.groupings().to_vec(),
            platforms: self.platforms().map(str::to_string).collect(),
            original_filename: self.original_filename().map(str::to_string),
            bitrate: self.bitrate(),
            sample_rate: self.sample_rate(),
            sha256: self.sha256(),
        }
    }
}

/// An album in the catalog.
#[derive(Clone, Copy)]
pub struct Album<'a> {
    pools: &'a Pools,
    row: &'a AlbumRow,
}

impl<'a> Album<'a> {
    pub(super) fn new(pools: &'a Pools, row: &'a AlbumRow) -> Album<'a> {
        Album { pools, row }
    }

    pub fn id(self) -> Uuid {
        self.row.id
    }

    pub fn title(self) -> &'a str {
        self.pools.text.get(self.row.title)
    }

    /// "album", "ep", "single"…
    pub fn kind(self) -> &'a str {
        self.pools.text.get(self.row.kind)
    }

    pub fn artist_display(self) -> &'a str {
        self.pools.text.get(self.row.artist_display)
    }

    pub fn credits(self) -> &'a [Credit] {
        self.row.credits.of(&self.pools.credits)
    }

    pub fn release_date(self) -> Option<&'a str> {
        self.pools.text.opt(self.row.release_date)
    }

    pub fn year(self) -> Option<i32> {
        (self.row.year != i32::MIN).then_some(self.row.year)
    }

    pub fn cover_id(self) -> Option<Uuid> {
        (!self.row.cover_id.is_nil()).then_some(self.row.cover_id)
    }

    pub fn cover(self) -> Option<Cover> {
        cover(self.row.cover_id, self.row.cover_key, self.row.flags)
    }

    pub fn track_count(self) -> i64 {
        self.row.track_count as i64
    }

    pub fn duration_ms(self) -> i64 {
        self.row.duration_ms
    }

    /// The groupings it's in.
    pub fn groupings(self) -> &'a [Uuid] {
        self.row.groupings.of(&self.pools.ids)
    }

    pub(super) fn to_model(self) -> model::Album {
        model::Album {
            id: self.id(),
            title: self.title().to_string(),
            kind: self.kind().to_string(),
            artist_display: self.artist_display().to_string(),
            credits: self.credits().to_vec(),
            release_date: self.release_date().map(str::to_string),
            year: self.year(),
            cover_id: self.cover_id(),
            cover_key: (self.row.flags & HAS_COVER_KEY != 0).then_some(CoverKey(self.row.cover_key)),
            created_at: String::new(),
            track_count: self.track_count(),
            duration_ms: self.duration_ms(),
            groupings: self.groupings().to_vec(),
        }
    }
}
