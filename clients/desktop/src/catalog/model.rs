//! The catalog as the server sends it (see the server's `src/db/library.rs`): per kind a
//! list of `columns` and `rows`, each row an array in that order.
//!
//! serde can read a struct from an array (the fields in order), so each row type below
//! lists its fields in the server's column order, and `check_columns` makes sure the
//! server's columns are what this client expects.

use serde::Deserialize;
use uuid::Uuid;

/// The catalog format this client reads (the server's `FORMAT`).
pub const FORMAT: u32 = 1;

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
pub struct Credit(pub Uuid, pub Role, pub Option<String>);

/// An album a track is on, with its disc and track number there.
#[derive(Debug, Clone, Deserialize)]
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
    /// As written, never reformatted.
    pub release_date: Option<String>,
    pub year: Option<i16>,
    pub status: String,
    pub genres: Vec<String>,
    pub created_at: String,
    pub is_restricted: bool,
    pub codec: Option<String>,
}

const TRACK_COLUMNS: &[&str] = &[
    "id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id",
    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec",
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
    pub created_at: String,
    pub track_count: i64,
    pub duration_ms: i64,
}

const ALBUM_COLUMNS: &[&str] = &[
    "id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id",
    "created_at", "track_count", "duration_ms",
];

#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    pub id: Uuid,
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<String>,
    pub track_count: i64,
    pub album_count: i64,
}

const ARTIST_COLUMNS: &[&str] = &["id", "name", "disambiguation", "aliases", "track_count", "album_count"];

/// The user's own plays, rating and star on a track, album or artist.
// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
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

const ANNOTATION_COLUMNS: &[&str] = &["kind", "id", "play_count", "last_played_at", "rating", "starred_at"];

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub public: bool,
    pub owner: String,
    pub mine: bool,
    pub entries: i64,
    pub duration_ms: i64,
}

#[derive(Deserialize)]
pub struct Part<T> {
    pub columns: Vec<String>,
    pub rows: Vec<T>,
}

// Every column is read (rows are arrays in column order); not all are shown yet.
#[allow(dead_code)]
#[derive(Deserialize)]
pub struct Raw {
    pub format: u32,
    pub tracks: Part<Track>,
    pub albums: Part<Album>,
    pub artists: Part<Artist>,
    pub annotations: Part<Annotation>,
    pub playlists: Vec<Playlist>,
}

/// Reads a catalog body. A format or columns this client doesn't know is an error that
/// says to update the app (rather than showing things in the wrong places).
pub fn parse(body: &[u8]) -> anyhow::Result<Raw> {
    #[derive(Deserialize)]
    struct Format {
        format: u32,
    }
    // The format first, so a newer server gets a clear message rather than a parse error.
    let format: Format = serde_json::from_slice(body)?;
    if format.format != FORMAT {
        anyhow::bail!("the server's library format is {} and this app reads {FORMAT}: update the app", format.format);
    }
    let raw: Raw = serde_json::from_slice(body)?;
    check_columns("tracks", &raw.tracks.columns, TRACK_COLUMNS)?;
    check_columns("albums", &raw.albums.columns, ALBUM_COLUMNS)?;
    check_columns("artists", &raw.artists.columns, ARTIST_COLUMNS)?;
    check_columns("annotations", &raw.annotations.columns, ANNOTATION_COLUMNS)?;
    Ok(raw)
}

fn check_columns(part: &str, got: &[String], expected: &[&str]) -> anyhow::Result<()> {
    if got.iter().map(String::as_str).ne(expected.iter().copied()) {
        anyhow::bail!("the server's {part} columns ({}) aren't what this app reads: update the app", got.join(", "));
    }
    Ok(())
}
