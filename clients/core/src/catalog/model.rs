//! The catalog as the server sends it (see the server's `src/db/library.rs`): per kind a
//! list of `columns` and `rows`, each row an array in that order.
//!
//! serde can read a struct from an array (the fields in order), so each row type below
//! lists its fields in the server's column order, and `check_columns` makes sure the
//! server's columns are what this client expects.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

/// The catalog format this client reads: the number in the repository's `CATALOG_FORMAT`
/// file, the same one the server is built with.
pub const FORMAT: u32 = parse_format(include_str!("../../../../CATALOG_FORMAT"));

/// (`str::parse` isn't `const`.)
const fn parse_format(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut n = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            n = n * 10 + (bytes[i] - b'0') as u32;
        }
        i += 1;
    }
    n
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Primary,
    Featured,
    Producer,
    /// A role this client doesn't know yet.
    #[serde(other)]
    Other,
}

/// A credit: the artist, their role, and the alias they're credited under (if any).
#[derive(Debug, Clone, Deserialize)]
pub struct Credit(pub Uuid, pub Role, pub Option<Box<str>>);

/// A cover to show: its id (to download it) and its key, which is the same for the same
/// image under different covers (so each image is loaded and kept once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cover {
    pub id: Uuid,
    pub key: CoverKey,
}

/// The start of a cover image's SHA-256 (16 hex digits), as a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoverKey(pub u64);

impl CoverKey {
    /// From hex (a key, or a whole SHA-256: its start is used).
    pub fn from_hex(hex: &str) -> Option<CoverKey> {
        u64::from_str_radix(hex.get(..16)?, 16).ok().map(CoverKey)
    }
}

impl<'de> Deserialize<'de> for CoverKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<CoverKey, D::Error> {
        let hex = String::deserialize(d)?;
        CoverKey::from_hex(&hex).ok_or_else(|| serde::de::Error::custom("a cover key is 16 hex digits"))
    }
}

/// An album a track is on, with its disc and track number there.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Membership(pub Uuid, pub Option<i16>, pub Option<i16>);

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub credits: Vec<Credit>,
    /// Its main album first.
    pub albums: Vec<Membership>,
    pub duration_ms: Option<i64>,
    pub file_id: Uuid,
    pub cover_id: Option<Uuid>,
    pub cover_key: Option<CoverKey>,
    /// As written, never reformatted.
    pub release_date: Option<String>,
    pub year: Option<i16>,
    pub status: String,
    pub genres: Vec<String>,
    pub created_at: String,
    pub is_restricted: bool,
    pub codec: Option<String>,
    /// The groupings it's in (collectives, eras, projects…).
    #[serde(default)]
    pub groupings: Vec<Uuid>,
    /// Where its sources are ("youtube", "soundcloud"…), sorted, each once.
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Set only when its file is an original file (an "og file"): the name the artist
    /// gave it.
    #[serde(default)]
    pub original_filename: Option<String>,
    /// Its file's bitrate (kbps) and sample rate (Hz), where known.
    #[serde(default)]
    pub bitrate: Option<i32>,
    #[serde(default)]
    pub sample_rate: Option<i32>,
    /// Its file's SHA-256 (hex): the name it's kept under offline.
    #[serde(default)]
    pub sha256: Option<String>,
}

pub(super) const TRACK_COLUMNS: &[&str] = &[
    "id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id",
    "cover_key", "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec",
    "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256",
];

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    pub id: Uuid,
    pub title: String,
    pub kind: String,
    pub artist_display: String,
    pub credits: Vec<Credit>,
    pub release_date: Option<String>,
    pub year: Option<i32>,
    pub cover_id: Option<Uuid>,
    pub cover_key: Option<CoverKey>,
    pub created_at: String,
    pub track_count: i64,
    pub duration_ms: i64,
    /// The groupings it's in.
    #[serde(default)]
    pub groupings: Vec<Uuid>,
}

pub(super) const ALBUM_COLUMNS: &[&str] = &[
    "id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id",
    "cover_key", "created_at", "track_count", "duration_ms", "groupings",
];

/// A grouping: a collective (an account several artists post under), an era, a
/// project… `track_count`/`album_count`: of those the user can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grouping {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub track_count: i64,
    pub album_count: i64,
    /// Its members (said by hand, not worked out from credits).
    pub artists: Vec<Uuid>,
}

pub(super) const GROUPING_COLUMNS: &[&str] = &["id", "kind", "name", "track_count", "album_count", "artists"];

/// An extra (vocals, stems, a sample…: not a track), to search and play.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extra {
    pub id: Uuid,
    pub name: String,
    /// "vocals", "stems", "sample"… (free text).
    pub kind: String,
    pub is_restricted: bool,
    /// Taken off: left out.
    pub hidden: bool,
    /// What it belongs to: ("track" | "album" | "artist" | "grouping", id).
    pub links: Vec<(String, Uuid)>,
    /// How many files it has.
    pub files: i64,
    /// Its first file (by name), to play, and its length.
    pub first_file: Option<Uuid>,
    pub duration_ms: Option<i64>,
    /// Its first video file (to watch), and that video's thumbnail.
    #[serde(default)]
    pub video: Option<Uuid>,
    #[serde(default)]
    pub thumb_key: Option<String>,
}

impl Extra {
    /// Its video's thumbnail, as a cover to show.
    pub fn thumb(&self) -> Option<Cover> {
        Some(Cover { id: self.video?, key: CoverKey::from_hex(self.thumb_key.as_deref()?)? })
    }
}

pub(super) const EXTRA_COLUMNS: &[&str] = &["id", "name", "kind", "is_restricted", "hidden", "links", "files", "first_file", "duration_ms", "video", "thumb_key"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: Uuid,
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<String>,
    pub track_count: i64,
    pub album_count: i64,
}

impl Artist {
    /// Its name with its disambiguation, if it has one: "Kai (UK producer)".
    pub fn label(&self) -> String {
        if self.disambiguation.is_empty() { self.name.clone() } else { format!("{} ({})", self.name, self.disambiguation) }
    }
}

pub(super) const ARTIST_COLUMNS: &[&str] = &["id", "name", "disambiguation", "aliases", "track_count", "album_count"];

/// The user's own plays, rating and star on a track, album or artist.
// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Annotation {
    pub kind: String,
    pub id: Uuid,
    pub play_count: i32,
    pub last_played_at: Option<String>,
    /// 0 to 10 (half stars); 0: not rated.
    pub rating: i16,
    /// Set when starred.
    pub starred_at: Option<String>,
}

pub(super) const ANNOTATION_COLUMNS: &[&str] = &["kind", "id", "play_count", "last_played_at", "rating", "starred_at"];

/// A track's open review flags: the track, and each flag's id and kind (a kind from
/// `/api/vocab`'s `review_kinds`), in the order to show them. In a delta, a track with
/// none left has an empty list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issues(pub Uuid, pub Vec<(Uuid, String)>);

pub(super) const ISSUE_COLUMNS: &[&str] = &["track_id", "flags"];

/// A lyric: its track, id, language, whether it's official and synced, and its text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyric(pub Uuid, pub Uuid, pub Option<String>, pub bool, pub bool, pub String);

impl Lyric {
    pub fn as_lyric(&self) -> (Uuid, super::lyrics::Lyric<'_>) {
        (self.0, super::lyrics::Lyric { id: self.1, language: self.2.as_deref(), is_official: self.3, is_synced: self.4, body: &self.5 })
    }
}

pub(super) const LYRIC_COLUMNS: &[&str] = &["track_id", "id", "language", "is_official", "is_synced", "body"];

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub public: bool,
    pub owner: String,
    pub mine: bool,
    pub entries: i64,
    pub duration_ms: i64,
    /// When it last changed (its entries, their order, its name…).
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Deserialize)]
pub struct Part<T> {
    pub columns: Vec<String>,
    pub rows: Vec<T>,
}

impl<T> Default for Part<T> {
    fn default() -> Self {
        Part { columns: Vec::new(), rows: Vec::new() }
    }
}

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Deserialize)]
pub struct Raw {
    pub format: u32,
    /// The catalog's version: ask `?since=` it for what changed (None: an older server).
    #[serde(default)]
    pub version: Option<String>,
    /// Only what changed since a version (to apply to a copy), not the whole catalog.
    #[serde(default)]
    pub delta: bool,
    /// In a delta: what to drop.
    #[serde(default)]
    pub removed: Removed,
    pub tracks: Part<Track>,
    pub albums: Part<Album>,
    pub artists: Part<Artist>,
    pub annotations: Part<Annotation>,
    /// Tracks' open review flags.
    pub issues: Part<Issues>,
    /// Lyrics (a delta: the changed tracks', all of them; none for a track: it has none).
    #[serde(default)]
    pub lyrics: Part<Lyric>,
    /// Every grouping (a delta has them all too: there are few).
    pub groupings: Part<Grouping>,
    /// Extras (a delta: the changed ones).
    #[serde(default)]
    pub extras: Part<Extra>,
    pub playlists: Vec<Playlist>,
}

/// What a delta drops: tracks, albums and artists merged into another; tracks, albums and
/// extras the user can't see anymore (restricted, turned into extras); and annotations that
/// say nothing anymore.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Removed {
    pub tracks: Vec<Uuid>,
    pub albums: Vec<Uuid>,
    pub artists: Vec<Uuid>,
    pub annotations: Vec<Uuid>,
    /// Extras the user can't see anymore (restricted).
    pub extras: Vec<Uuid>,
}

/// A whole catalog with its tracks and albums left as text, a row each: they're read one at
/// a time into the catalog (`index::Merger`), so the server's rows are never all in memory
/// at once (at 160k tracks they'd take some 300 MB).
#[derive(Deserialize)]
pub struct Base<'a> {
    #[serde(default)]
    pub version: Option<String>,
    #[serde(borrow)]
    pub tracks: Part<&'a RawValue>,
    #[serde(borrow)]
    pub albums: Part<&'a RawValue>,
    pub artists: Part<Artist>,
    pub annotations: Part<Annotation>,
    pub issues: Part<Issues>,
    #[serde(default)]
    pub lyrics: Part<Lyric>,
    pub groupings: Part<Grouping>,
    #[serde(default)]
    pub extras: Part<Extra>,
    pub playlists: Vec<Playlist>,
}

/// Whether a catalog body is a delta, after checking its format: a format this client
/// doesn't know is an error that says to update the app.
pub fn is_delta(body: &[u8]) -> anyhow::Result<bool> {
    #[derive(Deserialize)]
    struct Format {
        format: u32,
        #[serde(default)]
        delta: bool,
    }
    // The format first, so a newer server gets a clear message rather than a parse error.
    let format: Format = serde_json::from_slice(body)?;
    if format.format != FORMAT {
        anyhow::bail!("the server's library format is {} and this app reads {FORMAT}: update the app", format.format);
    }
    Ok(format.delta)
}

/// Reads a whole catalog body (not a delta), its tracks and albums left as text.
pub fn parse_base(body: &[u8]) -> anyhow::Result<Base<'_>> {
    is_delta(body)?;
    let base: Base = serde_json::from_str(std::str::from_utf8(body)?)?;
    check_columns("tracks", &base.tracks.columns, TRACK_COLUMNS)?;
    check_columns("albums", &base.albums.columns, ALBUM_COLUMNS)?;
    check_columns("artists", &base.artists.columns, ARTIST_COLUMNS)?;
    check_columns("annotations", &base.annotations.columns, ANNOTATION_COLUMNS)?;
    check_columns("issues", &base.issues.columns, ISSUE_COLUMNS)?;
    if !base.lyrics.columns.is_empty() {
        check_columns("lyrics", &base.lyrics.columns, LYRIC_COLUMNS)?;
    }
    check_columns("groupings", &base.groupings.columns, GROUPING_COLUMNS)?;
    if !base.extras.columns.is_empty() {
        check_columns("extras", &base.extras.columns, EXTRA_COLUMNS)?;
    }
    Ok(base)
}

/// Reads a catalog body, every row (for deltas, which are small). A format or columns this
/// client doesn't know is an error that says to update the app (rather than showing things
/// in the wrong places).
pub fn parse(body: &[u8]) -> anyhow::Result<Raw> {
    is_delta(body)?;
    let raw: Raw = serde_json::from_slice(body)?;
    check_columns("tracks", &raw.tracks.columns, TRACK_COLUMNS)?;
    check_columns("albums", &raw.albums.columns, ALBUM_COLUMNS)?;
    check_columns("artists", &raw.artists.columns, ARTIST_COLUMNS)?;
    check_columns("annotations", &raw.annotations.columns, ANNOTATION_COLUMNS)?;
    check_columns("issues", &raw.issues.columns, ISSUE_COLUMNS)?;
    if !raw.lyrics.columns.is_empty() {
        check_columns("lyrics", &raw.lyrics.columns, LYRIC_COLUMNS)?;
    }
    check_columns("groupings", &raw.groupings.columns, GROUPING_COLUMNS)?;
    if !raw.extras.columns.is_empty() {
        check_columns("extras", &raw.extras.columns, EXTRA_COLUMNS)?;
    }
    Ok(raw)
}

pub(super) fn check_columns(part: &str, got: &[String], expected: &[&str]) -> anyhow::Result<()> {
    if got.iter().map(String::as_str).ne(expected.iter().copied()) {
        anyhow::bail!("the server's {part} columns ({}) aren't what this app reads: update the app", got.join(", "));
    }
    Ok(())
}
