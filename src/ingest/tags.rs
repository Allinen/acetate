//! Reading embedded tags, cover art and lyrics via lofty. Read-only, always.
//!
//! Two views of the same tags are read:
//! * `raw`: every item of every tag in the file, verbatim, using lofty's format-specific
//!   tag types (Vorbis comments, MP4 atoms, APE items) and the `id3` crate for ID3v2
//!   (lofty parses ID3v2 timestamps and loses the original text, e.g. a trailing "Z").
//!   This includes custom keys like ORIGINALFILENAME that lofty's generic `Tag` would drop.
//! * The handful of mapped fields (title, artist, ...) via lofty's generic `Tag`,
//!   which knows e.g. that TIT2, TITLE and ©nam all mean "title".
//!
//! Values are never parsed, trimmed or normalized. What the tags say is what we store.
//! One exception: an ID3v2.3 date given as a year (TYER) and a day and month (TDAT) is put
//! together into one date, as ID3v2.4 does in TDRC (see `resolve_date`).

use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::id3::v2::{Frame, Id3v2Tag};
use lofty::iff::wav::RiffInfoList;
use lofty::mp4::{AtomData, AtomIdent, Ilst};
use lofty::ogg::tag::VorbisComments;
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagType};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod id3_repeated;

#[derive(Debug, Clone, Default)]
pub struct TagInfo {
    /// Every tag in the file, verbatim.
    pub raw: Vec<RawTag>,

    pub title: Option<String>,
    /// The artists from the ARTIST tag, one per value, with values like "A;B" split (see
    /// `split_artists`). These are the track's credits.
    pub artists: Vec<String>,
    pub album: Option<String>,
    /// The same from ALBUM ARTIST: the album's credits (may differ from the track's).
    pub album_artists: Vec<String>,
    /// From COMPOSER, split like ARTIST: credited as producers (see `ingest`).
    pub composers: Vec<String>,
    pub track_number: Option<u32>,
    /// How many tracks the album has (TRACKTOTAL, or the "10" in TRACK "3/10").
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub date: Option<String>,
    /// Where `date` was read from, when it isn't one of the usual date tags as it is (e.g.
    /// "TYER+TDAT", "ICRD"; see `resolve_date`).
    pub date_from: Option<String>,
    pub genres: Vec<String>,
    pub comment: Option<String>,
    pub lyrics: Vec<String>,
    pub pictures: Vec<EmbeddedPicture>,
    /// An original file's name, from an ORIGINALFILENAME / TOFN / ... tag, verbatim. The
    /// old tag-based library kept the names of original files there, so it's where they're
    /// read from; see `files.original_filename`.
    pub original_filename: Option<String>,

    /// From the audio properties lofty reads alongside the tags.
    pub bitrate_kbps: Option<u32>,
    pub duration_ms: Option<u64>,

    /// What the uploader should hear about the tags (e.g. two dates that disagree).
    pub warnings: Vec<String>,
}

/// One tag block in the file (a file can have several, e.g. ID3v2 and APE on an MP3).
#[derive(Debug, Clone, Serialize)]
pub struct RawTag {
    /// One of `RAW_FORMATS`.
    pub format: &'static str,
    pub items: Vec<RawItem>,
}

/// The names of the tag formats in `RawTag::format`. "riff_info" is a WAV file's RIFF INFO
/// list; WAV files imported before it was read have only their ID3v2 chunk in the dump.
const RAW_FORMATS: [&str; 6] = ["vorbis_comments", "id3v2", "mp4_ilst", "ape", "riff_info", "generic"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawItem {
    /// The key exactly as in the file: "TITLE", "TPE1", "TXXX", "©nam", "----:com.apple.iTunes:FOO"...
    pub key: String,
    /// Usually a string; numbers, booleans, lists and binary data are kept in their own shape.
    pub value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl RawItem {
    fn new(key: impl Into<String>, value: serde_json::Value) -> Self {
        RawItem {
            key: key.into(),
            value,
            description: None,
            language: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddedPicture {
    pub data: Vec<u8>,
    pub is_front_cover: bool,
}

/// Blocking: call from `spawn_blocking`.
///
/// `extension` is the original file's extension. Stored/temp files have none, so
/// it's used as a fallback when the format can't be recognized from the content.
pub fn read_tags(path: &Path, extension: Option<&str>) -> anyhow::Result<TagInfo> {
    let mut probe = Probe::open(path)?.guess_file_type()?;
    if probe.file_type().is_none()
        && let Some(file_type) = extension.and_then(FileType::from_ext) {
            probe = probe.set_file_type(file_type);
        }
    let tagged_file = probe.read()?;

    let props = tagged_file.properties();
    let mut info = TagInfo {
        bitrate_kbps: props.audio_bitrate().or(props.overall_bitrate()),
        duration_ms: Some(props.duration().as_millis() as u64).filter(|ms| *ms > 0),
        ..Default::default()
    };

    info.raw = match read_raw_tags(path, tagged_file.file_type()) {
        Ok(Some(raw)) => raw,
        // Formats without a format-specific reader below: dump the generic tags instead.
        Ok(None) => tagged_file.tags().iter().map(generic_dump).collect(),
        Err(err) => {
            tracing::warn!("reading raw tags failed, falling back to generic dump: {err:#}");
            tagged_file.tags().iter().map(generic_dump).collect()
        }
    };
    // Text frames given more than once (lofty and the id3 crate keep only one of each).
    let repeated = match tagged_file.file_type() {
        FileType::Mpeg | FileType::Aac | FileType::Wav | FileType::Aiff | FileType::Flac => {
            id3_repeated::repeated_text_frames(path).unwrap_or_else(|err| {
                tracing::warn!("scanning ID3 frames failed: {err:#}");
                Vec::new()
            })
        }
        _ => Vec::new(),
    };
    if !repeated.is_empty() {
        // Keep every frame in the raw dump, as it's in the file.
        if let Some(raw) = info.raw.iter_mut().find(|t| t.format == "id3v2") {
            for (id, values) in &repeated {
                raw.items.retain(|item| item.key != *id);
                for value in values {
                    raw.items.push(RawItem::new(id.clone(), json!(value)));
                }
            }
        }
    }
    strip_nuls(&mut info.raw);
    info.original_filename = find_original_filename(&info.raw);
    let primary_format = raw_format_name(tagged_file.primary_tag_type());

    // The "primary" tag is the preferred one for the format (e.g. Vorbis comments for
    // FLAC). Fall back to whatever tag exists (e.g. an ID3v2 tag stuck on a FLAC file).
    let Some(tag) = tagged_file.primary_tag().or(tagged_file.first_tag()) else {
        return Ok(info);
    };

    info.title = tag.title().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
    // Only ARTIST and ALBUM ARTIST count. Tags like DISPLAY ARTIST or origartist are
    // ignored (they're still in `raw`).
    info.artists = split_artists(strings(tag, ItemKey::TrackArtist));
    if info.artists.is_empty() {
        info.artists = split_artists(strings(tag, ItemKey::TrackArtists));
    }
    info.album = tag.album().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
    info.album_artists = split_artists(strings(tag, ItemKey::AlbumArtist));
    info.composers = split_artists(strings(tag, ItemKey::Composer));
    // ARTIST / ALBUM ARTIST / COMPOSER given as several frames (one per name, as some
    // taggers write ID3v2.3): all of them, in order.
    if primary_format == Some("id3v2") {
        for (id, values) in &repeated {
            let values = split_artists(values.clone());
            match id.as_str() {
                "TPE1" => info.artists = values,
                "TPE2" => info.album_artists = values,
                "TCOM" => info.composers = values,
                _ => {}
            }
        }
    }
    info.track_number = tag.track();
    info.track_total = tag.track_total();
    info.disc_number = tag.disk();
    // Dates come from the raw tags: lofty's generic view of ID3v2 dates is re-rendered
    // from a parsed timestamp and not verbatim.
    let resolved = resolve_date(&info.raw, primary_format);
    info.date_from = resolved.from;
    if !resolved.conflicts.is_empty()
        && let Some(date) = &resolved.date
    {
        info.warnings.push(format!(
            "the tags' release date is {date}, but {}; {date} was kept",
            resolved.conflicts.join(", ")
        ));
    }
    info.date = resolved.date.or_else(|| {
        [ItemKey::ReleaseDate, ItemKey::RecordingDate, ItemKey::Year]
            .into_iter()
            .filter_map(|key| tag.get_string(key))
            .find(|s| !s.trim().is_empty())
            .map(str::to_string)
    });
    info.genres = strings(tag, ItemKey::Genre);
    info.comment = tag.comment().map(|s| s.to_string()).filter(|s| !s.trim().is_empty());
    info.lyrics = strings(tag, ItemKey::Lyrics);
    for lyrics in strings(tag, ItemKey::UnsyncLyrics) {
        if !info.lyrics.contains(&lyrics) {
            info.lyrics.push(lyrics);
        }
    }

    info.pictures = tag
        .pictures()
        .iter()
        .map(|pic| EmbeddedPicture {
            data: pic.data().to_vec(),
            is_front_cover: pic.pic_type() == PictureType::CoverFront,
        })
        .collect();
    // Front cover first.
    info.pictures.sort_by_key(|p| !p.is_front_cover);

    Ok(info)
}

/// All values for a key, verbatim. Values that are empty or only whitespace are skipped.
fn strings(tag: &Tag, key: ItemKey) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in tag.get_strings(key) {
        if !value.trim().is_empty() && !out.iter().any(|v| v == value) {
            out.push(value.to_string());
        }
    }
    out
}

/// Splits artist values on `;`, the usual separator for several values in one tag
/// (some stores write `ARTIST=Ravel.;Moss Keeper`). The parts are trimmed ("A; B"), and empty
/// ones and repeats are dropped. A value without `;` is kept exactly as written.
///
/// Nothing else is split: "A feat. B", "A x B" or "A & B" stay one name, since those can be
/// a single artist's name. Fixing them is an edit.
pub fn split_artists(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        let parts: Vec<String> = if value.contains(';') {
            value
                .split(';')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect()
        } else {
            vec![value]
        };
        for part in parts {
            if !out.contains(&part) {
                out.push(part);
            }
        }
    }
    out
}

/// Whether a tag key holds an original file's name, in any spelling: ORIGINALFILENAME,
/// ORIGINAL_FILENAME, "Original Filename", origfilename, orig-fname... Case, spaces and
/// punctuation are ignored; what's left must be "orig"/"original" + "filename"/"fname".
/// Also TOFN, ID3v2's "original filename" frame.
///
/// (The same rule is in migration 0013, which applied it to files already stored.)
fn is_original_filename_key(key: &str) -> bool {
    let letters: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if letters == "tofn" {
        return true;
    }
    let Some(rest) = letters.strip_prefix("original").or_else(|| letters.strip_prefix("orig")) else {
        return false;
    };
    rest == "filename" || rest == "fname"
}

/// Looks through all raw tags for an original file's name (see `is_original_filename_key`).
fn find_original_filename(raw: &[RawTag]) -> Option<String> {
    for tag in raw {
        for item in &tag.items {
            let Some(value) = first_text(&item.value) else {
                continue;
            };
            let is_match = match (item.key.as_str(), &item.description) {
                // ID3v2 user-defined frames: key "TXXX", the real name is in the description.
                ("TXXX", Some(desc)) => is_original_filename_key(desc),
                // MP4 freeform atoms: "----:com.apple.iTunes:ORIGINALFILENAME"
                (key, _) if key.starts_with("----:") => {
                    key.rsplit(':').next().is_some_and(is_original_filename_key)
                }
                (key, _) => is_original_filename_key(key),
            };
            if is_match {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Date keys per raw tag format, most preferred first (release date, then recording date,
/// then year). Compared case-insensitively.
const DATE_KEYS: [(&str, &[&str]); 4] = [
    ("vorbis_comments", &["RELEASEDATE", "DATE", "YEAR"]),
    ("id3v2", &["TDRL", "TDRC", "TYER"]),
    ("mp4_ilst", &["©day"]),
    ("ape", &["RELEASEDATE", "YEAR"]),
];

/// Finds the date verbatim in the raw tags, looking at the file's primary tag format first.
/// The first of the `DATE_KEYS` there is, as it is (`resolve_date` builds on it).
pub(crate) fn find_date(raw: &[RawTag], primary_format: Option<&str>) -> Option<String> {
    // Primary format first, then the rest in file order.
    let ordered = raw
        .iter()
        .filter(|t| Some(t.format) == primary_format)
        .chain(raw.iter().filter(|t| Some(t.format) != primary_format));

    for tag in ordered {
        let Some((_, keys)) = DATE_KEYS.iter().find(|(format, _)| *format == tag.format) else {
            continue;
        };
        for wanted in keys.iter() {
            let found = tag
                .items
                .iter()
                .filter(|item| item.key.eq_ignore_ascii_case(wanted))
                .find_map(|item| first_text(&item.value));
            if let Some(date) = found {
                return Some(date.to_string());
            }
        }
    }
    None
}

/// What `resolve_date` found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedDate {
    pub date: Option<String>,
    /// Where it was read from, when it isn't one of the `DATE_KEYS` as it is.
    pub from: Option<String>,
    /// Full dates elsewhere in the tags that are in another year than `date` (e.g.
    /// "ICRD says 2020-08-01"). Not used.
    pub conflicts: Vec<String>,
}

/// The file's release date: the first of the `DATE_KEYS` as it is (`find_date`). Some
/// writers put only the year there and the rest of the date elsewhere, so when that's just
/// a year, the first of these that gives the same year more precisely is taken instead:
/// * TDAT, ID3v2.3's day and month ("DDMM"), with the year: TYER "2021" + TDAT "2512" is
///   2021-12-25. ID3v2.4 merged the two into TDRC; this is the one date that's put
///   together rather than taken as it is. (Some writers put a whole date in TDAT: as it is.)
/// * A RELEASETIME or RELEASEDATE field (an ID3v2 TXXX, a Vorbis comment, an MP4 freeform
///   atom), e.g. "2021-12-25" next to TYER "2021".
/// * A WAV file's RIFF INFO ICRD.
///
/// With none of the `DATE_KEYS` at all: a RELEASETIME/RELEASEDATE field, ICRD, then the
/// original release date (TDOR, TORY, ORIGINALDATE, ORIGINALYEAR), as they are.
pub(crate) fn resolve_date(raw: &[RawTag], primary_format: Option<&str>) -> ResolvedDate {
    let Some(date) = find_date(raw, primary_format) else {
        let from_elsewhere = release_date_fields(raw)
            .chain(raw_items(raw, "riff_info", &["ICRD"]))
            .chain(raw_items(raw, "id3v2", &["TDOR", "TORY"]))
            .chain(raw_items(raw, "vorbis_comments", &["ORIGINALDATE", "ORIGINALYEAR"]))
            .next();
        return match from_elsewhere {
            Some((value, from)) => ResolvedDate { date: Some(value), from: Some(from), conflicts: Vec::new() },
            None => ResolvedDate::default(),
        };
    };
    if !is_year(&date) {
        return ResolvedDate { date: Some(date), from: None, conflicts: Vec::new() };
    }

    let mut candidates: Vec<(String, String)> = Vec::new();
    for (tdat, _) in raw_items(raw, "id3v2", &["TDAT"]) {
        if let Some(composed) = compose_tdat(&date, &tdat) {
            candidates.push((composed, "TYER+TDAT".to_string()));
        } else if date_precision(&tdat) > 1 {
            candidates.push((tdat, "TDAT".to_string()));
        }
    }
    candidates.extend(release_date_fields(raw));
    candidates.extend(raw_items(raw, "riff_info", &["ICRD"]));

    let mut conflicts = Vec::new();
    for (value, from) in candidates.into_iter().filter(|(value, _)| date_precision(value) > 1) {
        if value.starts_with(date.as_str()) {
            return ResolvedDate { date: Some(value), from: Some(from), conflicts };
        }
        conflicts.push(format!("{from} says {value}"));
    }
    ResolvedDate { date: Some(date), from: None, conflicts }
}

/// For checking that TDAT is read the right way round (day first): what `year` and TDAT
/// make, and the first whole date the file has in a RELEASETIME/RELEASEDATE field or ICRD
/// (with where), when it has both.
pub(crate) fn tdat_and_other_date(raw: &[RawTag], year: &str) -> Option<(String, String, String)> {
    let composed = raw_items(raw, "id3v2", &["TDAT"]).find_map(|(tdat, _)| compose_tdat(year, &tdat))?;
    let (other, from) = release_date_fields(raw)
        .chain(raw_items(raw, "riff_info", &["ICRD"]))
        .find(|(value, _)| date_precision(value) == 3)?;
    Some((composed, other, from))
}

/// The day a date with all three parts is (`date_precision` 3), as `YYYY-MM-DD`.
pub(crate) fn day_of(text: &str) -> Option<String> {
    if date_precision(text) < 3 {
        return None;
    }
    let digits: String = text.chars().filter(char::is_ascii_digit).take(8).collect();
    Some(format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8]))
}

/// The non-blank values of these keys (case-insensitive) in the tags of `format`, with
/// the key for messages.
fn raw_items<'a>(raw: &'a [RawTag], format: &'a str, keys: &'a [&'a str]) -> impl Iterator<Item = (String, String)> + 'a {
    raw.iter()
        .filter(move |t| t.format == format)
        .flat_map(|t| &t.items)
        .filter(move |item| keys.iter().any(|k| item.key.eq_ignore_ascii_case(k)))
        .filter_map(|item| Some((first_text(&item.value)?.to_string(), item.key.to_ascii_uppercase())))
}

/// The non-blank values of fields named RELEASETIME or RELEASEDATE (in any spelling, e.g.
/// RELEASE_DATE): ID3v2 TXXX frames by their description, MP4 freeform atoms by their
/// name, other formats by their key. With the field's name for messages.
fn release_date_fields(raw: &[RawTag]) -> impl Iterator<Item = (String, String)> + '_ {
    fn is_release_date_name(name: &str) -> bool {
        let letters: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect();
        letters == "releasetime" || letters == "releasedate"
    }
    raw.iter().filter(|t| t.format != "generic").flat_map(|t| &t.items).filter_map(|item| {
        let name = match (item.key.as_str(), &item.description) {
            ("TXXX", Some(desc)) => format!("TXXX:{desc}"),
            (key, _) if key.starts_with("----:") => key.rsplit(':').next().unwrap_or_default().to_string(),
            (key, _) => key.to_string(),
        };
        let field = name.strip_prefix("TXXX:").unwrap_or(&name);
        if !is_release_date_name(field) {
            return None;
        }
        Some((first_text(&item.value)?.to_string(), name))
    })
}

/// Exactly four digits: a year and nothing else.
fn is_year(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

/// `year` (four digits) with an ID3v2.3 TDAT ("DDMM"), as `YYYY-MM-DD`, if that's a real
/// day.
fn compose_tdat(year: &str, tdat: &str) -> Option<String> {
    if tdat.len() != 4 || !tdat.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (day, month) = (tdat[..2].parse().ok()?, tdat[2..].parse().ok()?);
    chrono::NaiveDate::from_ymd_opt(year.parse().ok()?, month, day)?;
    Some(format!("{year}-{month:02}-{day:02}"))
}

/// How much of a date `text` starts with: 0 = none, 1 = a year, 2 = a month, 3 = a day.
/// The parts may be separated by "-", "/" or "." or not at all ("20211225").
fn date_precision(text: &str) -> u8 {
    let bytes = text.as_bytes();
    let digits = |at: usize, n: usize| bytes.get(at..at + n).is_some_and(|d| d.iter().all(u8::is_ascii_digit));
    if !digits(0, 4) {
        return 0;
    }
    let mut at = 4;
    let mut precision = 1;
    for range in [1..=12u32, 1..=31] {
        if matches!(bytes.get(at), Some(b'-' | b'/' | b'.')) {
            at += 1;
        }
        let in_range = digits(at, 2) && text[at..at + 2].parse::<u32>().is_ok_and(|n| range.contains(&n));
        if !in_range {
            break;
        }
        at += 2;
        precision += 1;
    }
    precision
}

/// The text of a value; for multi-value arrays, the first entry. Blank text counts as none.
fn first_text(value: &serde_json::Value) -> Option<&str> {
    let text = match value {
        serde_json::Value::Array(values) => values.first()?.as_str()?,
        other => other.as_str()?,
    };
    Some(text).filter(|t| !t.trim().is_empty())
}

/// The name used in `RawTag::format` for a lofty tag type.
fn raw_format_name(tag_type: TagType) -> Option<&'static str> {
    match tag_type {
        TagType::VorbisComments => Some("vorbis_comments"),
        TagType::Id3v2 => Some("id3v2"),
        TagType::Mp4Ilst => Some("mp4_ilst"),
        TagType::Ape => Some("ape"),
        _ => None,
    }
}

/// A text value as JSON. Some formats (ID3v2.4) separate multiple values with NUL
/// characters; those become an array, since Postgres can't store NUL in text/jsonb.
/// Trailing NULs are a terminator or padding some writers add (ffmpeg pads COMM frames
/// with hundreds), not values.
fn text_value(text: &str) -> serde_json::Value {
    let text = text.trim_end_matches('\0');
    if text.contains('\0') {
        json!(text.split('\0').collect::<Vec<_>>())
    } else {
        json!(text)
    }
}

/// Removes NUL characters left anywhere in the raw dump, since Postgres can't store them
/// in jsonb. Values were already split on NULs by `text_value`; this catches the fields
/// that weren't, e.g. ffmpeg writes a COMM frame's language as three NULs. A description
/// or language that was only NULs becomes none.
fn strip_nuls(raw: &mut [RawTag]) {
    fn clean(s: &mut String) {
        if s.contains('\0') {
            s.retain(|c| c != '\0');
        }
    }
    fn clean_value(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(s) => clean(s),
            serde_json::Value::Array(values) => values.iter_mut().for_each(clean_value),
            serde_json::Value::Object(map) => {
                // Keys can't be changed in place; rebuild only when one has a NUL.
                if map.keys().any(|k| k.contains('\0')) {
                    *map = std::mem::take(map)
                        .into_iter()
                        .map(|(k, v)| (k.replace('\0', ""), v))
                        .collect();
                }
                map.values_mut().for_each(clean_value);
            }
            _ => {}
        }
    }
    fn clean_field(field: &mut Option<String>) {
        if let Some(s) = field.as_mut().filter(|s| s.contains('\0')) {
            clean(s);
            if s.is_empty() {
                *field = None;
            }
        }
    }
    for item in raw.iter_mut().flat_map(|tag| tag.items.iter_mut()) {
        clean(&mut item.key);
        clean_value(&mut item.value);
        clean_field(&mut item.description);
        clean_field(&mut item.language);
    }
}

/// Reads the format-specific tags. Returns Ok(None) for formats not handled here.
fn read_raw_tags(path: &Path, file_type: FileType) -> anyhow::Result<Option<Vec<RawTag>>> {
    // Properties were already read by the generic pass.
    let options = ParseOptions::new().read_properties(false);
    let mut file = File::open(path)?;
    let mut tags = Vec::new();

    match file_type {
        FileType::Flac => {
            let f = lofty::flac::FlacFile::read_from(&mut file, options)?;
            tags.extend(f.vorbis_comments().map(vorbis_dump));
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
        }
        FileType::Opus => {
            let f = lofty::ogg::OpusFile::read_from(&mut file, options)?;
            tags.push(vorbis_dump(f.vorbis_comments()));
        }
        FileType::Vorbis => {
            let f = lofty::ogg::VorbisFile::read_from(&mut file, options)?;
            tags.push(vorbis_dump(f.vorbis_comments()));
        }
        FileType::Speex => {
            let f = lofty::ogg::SpeexFile::read_from(&mut file, options)?;
            tags.push(vorbis_dump(f.vorbis_comments()));
        }
        FileType::Mpeg => {
            let f = lofty::mpeg::MpegFile::read_from(&mut file, options)?;
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
            tags.extend(f.ape().map(ape_dump));
        }
        FileType::Aac => {
            let f = lofty::aac::AacFile::read_from(&mut file, options)?;
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
        }
        FileType::Mp4 => {
            let f = lofty::mp4::Mp4File::read_from(&mut file, options)?;
            tags.extend(f.ilst().map(ilst_dump));
        }
        FileType::Wav => {
            let f = lofty::iff::wav::WavFile::read_from(&mut file, options)?;
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
            tags.extend(f.riff_info().map(riff_info_dump));
        }
        FileType::Aiff => {
            let f = lofty::iff::aiff::AiffFile::read_from(&mut file, options)?;
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
        }
        FileType::Ape => {
            let f = lofty::ape::ApeFile::read_from(&mut file, options)?;
            tags.extend(f.ape().map(ape_dump));
            tags.extend(f.id3v2().map(|t| id3v2_raw(path, t)));
        }
        FileType::WavPack => {
            let f = lofty::wavpack::WavPackFile::read_from(&mut file, options)?;
            tags.extend(f.ape().map(ape_dump));
        }
        _ => return Ok(None),
    }
    Ok(Some(tags))
}

/// A WAV file's RIFF INFO list (INAM, IART, ICRD...). Many writers fill it alongside an
/// ID3v2 chunk, and some put the whole date only here.
fn riff_info_dump(tag: &RiffInfoList) -> RawTag {
    RawTag {
        format: "riff_info",
        items: tag.into_iter().map(|(key, value)| RawItem::new(key.clone(), text_value(value))).collect(),
    }
}

/// A WAV file's RIFF INFO list as `read_tags` dumps it, for files stored before it was
/// read. Blocking.
pub(crate) fn read_riff_info(path: &Path) -> anyhow::Result<Option<RawTag>> {
    let mut file = File::open(path)?;
    let f = lofty::iff::wav::WavFile::read_from(&mut file, ParseOptions::new().read_properties(false))?;
    Ok(f.riff_info().map(riff_info_dump))
}

/// The raw tags as kept in `files.embedded_tags` (`{"tags": [...]}`), for looking at files
/// already stored. Tags of a format this version doesn't know are left out.
pub(crate) fn raw_from_stored(embedded_tags: &serde_json::Value) -> Vec<RawTag> {
    #[derive(Deserialize)]
    struct Stored {
        format: String,
        #[serde(default)]
        items: Vec<RawItem>,
    }
    let Some(tags) = embedded_tags.get("tags") else {
        return Vec::new();
    };
    let stored: Vec<Stored> = serde_json::from_value(tags.clone()).unwrap_or_default();
    stored
        .into_iter()
        .filter_map(|t| {
            let format = RAW_FORMATS.iter().find(|f| **f == t.format)?;
            Some(RawTag { format, items: t.items })
        })
        .collect()
}

/// The primary tag format a file was read with (see `raw_format_name`), from its container
/// as stored in `files.container` (symphonia's name for it). The same choice lofty's
/// `FileType::primary_tag_type` makes at import.
pub(crate) fn primary_format_of_container(container: &str) -> Option<&'static str> {
    match container {
        "wave" | "aiff" | "mp1" | "mp2" | "mp3" | "adts" | "aac" => Some("id3v2"),
        "flac" | "ogg" => Some("vorbis_comments"),
        "isomp4" => Some("mp4_ilst"),
        _ => None,
    }
}

fn vorbis_dump(tag: &VorbisComments) -> RawTag {
    RawTag {
        format: "vorbis_comments",
        items: tag
            .items()
            .map(|(key, value)| RawItem::new(key, text_value(value)))
            .collect(),
    }
}

/// Dumps the ID3v2 tag with the `id3` crate, which keeps every text frame verbatim.
/// Falls back to lofty's (slightly lossy) view if the `id3` crate can't read it.
fn id3v2_raw(path: &Path, lofty_tag: &Id3v2Tag) -> RawTag {
    // Handles plain ID3v2 at the start of the file as well as WAV/AIFF chunks.
    match id3::Tag::read_from_path(path) {
        Ok(tag) => id3_crate_dump(&tag),
        Err(err) => {
            tracing::warn!("id3 crate couldn't read the ID3v2 tag, using lofty's view: {err}");
            lofty_id3v2_dump(lofty_tag)
        }
    }
}

fn id3_crate_dump(tag: &id3::Tag) -> RawTag {
    use id3::Content;

    let items = tag
        .frames()
        .map(|frame| {
            let id = frame.id().to_string();
            match frame.content() {
                Content::Text(text) => RawItem::new(id, text_value(text)),
                Content::Link(url) => RawItem::new(id, json!(url)),
                Content::ExtendedText(t) => RawItem {
                    description: Some(t.description.clone()),
                    ..RawItem::new(id, text_value(&t.value))
                },
                Content::ExtendedLink(l) => RawItem {
                    description: Some(l.description.clone()),
                    ..RawItem::new(id, json!(l.link))
                },
                Content::Comment(c) => RawItem {
                    description: Some(c.description.clone()).filter(|d| !d.is_empty()),
                    language: Some(c.lang.clone()),
                    ..RawItem::new(id, text_value(&c.text))
                },
                Content::Lyrics(l) => RawItem {
                    description: Some(l.description.clone()).filter(|d| !d.is_empty()),
                    language: Some(l.lang.clone()),
                    ..RawItem::new(id, text_value(&l.text))
                },
                Content::SynchronisedLyrics(l) => RawItem {
                    description: Some(l.description.clone()).filter(|d| !d.is_empty()),
                    language: Some(l.lang.clone()),
                    ..RawItem::new(
                        id,
                        json!({
                            "timestamp_format": format!("{:?}", l.timestamp_format),
                            "content_type": format!("{:?}", l.content_type),
                            "lines": l.content,
                        }),
                    )
                },
                Content::InvolvedPeopleList(list) => {
                    let pairs: Vec<[&str; 2]> = list
                        .items
                        .iter()
                        .map(|i| [i.involvement.as_str(), i.involvee.as_str()])
                        .collect();
                    RawItem::new(id, json!(pairs))
                }
                Content::Picture(p) => {
                    RawItem::new(id, json!(format!("<picture, {} bytes>", p.data.len())))
                }
                Content::Unknown(u) => {
                    RawItem::new(id, json!({ "type": "binary", "hex": hex::encode(&u.data) }))
                }
                _ => RawItem::new(id, json!("<frame not decoded>")),
            }
        })
        .collect();
    RawTag {
        format: "id3v2",
        items,
    }
}

fn lofty_id3v2_dump(tag: &Id3v2Tag) -> RawTag {
    let items = tag
        .into_iter()
        .map(|frame| {
            let id = frame.id_str().to_string();
            match frame {
                Frame::Text(f) => RawItem::new(id, text_value(&f.value)),
                Frame::UserText(f) => RawItem {
                    description: Some(f.description.to_string()),
                    ..RawItem::new(id, json!(f.content))
                },
                Frame::Comment(f) => RawItem {
                    description: Some(f.description.to_string()).filter(|d| !d.is_empty()),
                    language: Some(String::from_utf8_lossy(&f.language).to_string()),
                    ..RawItem::new(id, json!(f.content))
                },
                Frame::UnsynchronizedText(f) => RawItem {
                    description: Some(f.description.to_string()).filter(|d| !d.is_empty()),
                    language: Some(String::from_utf8_lossy(&f.language).to_string()),
                    ..RawItem::new(id, json!(f.content))
                },
                Frame::Url(f) => RawItem::new(id, json!(f.url())),
                Frame::UserUrl(f) => RawItem {
                    description: Some(f.description.to_string()),
                    ..RawItem::new(id, json!(f.content))
                },
                // lofty parses timestamp frames (TDRC etc.); this renders it back as text.
                Frame::Timestamp(f) => RawItem::new(id, json!(f.timestamp.to_string())),
                Frame::KeyValue(f) => {
                    let pairs: Vec<[&str; 2]> = f
                        .key_value_pairs
                        .iter()
                        .map(|(k, v)| [k.as_ref(), v.as_ref()])
                        .collect();
                    RawItem::new(id, json!(pairs))
                }
                Frame::Picture(f) => RawItem::new(
                    id,
                    json!(format!("<picture, {} bytes>", f.picture.data().len())),
                ),
                _ => RawItem::new(id, json!("<frame not decoded>")),
            }
        })
        .collect();
    RawTag {
        format: "id3v2",
        items,
    }
}

fn ilst_dump(tag: &Ilst) -> RawTag {
    let mut items = Vec::new();
    for atom in tag {
        let key = match atom.ident() {
            AtomIdent::Fourcc(code) => fourcc_to_string(code),
            AtomIdent::Freeform { mean, name } => format!("----:{mean}:{name}"),
        };
        for data in atom.data() {
            let value = match data {
                AtomData::UTF8(s) | AtomData::UTF16(s) => json!(s),
                AtomData::SignedInteger(n) => json!(n),
                AtomData::UnsignedInteger(n) => json!(n),
                AtomData::Bool(b) => json!(b),
                AtomData::Picture(p) => json!(format!("<picture, {} bytes>", p.data().len())),
                // e.g. track/disc numbers ("trkn", "disk") are binary.
                AtomData::Unknown { code, data } => {
                    json!({ "type": format!("{code:?}"), "hex": hex::encode(data) })
                }
            };
            items.push(RawItem::new(key.clone(), value));
        }
    }
    RawTag {
        format: "mp4_ilst",
        items,
    }
}

/// MP4 atom names are 4 bytes of Latin-1, e.g. [0xA9, b'n', b'a', b'm'] is "©nam".
fn fourcc_to_string(code: &[u8; 4]) -> String {
    code.iter().map(|&b| b as char).collect()
}

fn ape_dump(tag: &lofty::ape::ApeTag) -> RawTag {
    RawTag {
        format: "ape",
        items: tag
            .into_iter()
            .map(|item| RawItem::new(item.key(), item_value_json(item.value())))
            .collect(),
    }
}

/// For formats without a specific reader. Only includes keys lofty knows.
fn generic_dump(tag: &Tag) -> RawTag {
    let tag_type = tag.tag_type();
    RawTag {
        format: "generic",
        items: tag
            .items()
            .map(|item| {
                let key = item
                    .key()
                    .map_key(tag_type)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{:?}", item.key()));
                RawItem {
                    description: Some(item.description().to_string()).filter(|d| !d.is_empty()),
                    ..RawItem::new(key, item_value_json(item.value()))
                }
            })
            .collect(),
    }
}

fn item_value_json(value: &ItemValue) -> serde_json::Value {
    match value {
        ItemValue::Text(s) | ItemValue::Locator(s) => text_value(s),
        ItemValue::Binary(b) => json!({ "type": "binary", "hex": hex::encode(b) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(format: &'static str, items: Vec<RawItem>) -> RawTag {
        RawTag { format, items }
    }

    #[test]
    fn artists_are_split_on_semicolons() {
        let split = |values: &[&str]| split_artists(values.iter().map(|v| v.to_string()).collect());
        assert_eq!(split(&["Ravel.;Moss Keeper"]), ["Ravel.", "Moss Keeper"]);
        assert_eq!(split(&["VANTA; KR8; "]), ["VANTA", "KR8"]);
        // Repeats (also across values) are dropped; other separators aren't split.
        assert_eq!(split(&["A;A", "A", "B feat. C"]), ["A", "B feat. C"]);
        // Without `;`, verbatim (not even trimmed).
        assert_eq!(split(&[" Odd Spacing "]), [" Odd Spacing "]);
        assert!(split(&[";", " ; "]).is_empty());
    }

    #[test]
    fn original_filename_from_vorbis() {
        let raw = vec![tag(
            "vorbis_comments",
            vec![
                RawItem::new("TITLE", json!("Song")),
                RawItem::new("originalfilename", json!("Song (master v2).wav")),
            ],
        )];
        assert_eq!(
            find_original_filename(&raw),
            Some("Song (master v2).wav".into())
        );
    }

    #[test]
    fn original_filename_from_id3() {
        let txxx = RawItem {
            description: Some("ORIGINALFILENAME".into()),
            ..RawItem::new("TXXX", json!("a.wav"))
        };
        let raw = vec![tag("id3v2", vec![txxx])];
        assert_eq!(
            find_original_filename(&raw),
            Some("a.wav".into())
        );

        let raw = vec![tag("id3v2", vec![RawItem::new("TOFN", json!("b.wav"))])];
        assert_eq!(
            find_original_filename(&raw),
            Some("b.wav".into())
        );
    }

    #[test]
    fn original_filename_from_mp4_freeform() {
        let key = "----:com.apple.iTunes:ORIGINALFILENAME";
        let raw = vec![tag("mp4_ilst", vec![RawItem::new(key, json!("c.wav"))])];
        assert_eq!(
            find_original_filename(&raw),
            Some("c.wav".into())
        );
    }

    #[test]
    fn original_filename_keys_in_any_spelling() {
        for key in [
            "ORIGINALFILENAME", "ORIGINAL_FILENAME", "Original Filename", "origfilename",
            "ORIG_FILENAME", "orig-fname", "original.file.name", "TOFN",
        ] {
            assert!(is_original_filename_key(key), "{key}");
        }
        for key in ["FILENAME", "ORIGINALNAME", "ORIGARTIST", "originalfilenames", "TOF"] {
            assert!(!is_original_filename_key(key), "{key}");
        }
    }

    #[test]
    fn original_filename_ignores_empty_and_unrelated() {
        let raw = vec![tag(
            "vorbis_comments",
            vec![
                RawItem::new("ORIGINALFILENAME", json!("  ")),
                RawItem::new("FILENAME_NOTE", json!("x")),
            ],
        )];
        assert_eq!(find_original_filename(&raw), None);
    }

    #[test]
    fn dates_are_verbatim_and_prefer_primary_format() {
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TDRC", json!("2001"))]),
            tag(
                "vorbis_comments",
                vec![
                    RawItem::new("date", json!("2019-05-12T11:11:11Z")),
                    RawItem::new("YEAR", json!("2019")),
                ],
            ),
        ];
        assert_eq!(
            find_date(&raw, Some("vorbis_comments")).as_deref(),
            Some("2019-05-12T11:11:11Z")
        );
        assert_eq!(find_date(&raw, Some("id3v2")).as_deref(), Some("2001"));

        // Release date beats recording date.
        let raw = vec![tag(
            "id3v2",
            vec![
                RawItem::new("TDRC", json!("2018-01-01 10:00")),
                RawItem::new("TDRL", json!("2019-05-12T11:11:11Z")),
            ],
        )];
        assert_eq!(
            find_date(&raw, Some("id3v2")).as_deref(),
            Some("2019-05-12T11:11:11Z")
        );
    }

    fn txxx(description: &str, value: &str) -> RawItem {
        RawItem { description: Some(description.into()), ..RawItem::new("TXXX", json!(value)) }
    }

    fn resolved(raw: &[RawTag]) -> (Option<String>, Option<String>) {
        let r = resolve_date(raw, Some("id3v2"));
        (r.date, r.from)
    }

    #[test]
    fn a_year_is_completed_from_tdat() {
        let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2021")), RawItem::new("TDAT", json!("2512"))])];
        assert_eq!(resolved(&raw), (Some("2021-12-25".into()), Some("TYER+TDAT".into())));

        // DDMM that isn't a day (31 February, month 13) isn't used.
        for tdat in ["3102", "0113", "25", "x512"] {
            let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2021")), RawItem::new("TDAT", json!(tdat))])];
            assert_eq!(resolved(&raw), (Some("2021".into()), None), "{tdat}");
        }
        // 29 February only in leap years.
        let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2020")), RawItem::new("TDAT", json!("2902"))])];
        assert_eq!(resolved(&raw).0.as_deref(), Some("2020-02-29"));

        // A whole date in TDAT is taken as it is.
        let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2021")), RawItem::new("TDAT", json!("2021-12-25"))])];
        assert_eq!(resolved(&raw), (Some("2021-12-25".into()), Some("TDAT".into())));
    }

    #[test]
    fn a_year_is_completed_from_release_time_fields_and_icrd() {
        let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2021")), txxx("RELEASETIME", "2021-11-01T10:15:37Z")])];
        assert_eq!(resolved(&raw), (Some("2021-11-01T10:15:37Z".into()), Some("TXXX:RELEASETIME".into())));
        let raw = vec![tag("id3v2", vec![RawItem::new("TDRC", json!("2021")), txxx("release_date", "2021-11")])];
        assert_eq!(resolved(&raw), (Some("2021-11".into()), Some("TXXX:release_date".into())));

        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2020"))]),
            tag("riff_info", vec![RawItem::new("INAM", json!("Song")), RawItem::new("ICRD", json!("20200801"))]),
        ];
        assert_eq!(resolved(&raw), (Some("20200801".into()), Some("ICRD".into())));

        // A year-only ICRD adds nothing.
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2019"))]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2019"))]),
        ];
        assert_eq!(resolved(&raw), (Some("2019".into()), None));

        // TDAT first, then the release time fields, then ICRD.
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2021")), txxx("RELEASETIME", "2021-12-24"), RawItem::new("TDAT", json!("2512"))]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2021-12-23"))]),
        ];
        assert_eq!(resolved(&raw).0.as_deref(), Some("2021-12-25"));
    }

    #[test]
    fn dates_of_another_year_are_not_used() {
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2021")), txxx("RELEASETIME", "2020-08-01")]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2021-03-04"))]),
        ];
        let r = resolve_date(&raw, Some("id3v2"));
        assert_eq!(r.date.as_deref(), Some("2021-03-04"));
        assert_eq!(r.conflicts, ["TXXX:RELEASETIME says 2020-08-01"]);

        let raw = vec![tag("id3v2", vec![RawItem::new("TYER", json!("2021")), txxx("RELEASETIME", "2020-08-01")])];
        let r = resolve_date(&raw, Some("id3v2"));
        assert_eq!((r.date.as_deref(), r.from), (Some("2021"), None));
        assert_eq!(r.conflicts.len(), 1);
    }

    #[test]
    fn dates_that_are_more_than_a_year_are_kept_as_they_are() {
        // A whole date in TYER (where ID3v2.3 wants four digits) is still the date.
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2024-11-15T18:01:53.000Z")), RawItem::new("TDAT", json!("1511"))]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2024-11-16"))]),
        ];
        assert_eq!(resolved(&raw), (Some("2024-11-15T18:01:53.000Z".into()), None));
    }

    #[test]
    fn without_a_date_tag_other_fields_are_used() {
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TIT2", json!("Song"))]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2019"))]),
        ];
        assert_eq!(resolved(&raw), (Some("2019".into()), Some("ICRD".into())));

        let raw = vec![tag("id3v2", vec![RawItem::new("TORY", json!("1999")), txxx("RELEASEDATE", "2001-02-03")])];
        assert_eq!(resolved(&raw), (Some("2001-02-03".into()), Some("TXXX:RELEASEDATE".into())));

        let raw = vec![tag("vorbis_comments", vec![RawItem::new("originaldate", json!("1998-07"))])];
        assert_eq!(
            resolve_date(&raw, Some("vorbis_comments")),
            ResolvedDate { date: Some("1998-07".into()), from: Some("ORIGINALDATE".into()), conflicts: Vec::new() }
        );

        assert_eq!(resolved(&[tag("id3v2", vec![RawItem::new("TDAT", json!("2512"))])]), (None, None));
    }

    #[test]
    fn date_precision_counts_the_parts() {
        for (text, precision) in [
            ("2021", 1), ("2021-12", 2), ("2021-12-25", 3), ("2021-12-25T10:00:00Z", 3), ("20211225", 3),
            ("2021/12/25", 3), ("2021.12", 2), ("2021-13-01", 1), ("2021-12-32", 2), ("21-12-25", 0), ("", 0),
        ] {
            assert_eq!(super::date_precision(text), precision, "{text}");
        }
    }

    #[test]
    fn tdat_is_checked_against_other_dates() {
        let raw = vec![
            tag("id3v2", vec![RawItem::new("TYER", json!("2021")), RawItem::new("TDAT", json!("0302")), txxx("RELEASETIME", "2021-03-02T00:00:00Z")]),
        ];
        let (composed, other, from) = tdat_and_other_date(&raw, "2021").unwrap();
        assert_eq!((composed.as_str(), from.as_str()), ("2021-02-03", "TXXX:RELEASETIME"));
        assert_eq!(day_of(&other).as_deref(), Some("2021-03-02"));
        assert_eq!(day_of("20210302").as_deref(), Some("2021-03-02"));
        assert_eq!(day_of("2021-03"), None);
        assert!(tdat_and_other_date(&raw[..0], "2021").is_none());
    }

    #[test]
    fn stored_tags_read_back() {
        let raw = vec![
            tag("id3v2", vec![txxx("RELEASETIME", "2021-12-25"), RawItem::new("TYER", json!("2021"))]),
            tag("riff_info", vec![RawItem::new("ICRD", json!("2021"))]),
        ];
        let mut stored = json!({ "tags": raw });
        stored["tags"].as_array_mut().unwrap().push(json!({ "format": "some_future_format", "items": [] }));
        let back = raw_from_stored(&stored);
        assert_eq!(back.iter().map(|t| t.format).collect::<Vec<_>>(), ["id3v2", "riff_info"]);
        assert_eq!(back[0].items[0].description.as_deref(), Some("RELEASETIME"));
        assert_eq!(resolved(&back).0.as_deref(), Some("2021-12-25"));
        assert!(raw_from_stored(&json!({})).is_empty());
    }

    #[test]
    fn nul_separated_values_become_arrays() {
        assert_eq!(text_value("A\0B"), json!(["A", "B"]));
        assert_eq!(text_value("a.wav\0"), json!("a.wav"));
        assert_eq!(text_value("A\0B\0\0\0"), json!(["A", "B"]));
        assert_eq!(text_value("A, B"), json!("A, B"));
    }

    #[test]
    fn nuls_are_stripped_from_the_raw_dump() {
        // As ffmpeg writes a COMM frame: language is three NULs.
        let comm = RawItem {
            language: Some("\0\0\0".into()),
            ..RawItem::new("COMM", json!("hi"))
        };
        let txxx = RawItem {
            description: Some(String::new()),
            ..RawItem::new("TXXX", json!({ "k\0": ["a\0b"] }))
        };
        let mut raw = vec![tag("id3v2", vec![comm, txxx])];
        strip_nuls(&mut raw);
        assert_eq!(raw[0].items[0].language, None);
        // An empty description that had no NULs is kept as it was.
        assert_eq!(raw[0].items[1].description.as_deref(), Some(""));
        assert_eq!(raw[0].items[1].value, json!({ "k": ["ab"] }));
    }

    #[test]
    fn fourcc() {
        assert_eq!(fourcc_to_string(&[0xA9, b'n', b'a', b'm']), "©nam");
        assert_eq!(fourcc_to_string(b"trkn"), "trkn");
    }
}
