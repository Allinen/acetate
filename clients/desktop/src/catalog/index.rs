//! The catalog with its lookups: by id, an album's tracks in order, an artist's tracks
//! and albums, the user's annotations, and lowercase text for filtering and sorting.
//!
//! Things are referred to by their position in the catalog's lists (`u32`, small and
//! quick to sort), not by id; positions are only valid for the catalog they came from,
//! which is replaced as a whole when the server's copy changes.

use std::cmp::Ordering;
use std::collections::HashMap;

use uuid::Uuid;

use super::model::{self, Album, Annotation, Artist, Grouping, Playlist, Track};

pub struct Catalog {
    /// Its version, for asking the server what changed since (None: an older server).
    pub version: Option<String>,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
    pub annotations: HashMap<Uuid, Annotation>,
    /// Each track's open review flags: (flag id, kind), in the order to show them. Only
    /// tracks with any.
    pub issues: HashMap<Uuid, Vec<(Uuid, String)>>,
    /// Collectives, eras, projects…, by name.
    pub groupings: Vec<Grouping>,
    /// Extras (vocals, stems…: not tracks), not taken off.
    pub extras: Vec<model::Extra>,
    /// Lowercase text to search them by: name, kind, what they belong to.
    extra_text: Vec<String>,

    track_by_id: HashMap<Uuid, u32>,
    grouping_by_id: HashMap<Uuid, u32>,
    /// Each grouping's tracks (newest release first) and albums (newest first).
    grouping_tracks: HashMap<Uuid, Vec<u32>>,
    grouping_albums: HashMap<Uuid, Vec<u32>>,
    album_by_id: HashMap<Uuid, u32>,
    artist_by_id: HashMap<Uuid, u32>,
    /// Each album's tracks, by disc and track number.
    album_tracks: HashMap<Uuid, Vec<u32>>,
    /// Each artist's tracks (any role), newest release first.
    artist_tracks: HashMap<Uuid, Vec<u32>>,
    /// Each artist's albums (credited on the album), newest first.
    artist_albums: HashMap<Uuid, Vec<u32>>,

    /// Lowercase text to filter by: title, artists and album (tracks); title and artists
    /// (albums); name and aliases (artists).
    track_text: Vec<String>,
    album_text: Vec<String>,
    artist_text: Vec<String>,
    /// Lowercase sort keys.
    track_title_key: Vec<String>,
    track_artist_key: Vec<String>,
    album_title_key: Vec<String>,
    album_artist_key: Vec<String>,
    artist_name_key: Vec<String>,
    /// Each track's main album's place in title order (`u32::MAX`: on none), so sorting
    /// tracks by album compares numbers.
    track_album_rank: Vec<u32>,
}

impl Catalog {
    pub fn build(raw: model::Raw) -> Catalog {
        let tracks = raw.tracks.rows;
        let albums = raw.albums.rows;
        let artists = raw.artists.rows;
        let by_id = |ids: &mut dyn Iterator<Item = Uuid>| -> HashMap<Uuid, u32> {
            ids.enumerate().map(|(i, id)| (id, i as u32)).collect()
        };
        let track_by_id = by_id(&mut tracks.iter().map(|t| t.id));
        let album_by_id = by_id(&mut albums.iter().map(|a| a.id));
        let artist_by_id = by_id(&mut artists.iter().map(|a| a.id));

        let mut album_tracks: HashMap<Uuid, Vec<(Option<i16>, Option<i16>, u32)>> = HashMap::new();
        let mut artist_tracks: HashMap<Uuid, Vec<u32>> = HashMap::new();
        for (i, track) in tracks.iter().enumerate() {
            for model::Membership(album, disc, number) in &track.albums {
                album_tracks.entry(*album).or_default().push((*disc, *number, i as u32));
            }
            for credit in &track.credits {
                let list = artist_tracks.entry(credit.0).or_default();
                // An artist credited twice on a track (say, primary and producer) lists
                // it once.
                if list.last() != Some(&(i as u32)) {
                    list.push(i as u32);
                }
            }
        }
        let album_tracks = album_tracks
            .into_iter()
            .map(|(album, mut list)| {
                // Disc and number, missing ones last, then title.
                list.sort_by(|a, b| {
                    let key = |x: &(Option<i16>, Option<i16>, u32)| (x.0.unwrap_or(i16::MAX), x.1.unwrap_or(i16::MAX));
                    key(a).cmp(&key(b)).then_with(|| tracks[a.2 as usize].title.cmp(&tracks[b.2 as usize].title))
                });
                (album, list.into_iter().map(|x| x.2).collect())
            })
            .collect();
        for list in artist_tracks.values_mut() {
            list.sort_by(|a, b| newest_first(&tracks[*a as usize], &tracks[*b as usize]));
        }
        let mut artist_albums: HashMap<Uuid, Vec<u32>> = HashMap::new();
        for (i, album) in albums.iter().enumerate() {
            for credit in &album.credits {
                let list = artist_albums.entry(credit.0).or_default();
                if list.last() != Some(&(i as u32)) {
                    list.push(i as u32);
                }
            }
        }
        for list in artist_albums.values_mut() {
            list.sort_by(|a, b| {
                let (a, b) = (&albums[*a as usize], &albums[*b as usize]);
                b.year.cmp(&a.year).then_with(|| b.id.cmp(&a.id))
            });
        }
        // Each grouping's tracks and albums, in the same orders as an artist's.
        let mut grouping_tracks: HashMap<Uuid, Vec<u32>> = HashMap::new();
        for (i, track) in tracks.iter().enumerate() {
            for grouping in &track.groupings {
                grouping_tracks.entry(*grouping).or_default().push(i as u32);
            }
        }
        for list in grouping_tracks.values_mut() {
            list.sort_by(|a, b| newest_first(&tracks[*a as usize], &tracks[*b as usize]));
        }
        let mut grouping_albums: HashMap<Uuid, Vec<u32>> = HashMap::new();
        for (i, album) in albums.iter().enumerate() {
            for grouping in &album.groupings {
                grouping_albums.entry(*grouping).or_default().push(i as u32);
            }
        }
        for list in grouping_albums.values_mut() {
            list.sort_by(|a, b| {
                let (a, b) = (&albums[*a as usize], &albums[*b as usize]);
                b.year.cmp(&a.year).then_with(|| b.id.cmp(&a.id))
            });
        }

        let album_title = |id: Uuid| album_by_id.get(&id).map(|&i| albums[i as usize].title.as_str()).unwrap_or("");
        let track_text = tracks
            .iter()
            .map(|t| {
                let album = t.albums.first().map(|m| album_title(m.0)).unwrap_or("");
                format!("{}\0{}\0{}", t.title, t.artist_display, album).to_lowercase()
            })
            .collect();
        let album_text = albums.iter().map(|a| format!("{}\0{}", a.title, a.artist_display).to_lowercase()).collect();
        let artist_text = artists
            .iter()
            .map(|a| format!("{}\0{}", a.name, a.aliases.join("\0")).to_lowercase())
            .collect();

        let album_title_key: Vec<String> = albums.iter().map(|a| sort_key(&a.title)).collect();
        let mut by_title: Vec<u32> = (0..albums.len() as u32).collect();
        by_title.sort_by(|a, b| by_key(&album_title_key, *a, *b));
        let mut album_rank = vec![0u32; albums.len()];
        for (rank, album) in by_title.into_iter().enumerate() {
            album_rank[album as usize] = rank as u32;
        }
        let track_album_rank = tracks
            .iter()
            .map(|t| t.albums.first().and_then(|m| album_by_id.get(&m.0)).map(|&a| album_rank[a as usize]).unwrap_or(u32::MAX))
            .collect();

        // Extras (not those taken off), found by their name, kind and what they belong to.
        let extras: Vec<model::Extra> = raw.extras.rows.into_iter().filter(|e| !e.hidden).collect();
        let extra_text = extras
            .iter()
            .map(|e| {
                let linked: Vec<&str> = e
                    .links
                    .iter()
                    .filter_map(|(kind, id)| match kind.as_str() {
                        "track" => track_by_id.get(id).map(|&i| tracks[i as usize].title.as_str()),
                        "album" => album_by_id.get(id).map(|&i| albums[i as usize].title.as_str()),
                        _ => artist_by_id.get(id).map(|&i| artists[i as usize].name.as_str()),
                    })
                    .collect();
                format!("{}\0{}\0{}", e.name, e.kind, linked.join("\0")).to_lowercase()
            })
            .collect();

        Catalog {
            version: raw.version,
            track_album_rank,
            album_title_key,
            track_title_key: tracks.iter().map(|t| sort_key(&t.title)).collect(),
            track_artist_key: tracks.iter().map(|t| sort_key(&t.artist_display)).collect(),
            album_artist_key: albums.iter().map(|a| sort_key(&a.artist_display)).collect(),
            artist_name_key: artists.iter().map(|a| sort_key(&a.name)).collect(),
            annotations: raw.annotations.rows.into_iter().map(|a| (a.id, a)).collect(),
            issues: raw.issues.rows.into_iter().filter(|i| !i.1.is_empty()).map(|i| (i.0, i.1)).collect(),
            grouping_by_id: raw.groupings.rows.iter().enumerate().map(|(i, g)| (g.id, i as u32)).collect(),
            grouping_tracks,
            grouping_albums,
            groupings: raw.groupings.rows,
            extra_text,
            extras,
            playlists: raw.playlists,
            tracks,
            albums,
            artists,
            track_by_id,
            album_by_id,
            artist_by_id,
            album_tracks,
            artist_tracks,
            artist_albums,
            track_text,
            album_text,
            artist_text,
        }
    }

    /// This catalog with a delta applied (`Raw::delta`): changed rows replaced by id,
    /// new ones added, removed ones dropped; then indexed again (a fraction of a second
    /// at 185k tracks: call it off the UI thread).
    pub fn apply(&self, delta: model::Raw) -> Catalog {
        Catalog::build(merge_raw(self.to_raw(), delta))
    }

    /// The rows it was built from (for merging a delta into).
    fn to_raw(&self) -> model::Raw {
        let part = |rows| model::Part { columns: Vec::new(), rows };
        model::Raw {
            format: model::FORMAT,
            version: self.version.clone(),
            delta: false,
            removed: model::Removed::default(),
            tracks: part(self.tracks.clone()),
            albums: model::Part { columns: Vec::new(), rows: self.albums.clone() },
            artists: model::Part { columns: Vec::new(), rows: self.artists.clone() },
            annotations: model::Part { columns: Vec::new(), rows: self.annotations.values().cloned().collect() },
            issues: model::Part { columns: Vec::new(), rows: self.issues.iter().map(|(t, f)| model::Issues(*t, f.clone())).collect() },
            groupings: model::Part { columns: Vec::new(), rows: self.groupings.clone() },
            extras: model::Part { columns: Vec::new(), rows: self.extras.clone() },
            playlists: self.playlists.clone(),
        }
    }

    /// A track's position in `tracks`.
    pub fn track_index(&self, id: Uuid) -> Option<u32> {
        self.track_by_id.get(&id).copied()
    }

    pub fn album(&self, id: Uuid) -> Option<&Album> {
        self.album_by_id.get(&id).map(|&i| &self.albums[i as usize])
    }

    pub fn artist(&self, id: Uuid) -> Option<&Artist> {
        self.artist_by_id.get(&id).map(|&i| &self.artists[i as usize])
    }

    /// An artist's name with its disambiguation ("Kai (UK producer)"), as it is now.
    pub fn artist_label(&self, id: Uuid) -> Option<String> {
        self.artist(id).map(Artist::label)
    }

    /// The album's tracks in order (positions in `tracks`).
    pub fn album_tracks(&self, album: Uuid) -> &[u32] {
        self.album_tracks.get(&album).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn artist_tracks(&self, artist: Uuid) -> &[u32] {
        self.artist_tracks.get(&artist).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn artist_albums(&self, artist: Uuid) -> &[u32] {
        self.artist_albums.get(&artist).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn grouping(&self, id: Uuid) -> Option<&Grouping> {
        self.grouping_by_id.get(&id).map(|&i| &self.groupings[i as usize])
    }

    /// A grouping's tracks (newest release first) and albums (newest first).
    pub fn grouping_tracks(&self, grouping: Uuid) -> &[u32] {
        self.grouping_tracks.get(&grouping).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn grouping_albums(&self, grouping: Uuid) -> &[u32] {
        self.grouping_albums.get(&grouping).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The track's main album, if it's on one.
    pub fn main_album(&self, track: &Track) -> Option<&Album> {
        track.albums.first().and_then(|m| self.album(m.0))
    }

    /// The lowercase text of every track, album and artist, for searching.
    /// A track's text to find it by: title, artists and album, lowercase.
    pub fn track_text(&self, i: u32) -> &str {
        &self.track_text[i as usize]
    }

    pub fn texts(&self) -> (&[String], &[String], &[String]) {
        (&self.track_text, &self.album_text, &self.artist_text)
    }

    /// The extras' search text (by place in `extras`).
    pub fn extra_texts(&self) -> &[String] {
        &self.extra_text
    }


    // ---- Lists: filtered and sorted ----

    /// All tracks, filtered and sorted (the tests' and timing's shorthand).
    #[cfg(test)]
    pub fn track_list(&self, filter: &str, sort: TrackSort, descending: bool) -> Vec<u32> {
        let rows = TrackRows { tracks: None, added: None };
        self.order_tracks(&rows, filter, sort, descending).into_iter().map(|i| i as u32).collect()
    }

    /// Some tracks (a playlist's, say; the same track may be there twice), filtered and
    /// sorted: the positions in `rows` to show, in order.
    pub fn order_tracks(&self, rows: &TrackRows, filter: &str, sort: TrackSort, descending: bool) -> Vec<usize> {
        // Words and `key:value` parts alike (see `query`).
        let query = super::query::Query::parse(filter);
        let track = |row: usize| rows.track(row);
        let mut list: Vec<usize> = (0..rows.len(self)).filter(|&row| query.matches_track(self, track(row), "")).collect();
        let t = &self.tracks;
        let plays = |i: u32| self.annotations.get(&t[i as usize].id).map(|a| a.play_count).unwrap_or(0);
        match sort {
            TrackSort::Natural => {}
            // Newest first: when it was added to the playlist, or to the library.
            TrackSort::Added => match rows.added {
                Some(added) => list.sort_by(|a, b| added[*b].cmp(&added[*a]).then(a.cmp(b))),
                None => list.sort_by(|a, b| t[track(*b) as usize].id.cmp(&t[track(*a) as usize].id)),
            },
            TrackSort::Title => list.sort_by(|a, b| by_key(&self.track_title_key, track(*a), track(*b))),
            TrackSort::Artist => list.sort_by(|a, b| {
                by_key(&self.track_artist_key, track(*a), track(*b)).then_with(|| newest_first(&t[track(*a) as usize], &t[track(*b) as usize]))
            }),
            TrackSort::Album => list.sort_by(|a, b| {
                let rank = &self.track_album_rank;
                rank[track(*a) as usize].cmp(&rank[track(*b) as usize]).then_with(|| by_key(&self.track_title_key, track(*a), track(*b)))
            }),
            TrackSort::Year => list.sort_by(|a, b| newest_first(&t[track(*a) as usize], &t[track(*b) as usize])),
            TrackSort::Length => list.sort_by(|a, b| t[track(*b) as usize].duration_ms.cmp(&t[track(*a) as usize].duration_ms)),
            TrackSort::Plays => list.sort_by(|a, b| plays(track(*b)).cmp(&plays(track(*a))).then(a.cmp(b))),
            TrackSort::Og => list.sort_by_key(|r| t[track(*r) as usize].original_filename.is_none()),
        }
        if descending {
            list.reverse();
        }
        list
    }

    pub fn album_list(&self, filter: &str, sort: AlbumSort) -> Vec<u32> {
        use super::query::{Key, Query};
        let query = Query::parse(filter);
        let mut list: Vec<u32> = matching(&self.album_text, &query.words.join(" "))
            .into_iter()
            .filter(|&i| {
                let a = &self.albums[i as usize];
                query.fields.iter().filter(|(k, v)| !v.is_empty() || *k == Key::Og).all(|(key, value)| {
                    let value = value.to_lowercase();
                    match key {
                        Key::Artist => a.artist_display.to_lowercase().contains(&value),
                        Key::Title | Key::Album => a.title.to_lowercase().contains(&value),
                        Key::Year => a.release_date.as_deref().is_some_and(|d| d.starts_with(&value)) || a.year.is_some_and(|y| y.to_string() == value),
                        Key::Kind => a.kind.starts_with(&value),
                        // Albums with an original file on them (or, `og:no`, without any).
                        Key::Og => {
                            let tracks = self.album_tracks(a.id);
                            match value.as_str() {
                                "no" | "n" | "false" | "0" => tracks.iter().all(|&t| self.tracks[t as usize].original_filename.is_none()),
                                _ => tracks.iter().any(|&t| super::query::og_matches(self.tracks[t as usize].original_filename.as_deref(), &value)),
                            }
                        }
                        // Keys about tracks don't apply to albums.
                        _ => true,
                    }
                })
            })
            .collect();
        let a = &self.albums;
        match sort {
            AlbumSort::Added => list.sort_by(|x, y| a[*y as usize].id.cmp(&a[*x as usize].id)),
            AlbumSort::Title => list.sort_by(|x, y| by_key(&self.album_title_key, *x, *y)),
            AlbumSort::Artist => list.sort_by(|x, y| {
                by_key(&self.album_artist_key, *x, *y).then_with(|| a[*x as usize].year.cmp(&a[*y as usize].year))
            }),
            AlbumSort::Year => list.sort_by(|x, y| {
                let (x, y) = (&a[*x as usize], &a[*y as usize]);
                y.year.cmp(&x.year).then_with(|| y.id.cmp(&x.id))
            }),
        }
        list
    }

    pub fn artist_list(&self, filter: &str, sort: ArtistSort) -> Vec<u32> {
        // `artist:x` (or `name:x`) is a name; other keys don't apply to artists.
        let query = super::query::Query::parse(filter);
        let mut words = query.words.clone();
        for (key, value) in &query.fields {
            if matches!(key, super::query::Key::Artist | super::query::Key::Title) {
                words.extend(value.to_lowercase().split_whitespace().map(str::to_string));
            }
        }
        let mut list = matching(&self.artist_text, &words.join(" "));
        match sort {
            ArtistSort::Name => list.sort_by(|x, y| by_key(&self.artist_name_key, *x, *y)),
            ArtistSort::Tracks => list.sort_by(|x, y| {
                let (x, y) = (*x as usize, *y as usize);
                self.artists[y].track_count.cmp(&self.artists[x].track_count).then_with(|| self.artist_name_key[x].cmp(&self.artist_name_key[y]))
            }),
        }
        list
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackSort {
    /// As given (a playlist's own order).
    Natural,
    Added,
    Title,
    Artist,
    Album,
    Year,
    Length,
    Plays,
    /// Original files (og files) first, the rest after, each in the list's order.
    Og,
}

/// Tracks to list: `tracks` (positions in the catalog; None: all of them), and when each
/// was added (for a playlist: its entries' dates, sortable as text), if known.
pub struct TrackRows<'a> {
    pub tracks: Option<&'a [u32]>,
    pub added: Option<&'a [String]>,
}

impl TrackRows<'_> {
    pub fn len(&self, catalog: &Catalog) -> usize {
        self.tracks.map(<[u32]>::len).unwrap_or(catalog.tracks.len())
    }

    /// The track (a position in the catalog) of row `row`.
    pub fn track(&self, row: usize) -> u32 {
        match self.tracks {
            Some(tracks) => tracks[row],
            None => row as u32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumSort {
    Added,
    Title,
    Artist,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistSort {
    Name,
    Tracks,
}

/// Positions whose text contains every word of `filter` (all of them if it's empty).
fn matching(text: &[String], filter: &str) -> Vec<u32> {
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return (0..text.len() as u32).collect();
    }
    text.iter()
        .enumerate()
        .filter(|(_, t)| words.iter().all(|w| t.contains(w.as_str())))
        .map(|(i, _)| i as u32)
        .collect()
}

/// Lowercase, with a leading "the " left out, so "The Weeknd" sorts under W.
fn sort_key(text: &str) -> String {
    let lower = text.trim().to_lowercase();
    match lower.strip_prefix("the ") {
        Some(rest) => rest.to_string(),
        None => lower,
    }
}

fn by_key(keys: &[String], a: u32, b: u32) -> Ordering {
    keys[a as usize].cmp(&keys[b as usize]).then(a.cmp(&b))
}

/// A whole catalog's rows with a delta merged in (rows replaced by id, new ones added,
/// removed ones dropped), without building the index: several deltas are merged first
/// and the index built once (building it takes a moment at 185k tracks).
pub fn merge_raw(base: model::Raw, delta: model::Raw) -> model::Raw {
    fn merge<T>(rows: Vec<T>, changed: Vec<T>, removed: &[Uuid], id: impl Fn(&T) -> Uuid) -> Vec<T> {
        let removed: std::collections::HashSet<Uuid> = removed.iter().copied().collect();
        let mut changed: HashMap<Uuid, T> = changed.into_iter().map(|r| (id(&r), r)).collect();
        let mut out: Vec<T> = rows
            .into_iter()
            .filter(|r| !removed.contains(&id(r)))
            .map(|r| changed.remove(&id(&r)).unwrap_or(r))
            .collect();
        // New ones (their order doesn't matter: lists are sorted when shown).
        out.extend(changed.into_values());
        out
    }
    let removed = &delta.removed;
    let tracks = merge(base.tracks.rows, delta.tracks.rows, &removed.tracks, |t| t.id);
    let albums = merge(base.albums.rows, delta.albums.rows, &removed.albums, |a| a.id);
    let artists = merge(base.artists.rows, delta.artists.rows, &removed.artists, |a| a.id);
    // A changed extra replaces its row (one taken off comes flagged, and is left out when
    // indexed).
    let extras = merge(base.extras.rows, delta.extras.rows, &[], |e| e.id);
    let mut annotations: HashMap<Uuid, model::Annotation> = base.annotations.rows.into_iter().map(|a| (a.id, a)).collect();
    for id in &removed.annotations {
        annotations.remove(id);
    }
    for a in delta.annotations.rows {
        annotations.insert(a.id, a);
    }
    // A changed track's flags replace its old ones (none left: an empty list).
    let mut issues: HashMap<Uuid, Vec<(Uuid, String)>> = base.issues.rows.into_iter().map(|i| (i.0, i.1)).collect();
    for id in &removed.tracks {
        issues.remove(id);
    }
    for model::Issues(track, flags) in delta.issues.rows {
        if flags.is_empty() {
            issues.remove(&track);
        } else {
            issues.insert(track, flags);
        }
    }
    let part = |rows| model::Part { columns: Vec::new(), rows };
    model::Raw {
        format: delta.format,
        version: delta.version.or(base.version),
        delta: false,
        removed: model::Removed::default(),
        tracks: part(tracks),
        albums: model::Part { columns: Vec::new(), rows: albums },
        artists: model::Part { columns: Vec::new(), rows: artists },
        annotations: model::Part { columns: Vec::new(), rows: annotations.into_values().collect() },
        issues: model::Part { columns: Vec::new(), rows: issues.into_iter().map(|(t, f)| model::Issues(t, f)).collect() },
        extras: model::Part { columns: Vec::new(), rows: extras },
        // Sent whole with every delta.
        groupings: delta.groupings,
        playlists: delta.playlists,
    }
}

/// Newest release first (no date last), then newest added.
fn newest_first(a: &Track, b: &Track) -> Ordering {
    fn date(t: &Track) -> &str {
        t.release_date.as_deref().unwrap_or("")
    }
    date(b).cmp(date(a)).then_with(|| b.id.cmp(&a.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        // Two albums; track 3 is on both, first on "B" (its main one).
        let body = serde_json::json!({
            "format": 6,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename"],
                       "rows": [
                ["00000000-0000-7000-8000-000000000001", "Second", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]],
                 [["00000000-0000-7000-8000-0000000000b1", 1, 2]], 1000, "00000000-0000-7000-8000-0000000000f1", null, null, "2020", 2020, "released", [], "", false, "flac"],
                ["00000000-0000-7000-8000-000000000002", "First", "Ann feat. Bo",
                 [["00000000-0000-7000-8000-0000000000a1", "primary", null], ["00000000-0000-7000-8000-0000000000a2", "featured", "B.O."]],
                 [["00000000-0000-7000-8000-0000000000b1", 1, 1]], 2000, "00000000-0000-7000-8000-0000000000f2", null, null, "2021", 2021, "released", [], "", false, "flac"],
                ["00000000-0000-7000-8000-000000000003", "The Third", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", null]],
                 [["00000000-0000-7000-8000-0000000000b2", null, 1], ["00000000-0000-7000-8000-0000000000b1", 1, 3]], 500, "00000000-0000-7000-8000-0000000000f3", null, null, null, null, "released", [], "", false, "mp3"]
            ]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [
                ["00000000-0000-7000-8000-0000000000b1", "A", "album", "Ann", [["00000000-0000-7000-8000-0000000000a1", "primary", null]], "2021", 2021, null, null, "", 3, 3500],
                ["00000000-0000-7000-8000-0000000000b2", "B", "single", "Bo", [["00000000-0000-7000-8000-0000000000a2", "primary", null]], null, null, null, null, "", 1, 500]
            ]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [
                ["00000000-0000-7000-8000-0000000000a1", "Ann", "", [], 2, 1],
                ["00000000-0000-7000-8000-0000000000a2", "Bo", "", ["B.O."], 2, 1]
            ]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            // Track 1 has no cover and "fix later"; track 2 no cover.
            "issues": {"columns": ["track_id", "flags"], "rows": [
                ["00000000-0000-7000-8000-000000000001", [["00000000-0000-7000-8000-0000000000e1", "fix_later"], ["00000000-0000-7000-8000-0000000000e2", "missing_cover"]]],
                ["00000000-0000-7000-8000-000000000002", [["00000000-0000-7000-8000-0000000000e3", "missing_cover"]]]
            ]},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(model::parse(body.to_string().as_bytes()).unwrap())
    }

    fn titles(c: &Catalog, list: &[u32]) -> Vec<String> {
        list.iter().map(|i| c.tracks[*i as usize].title.clone()).collect()
    }

    #[test]
    fn an_album_lists_all_its_tracks_in_order() {
        let c = catalog();
        let a = "00000000-0000-7000-8000-0000000000b1".parse().unwrap();
        assert_eq!(titles(&c, c.album_tracks(a)), ["First", "Second", "The Third"]);
        let bo = "00000000-0000-7000-8000-0000000000a2".parse().unwrap();
        assert_eq!(titles(&c, c.artist_tracks(bo)), ["First", "The Third"], "featured too, newest first");
    }

    #[test]
    fn sorting_and_filtering() {
        let c = catalog();
        // "The Third" sorts under T without "the ", so: First, Second, Third.
        assert_eq!(titles(&c, &c.track_list("", TrackSort::Title, false)), ["First", "Second", "The Third"]);
        assert_eq!(titles(&c, &c.track_list("", TrackSort::Length, false)), ["First", "Second", "The Third"]);
        assert_eq!(titles(&c, &c.track_list("", TrackSort::Length, true)), ["The Third", "Second", "First"]);
        // Words anywhere in title, artists or album, all of them.
        assert_eq!(titles(&c, &c.track_list("bo fir", TrackSort::Title, false)), ["First"]);
        assert_eq!(c.artist_list("b.o.", ArtistSort::Name).len(), 1, "by alias");
    }

    #[test]
    fn fuzzy_search_finds_letters_in_order() {
        let c = catalog();
        let mut search = crate::catalog::search::Search::default();
        let hits = search.run(&c, "thrd", 10);
        assert_eq!(titles(&c, &hits.tracks.iter().map(|h| h.0).collect::<Vec<_>>()), ["The Third"]);
        // Words match separately, anywhere: an artist and a title.
        let hits = search.run(&c, "bo first", 10);
        assert_eq!(titles(&c, &hits.tracks.iter().map(|h| h.0).collect::<Vec<_>>()), ["First"]);
        assert_eq!(search.run(&c, "b.o.", 10).artists.len(), 1, "an alias");
    }

    #[test]
    fn a_delta_replaces_adds_and_removes() {
        let c = catalog();
        let t = |n: u8| format!("00000000-0000-7000-8000-00000000000{n}");
        let track = |n: u8, title: &str| {
            let album = serde_json::json!([["00000000-0000-7000-8000-0000000000b1", 1, 2]]);
            serde_json::json!([t(n), title, "Ann", [], album, 1000, "00000000-0000-7000-8000-0000000000f9", null, null, null, null, "released", [], "", false, "flac"])
        };
        let delta = serde_json::json!({
            "format": 6, "delta": true, "version": "10:20:",
            "tracks": {"columns": [], "rows": [track(1, "Second (edited)"), track(4, "Fourth")]},
            "albums": {"columns": [], "rows": []},
            "artists": {"columns": [], "rows": []},
            "annotations": {"columns": [], "rows": []},
            // Track 1's cover came (only "fix later" is left); track 2's flags are all
            // settled; the new track 4 has none of its own cover.
            "issues": {"columns": [], "rows": [
                [t(1), [["00000000-0000-7000-8000-0000000000e1", "fix_later"]]],
                [t(2), []],
                [t(4), [["00000000-0000-7000-8000-0000000000e4", "missing_cover"]]]
            ]},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": [],
            "removed": {"tracks": [t(3)]}
        });
        let delta: model::Raw = serde_json::from_value(delta).unwrap();
        let c = c.apply(delta);
        let mut titles: Vec<String> = c.tracks.iter().map(|t| t.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, ["First", "Fourth", "Second (edited)"]);
        assert_eq!(c.version.as_deref(), Some("10:20:"));
        // The index follows: "The Third" is gone from the album, "Fourth" is on it (by
        // number, then title).
        let a = "00000000-0000-7000-8000-0000000000b1".parse().unwrap();
        assert_eq!(super::tests::titles(&c, c.album_tracks(a)), ["First", "Fourth", "Second (edited)"]);
        // The flags follow: replaced, gone when none are left, added.
        let kinds = |n: u8| -> Vec<String> {
            let id: Uuid = t(n).parse().unwrap();
            c.issues.get(&id).map(|f| f.iter().map(|(_, k)| k.clone()).collect()).unwrap_or_default()
        };
        assert_eq!(kinds(1), ["fix_later"]);
        assert!(kinds(2).is_empty() && !c.issues.contains_key(&t(2).parse().unwrap()));
        assert_eq!(kinds(4), ["missing_cover"]);
    }

    #[test]
    fn unknown_columns_ask_for_an_update() {
        let body = br#"{"format": 7}"#;
        let err = model::parse(body).err().unwrap().to_string();
        assert!(err.contains("update the app"), "{err}");
    }
}

/// Timing at scale: `ACETATE_BENCH_CATALOG=<catalog.json> cargo test --release -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn a_big_catalog() {
        let Ok(path) = std::env::var("ACETATE_BENCH_CATALOG") else { return };
        let body = std::fs::read(path).unwrap();
        let t = Instant::now();
        let raw = model::parse(&body).unwrap();
        println!("parse: {:?}", t.elapsed());
        let t = Instant::now();
        let c = Catalog::build(raw);
        println!("index: {:?} ({} tracks)", t.elapsed(), c.tracks.len());
        for (name, sort) in [("added", TrackSort::Added), ("title", TrackSort::Title), ("artist", TrackSort::Artist), ("album", TrackSort::Album), ("year", TrackSort::Year)] {
            let t = Instant::now();
            let list = c.track_list("", sort, false);
            println!("tracks by {name}: {:?} ({})", t.elapsed(), list.len());
        }
        for filter in ["t", "track 12", "artist 4242 track"] {
            let t = Instant::now();
            let list = c.track_list(filter, TrackSort::Added, false);
            println!("filter {filter:?}: {:?} ({})", t.elapsed(), list.len());
        }
        let mut search = super::super::search::Search::default();
        for query in ["a", "track 12", "artst 42 trck 9", "album 77"] {
            let t = Instant::now();
            let hits = search.run(&c, query, 10);
            println!("search {query:?}: {:?} ({} tracks)", t.elapsed(), hits.tracks.len());
        }
        // A one-track delta, applied (index rebuilt).
        let mut changed = c.tracks[5000].clone();
        changed.title.push_str(" (edited)");
        let delta = model::Raw {
            format: model::FORMAT,
            version: Some("1:2:".into()),
            delta: true,
            removed: model::Removed::default(),
            tracks: model::Part { columns: Vec::new(), rows: vec![changed] },
            albums: model::Part { columns: Vec::new(), rows: Vec::new() },
            artists: model::Part { columns: Vec::new(), rows: Vec::new() },
            annotations: model::Part { columns: Vec::new(), rows: Vec::new() },
            issues: model::Part { columns: Vec::new(), rows: Vec::new() },
            groupings: model::Part { columns: Vec::new(), rows: Vec::new() },
            extras: model::Part::default(),
            playlists: Vec::new(),
        };
        let t = Instant::now();
        let c = c.apply(delta);
        println!("applying a one-track delta: {:?}", t.elapsed());
        let t = Instant::now();
        let list = c.album_list("", AlbumSort::Artist);
        println!("albums by artist: {:?} ({})", t.elapsed(), list.len());
    }
}
