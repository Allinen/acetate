//! A catalog kept on disk the way it is in memory, and opened by mapping the file: no
//! parsing, no sorting, no indexing at start, and the system can drop the pages and read
//! them again instead of the app being killed when memory is short.
//!
//! One file per catalog, written whole to a temporary name and renamed into place, never
//! changed afterwards. It starts with a header (magic, format, a fingerprint of the
//! layouts of what's stored, the file's length) and a table of sections; each section is
//! one column of plain data (rows, text, lists, ranks…) at an offset divisible by 16, so
//! it can be read in place. What isn't plain (the artists, playlists, annotations…: small)
//! is JSON in the first section.
//!
//! The file is the same on the machine that wrote it only: it's the machine's own cache,
//! and any difference (a new format, another build's layouts) just means it isn't used and
//! the server's catalog is fetched again.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::flat::{Col, Flat, Mapping};
use super::index::{Catalog, Lists};
use super::model::{self, Credit, Membership, Role};
use super::rows::{AlbumRow, Pools, Span, TrackRow};
use super::text::{Str, Text, Texts, Utf8};

const MAGIC: &[u8; 8] = b"ACETSNAP";

/// Changes with what's stored and how.
const FORMAT: u32 = 2;

/// A credit as stored: its alias (if any) is in the aliases section.
#[derive(Clone, Copy)]
#[repr(C)]
struct CreditRow {
    id: Uuid,
    alias_start: u32,
    /// `u32::MAX`: no alias.
    alias_len: u32,
    role: u8,
    _pad: [u8; 3],
}

// SAFETY: `repr(C)`, no padding left (spelled out), any bytes are valid.
unsafe impl Flat for CreditRow {}

/// A membership as stored (`Option<i16>`s have no fixed bytes): `i16::MIN` is none.
#[derive(Clone, Copy)]
#[repr(C)]
struct MembershipRow {
    album: Uuid,
    disc: i16,
    number: i16,
}

// SAFETY: `repr(C)`, 20 bytes, no padding, any bytes are valid.
unsafe impl Flat for MembershipRow {}

fn role_byte(role: Role) -> u8 {
    match role {
        Role::Primary => 0,
        Role::Featured => 1,
        Role::Producer => 2,
        Role::Other => 3,
    }
}

fn role_of(byte: u8) -> Role {
    match byte {
        0 => Role::Primary,
        1 => Role::Featured,
        2 => Role::Producer,
        _ => Role::Other,
    }
}

/// What isn't plain data.
#[derive(Serialize, Deserialize)]
struct Meta {
    version: Option<String>,
    etag: Option<String>,
    artists: Vec<model::Artist>,
    playlists: Vec<model::Playlist>,
    annotations: Vec<model::Annotation>,
    issues: Vec<model::Issues>,
    groupings: Vec<model::Grouping>,
    extras: Vec<model::Extra>,
}

/// Fingerprint of the stored layouts: their sizes and alignments, and where the fields of
/// a probe row are. A different compiler or target may give a different one.
fn fingerprint() -> u64 {
    use std::mem::{align_of, size_of};
    let mut bytes = Vec::new();
    for n in [
        size_of::<TrackRow>(), align_of::<TrackRow>(), size_of::<AlbumRow>(), align_of::<AlbumRow>(),
        size_of::<MembershipRow>(), align_of::<MembershipRow>(), size_of::<CreditRow>(), align_of::<CreditRow>(),
        size_of::<Span>(), size_of::<Str>(),
    ] {
        bytes.extend((n as u64).to_le_bytes());
    }
    let probe = MembershipRow { album: Uuid::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10), disc: 0x1234, number: 0x5678 };
    bytes.extend(Col::from(vec![probe]).as_bytes());
    // FNV-1a.
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3))
}

const HEADER: usize = 8 + 4 + 8 + 8 + 4;

// ---- Writing ----

struct Writer<W: Write> {
    out: W,
    at: u64,
    /// (offset, length in bytes) of each section so far.
    table: Vec<(u64, u64)>,
}

impl<W: Write> Writer<W> {
    fn section(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let pad = (16 - self.at % 16) % 16;
        self.out.write_all(&[0u8; 16][..pad as usize])?;
        self.at += pad;
        self.table.push((self.at, bytes.len() as u64));
        self.out.write_all(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }

    fn col<T: Flat>(&mut self, col: &Col<T>) -> std::io::Result<()> {
        self.section(col.as_bytes())
    }

    fn lists(&mut self, l: &Lists) -> std::io::Result<()> {
        self.col(&l.starts)?;
        self.col(&l.items)
    }

    fn texts(&mut self, t: &Texts) -> std::io::Result<()> {
        self.col(&t.ends)?;
        self.col(&t.buffer.0)
    }
}

/// The sections, in order. Reading mirrors this.
const SECTIONS: usize = 34;

impl Catalog {
    /// Writes this catalog to `path` (a temporary name; the caller renames it into place).
    pub fn write_snapshot(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let meta = Meta {
            version: self.version.clone(),
            etag: self.etag.clone(),
            artists: self.artists.clone(),
            playlists: self.playlists.clone(),
            annotations: self.annotations.values().cloned().collect(),
            issues: self.issues.iter().map(|(t, f)| model::Issues(*t, f.clone())).collect(),
            groupings: self.groupings.clone(),
            extras: self.extras.clone(),
        };
        let mut aliases = Vec::new();
        let credits: Vec<CreditRow> = self
            .pools
            .credits
            .iter()
            .map(|Credit(id, role, alias)| {
                let (alias_start, alias_len) = match alias {
                    Some(a) => {
                        let start = aliases.len() as u32;
                        aliases.extend_from_slice(a.as_bytes());
                        (start, a.len() as u32)
                    }
                    None => (0, u32::MAX),
                };
                CreditRow { id: *id, alias_start, alias_len, role: role_byte(*role), _pad: [0; 3] }
            })
            .collect();

        // The header and table come first but need the offsets: written as zeros now and
        // again at the end.
        let table_len = SECTIONS * 16;
        let mut w = Writer { out: std::io::BufWriter::with_capacity(1 << 20, file), at: 0, table: Vec::new() };
        w.out.write_all(&vec![0u8; HEADER + table_len])?;
        w.at = (HEADER + table_len) as u64;

        w.section(&serde_json::to_vec(&meta)?)?;
        w.col(&self.tracks)?;
        w.col(&self.albums)?;
        w.col(&self.pools.text.buffer.0)?;
        let none = |n: Option<i16>| n.unwrap_or(i16::MIN);
        let memberships: Vec<MembershipRow> =
            self.pools.memberships.iter().map(|m| MembershipRow { album: m.0, disc: none(m.1), number: none(m.2) }).collect();
        w.col(&Col::from(memberships))?;
        w.col(&self.pools.ids)?;
        w.col(&self.pools.strs)?;
        w.col(&Col::from(credits))?;
        w.section(&aliases)?;
        for lists in [&self.album_tracks, &self.artist_tracks, &self.artist_albums, &self.grouping_tracks, &self.grouping_albums] {
            w.lists(lists)?;
        }
        for texts in [&self.track_text, &self.album_text, &self.artist_text, &self.extra_text] {
            w.texts(texts)?;
        }
        for rank in [
            &self.track_title_rank, &self.track_artist_rank, &self.album_title_rank, &self.album_artist_rank,
            &self.artist_name_rank, &self.track_newest_rank, &self.track_album_rank,
        ] {
            w.col(rank)?;
        }
        anyhow::ensure!(w.table.len() == SECTIONS, "the snapshot has {} sections, not {SECTIONS}", w.table.len());

        let mut head = Vec::with_capacity(HEADER + table_len);
        head.extend(MAGIC);
        head.extend(FORMAT.to_le_bytes());
        head.extend(fingerprint().to_le_bytes());
        head.extend(w.at.to_le_bytes());
        head.extend((SECTIONS as u32).to_le_bytes());
        for (offset, len) in &w.table {
            head.extend(offset.to_le_bytes());
            head.extend(len.to_le_bytes());
        }
        let mut file = w.out.into_inner().map_err(|e| e.into_error())?;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0))?;
        file.write_all(&head)?;
        file.sync_all()?;
        Ok(())
    }

    /// Opens a snapshot by mapping it. An error means the file isn't one this program can
    /// use (damaged, another format or build): fetch the catalog again.
    pub fn open_snapshot(path: &Path) -> anyhow::Result<Catalog> {
        let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        // SAFETY: the file is written once, to a temporary name, and renamed into place;
        // nothing changes it while it's mapped.
        let map = unsafe { memmap2::Mmap::map(&file)? };
        let mapping = Arc::new(Mapping { map });
        let bytes: &[u8] = &mapping.map;

        anyhow::ensure!(bytes.len() >= HEADER && &bytes[..8] == MAGIC, "not a snapshot");
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        if u32_at(8) != FORMAT {
            bail!("a snapshot of another format");
        }
        if u64_at(12) != fingerprint() {
            bail!("a snapshot with other layouts");
        }
        anyhow::ensure!(u64_at(20) == bytes.len() as u64, "a snapshot cut short");
        anyhow::ensure!(u32_at(28) as usize == SECTIONS, "a snapshot with other sections");
        anyhow::ensure!(bytes.len() >= HEADER + SECTIONS * 16, "a snapshot cut short");

        let mut next = 0usize;
        let mut section = |size: usize, align: usize| -> anyhow::Result<(usize, usize)> {
            let at = HEADER + next * 16;
            next += 1;
            let (offset, len) = (u64_at(at) as usize, u64_at(at + 8) as usize);
            anyhow::ensure!(offset % align.max(1) == 0 && offset.checked_add(len).is_some_and(|end| end <= bytes.len()), "a section out of the file");
            anyhow::ensure!(len % size == 0, "a section of the wrong size");
            Ok((offset, len / size))
        };
        fn col<T: Flat>(mapping: &Arc<Mapping>, (offset, len): (usize, usize)) -> Col<T> {
            Col::mapped(mapping, offset, len)
        }
        macro_rules! col {
            ($t:ty) => {{
                let s = section(std::mem::size_of::<$t>(), std::mem::align_of::<$t>())?;
                col::<$t>(&mapping, s)
            }};
        }

        let (meta_offset, meta_len) = section(1, 1)?;
        let meta: Meta = serde_json::from_slice(&bytes[meta_offset..meta_offset + meta_len]).context("the snapshot's details")?;
        let tracks = col!(TrackRow);
        let albums = col!(AlbumRow);
        let text = Text { buffer: Utf8::checked(col!(u8)).context("text that isn't text")? };
        let memberships = col!(MembershipRow);
        let some = |n: i16| (n != i16::MIN).then_some(n);
        let memberships = memberships.iter().map(|m| Membership(m.album, some(m.disc), some(m.number))).collect();
        let ids = col!(Uuid);
        let strs = col!(Str);
        let credit_rows = col!(CreditRow);
        let (alias_offset, alias_len) = section(1, 1)?;
        let alias_bytes = &bytes[alias_offset..alias_offset + alias_len];
        let credits = credit_rows
            .iter()
            .map(|c| {
                let alias = match c.alias_len {
                    u32::MAX => None,
                    len => {
                        let s = alias_bytes.get(c.alias_start as usize..c.alias_start as usize + len as usize).context("an alias out of its section")?;
                        Some(std::str::from_utf8(s)?.into())
                    }
                };
                Ok(Credit(c.id, role_of(c.role), alias))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut lists = || -> anyhow::Result<Lists> {
            let starts = col!(u32);
            let items = col!(u32);
            Ok(Lists { starts, items })
        };
        let (album_tracks, artist_tracks, artist_albums, grouping_tracks, grouping_albums) = (lists()?, lists()?, lists()?, lists()?, lists()?);
        let mut texts = || -> anyhow::Result<Texts> {
            let ends = col!(u32);
            let buffer = Utf8::checked(col!(u8)).context("text that isn't text")?;
            Ok(Texts { buffer, ends })
        };
        let (track_text, album_text, artist_text, extra_text) = (texts()?, texts()?, texts()?, texts()?);
        let (track_title_rank, track_artist_rank, album_title_rank, album_artist_rank, artist_name_rank, track_newest_rank, track_album_rank) =
            (col!(u32), col!(u32), col!(u32), col!(u32), col!(u32), col!(u32), col!(u32));

        // What the columns have to agree on, or indexing would run off their ends.
        let (n_tracks, n_albums, n_artists, n_groupings) = (tracks.len(), albums.len(), meta.artists.len(), meta.groupings.len());
        let ok = album_tracks.starts.len() == n_albums + 1
            && artist_tracks.starts.len() == n_artists + 1
            && artist_albums.starts.len() == n_artists + 1
            && grouping_tracks.starts.len() == n_groupings + 1
            && grouping_albums.starts.len() == n_groupings + 1
            && track_text.ends.len() == n_tracks
            && album_text.ends.len() == n_albums
            && artist_text.ends.len() == n_artists
            && extra_text.ends.len() == meta.extras.len()
            && [&track_title_rank, &track_artist_rank, &track_newest_rank, &track_album_rank].iter().all(|r| r.len() == n_tracks)
            && [&album_title_rank, &album_artist_rank].iter().all(|r| r.len() == n_albums)
            && artist_name_rank.len() == n_artists;
        anyhow::ensure!(ok, "a snapshot whose columns don't agree");

        let mut catalog = Catalog {
            version: meta.version,
            etag: meta.etag,
            pools: Pools { text, credits, memberships, ids, strs },
            tracks,
            albums,
            artists: meta.artists,
            playlists: meta.playlists,
            annotations: meta.annotations.into_iter().map(|a| (a.id, a)).collect(),
            issues: meta.issues.into_iter().map(|i| (i.0, i.1)).collect(),
            groupings: meta.groupings,
            extras: meta.extras,
            extra_text,
            track_by_id: Default::default(),
            album_by_id: Default::default(),
            artist_by_id: Default::default(),
            grouping_by_id: Default::default(),
            album_tracks,
            artist_tracks,
            artist_albums,
            grouping_tracks,
            grouping_albums,
            track_text,
            album_text,
            artist_text,
            track_title_rank,
            track_artist_rank,
            album_title_rank,
            album_artist_rank,
            artist_name_rank,
            track_newest_rank,
            track_album_rank,
        };
        let (tracks, albums, artists, groupings) = catalog.id_maps();
        catalog.track_by_id = tracks;
        catalog.album_by_id = albums;
        catalog.artist_by_id = artists;
        catalog.grouping_by_id = groupings;
        Ok(catalog)
    }

    /// Whether the big columns are read from a mapped file (not held in memory).
    pub fn is_mapped(&self) -> bool {
        self.tracks.is_mapped()
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::sample;
    use super::super::index::{AlbumSort, ArtistSort, TrackSort};
    use super::*;

    fn snapshot_of(c: &Catalog, dir: &Path) -> Catalog {
        let path = dir.join("c.snap");
        c.write_snapshot(&path).unwrap();
        Catalog::open_snapshot(&path).unwrap()
    }

    /// Everything either catalog can be asked, as text.
    fn describe(c: &Catalog) -> Vec<String> {
        let mut out = vec![format!("{:?} {:?}", c.version, c.etag)];
        for t in c.tracks() {
            out.push(format!(
                "{} {} {} {:?} {:?} {:?} {} {:?} {:?} {:?} {} {:?} {} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                t.id(), t.title(), t.artist_display(), t.credits(), t.albums(), t.duration_ms(), t.file_id(), t.cover_id(), t.cover(),
                t.release_date(), t.status(), t.year(), t.is_restricted(), t.codec(), t.groupings(),
                t.genres().collect::<Vec<_>>(), t.platforms().collect::<Vec<_>>(), t.original_filename(), t.bitrate(), t.sample_rate()
            ));
        }
        for a in c.albums() {
            out.push(format!("{} {} {} {} {:?} {:?} {:?} {:?} {} {}", a.id(), a.title(), a.kind(), a.artist_display(), a.credits(), a.release_date(), a.year(), a.cover(), a.track_count(), a.duration_ms()));
        }
        for filter in ["", "ann", "bo", "artist:bo", "third", "\u{e4}\u{f6}", "zzz"] {
            for sort in [AlbumSort::Added, AlbumSort::Title, AlbumSort::Artist, AlbumSort::Year] {
                out.push(format!("albums {filter:?} {sort:?} {:?}", c.album_list(filter, sort)));
            }
            for sort in [ArtistSort::Name, ArtistSort::Tracks] {
                out.push(format!("artists {filter:?} {sort:?} {:?}", c.artist_list(filter, sort)));
            }
            for sort in [TrackSort::Added, TrackSort::Title, TrackSort::Artist, TrackSort::Album, TrackSort::Year, TrackSort::Length, TrackSort::Plays, TrackSort::Og] {
                for descending in [false, true] {
                    out.push(format!("tracks {filter:?} {sort:?} {descending} {:?}", c.track_list(filter, sort, descending)));
                }
            }
        }
        for a in &c.artists {
            out.push(format!("{} {} {:?} {:?} {:?}", a.label(), a.track_count, c.artist_tracks(a.id), c.artist_albums(a.id), c.artist(a.id).map(|a| &a.aliases)));
        }
        for a in c.albums() {
            out.push(format!("album {:?} {:?}", c.album_tracks(a.id()), c.album_index(a.id())));
        }
        for g in &c.groupings {
            out.push(format!("grouping {} {:?} {:?}", g.name, c.grouping_tracks(g.id), c.grouping_albums(g.id)));
        }
        let (t, a, ar) = c.texts();
        out.push(format!("{:?} {:?} {:?} {:?}", t.iter().collect::<Vec<_>>(), a.iter().collect::<Vec<_>>(), ar.iter().collect::<Vec<_>>(), c.extra_texts().iter().collect::<Vec<_>>()));
        let mut notes: Vec<_> = c.annotations.values().map(|a| format!("{a:?}")).collect();
        notes.sort();
        out.extend(notes);
        out.push(format!("{:?} {:?} {:?}", c.issues, c.playlists, c.extras));
        out
    }

    #[test]
    fn a_snapshot_is_the_catalog_it_was_made_from() {
        let mut owned = sample();
        owned.etag = Some("\"abc\"".into());
        let dir = tempfile::tempdir().unwrap();
        let mapped = snapshot_of(&owned, dir.path());
        assert!(mapped.is_mapped() && !owned.is_mapped());
        assert_eq!(describe(&owned), describe(&mapped));
        assert_eq!(mapped.track_count(), 3);
        // The alias survived.
        assert_eq!(mapped.album_at(1).credits()[0].2.as_deref(), Some("B.O."));
    }

    #[test]
    fn a_delta_applies_to_a_mapped_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let mapped = snapshot_of(&sample(), dir.path());
        // The sample's shape with no rows: a delta that only removes a track.
        let mut delta: serde_json::Value = serde_json::from_str(&super::super::fixture::sample_json()).unwrap();
        for part in ["tracks", "albums", "artists", "annotations", "issues", "groupings", "extras"] {
            delta[part]["rows"] = serde_json::json!([]);
        }
        delta["playlists"] = serde_json::json!([]);
        delta["version"] = "8:8:".into();
        delta["delta"] = true.into();
        delta["removed"] = serde_json::json!({"tracks": ["00000000-0000-7000-8000-000000000003"]});
        let applied = mapped.apply(model::parse(delta.to_string().as_bytes()).unwrap());
        assert_eq!(applied.track_count(), 2);
        assert_eq!(applied.version.as_deref(), Some("8:8:"));
        // And the result can be kept and opened again.
        assert_eq!(describe(&snapshot_of(&applied, dir.path())), describe(&applied));
    }

    #[test]
    fn a_file_that_is_not_a_snapshot_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.snap");
        c_refused(&path, b"");
        c_refused(&path, b"{\"format\": 8}");
        let good = {
            sample().write_snapshot(&path).unwrap();
            std::fs::read(&path).unwrap()
        };
        c_refused(&path, &good[..good.len() - 100]);
        let mut other_format = good.clone();
        other_format[8] ^= 1;
        c_refused(&path, &other_format);
        let mut other_layout = good.clone();
        other_layout[12] ^= 1;
        c_refused(&path, &other_layout);
    }

    fn c_refused(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        assert!(Catalog::open_snapshot(path).is_err());
    }

    #[test]
    fn rows_have_no_padding() {
        use std::mem::size_of;
        // The sum of the fields, as laid out in `repr(C)`: nothing is left to be written
        // as uninitialized bytes.
        assert_eq!(size_of::<TrackRow>(), 192);
        assert_eq!(size_of::<AlbumRow>(), 112);
        assert_eq!(size_of::<CreditRow>(), 28);
        assert_eq!(size_of::<MembershipRow>(), 20);
        assert_eq!(size_of::<Span>(), 8);
    }
}
