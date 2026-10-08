//! The catalog with its lookups: by id, an album's tracks in order, an artist's tracks
//! and albums, the user's annotations, and lowercase text for filtering and sorting.
//!
//! Things are referred to by their position in the catalog's lists (`u32`, small and
//! quick to sort), not by id; positions are only valid for the catalog they came from,
//! which is replaced as a whole when the server's copy changes.
//!
//! Tracks and albums are kept compactly (see `rows`), and so is the index: lists of
//! positions in one buffer each (`Lists`), lowercase text in one buffer each (`Texts`),
//! and sort orders as ranks (a number per track), not as lowercase copies of the text.

use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::Context;
use serde_json::value::RawValue;
use uuid::Uuid;

use super::flat::Col;
use super::model::{self, Annotation, Artist, Grouping, Playlist};
use super::rows::{self, AlbumRow, Pools, TrackRow};
use super::text::Texts;

pub use super::rows::{Album, Track};

pub struct Catalog {
    /// Its version, for asking the server what changed since (None: an older server).
    pub version: Option<String>,
    /// The server's ETag for the whole catalog this one is from (None: not known).
    pub etag: Option<String>,
    pub(super) pools: Pools,
    pub(super) tracks: Col<TrackRow>,
    pub(super) albums: Col<AlbumRow>,
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
    pub(super) extra_text: Texts,

    pub(super) track_by_id: HashMap<Uuid, u32>,
    pub(super) album_by_id: HashMap<Uuid, u32>,
    pub(super) artist_by_id: HashMap<Uuid, u32>,
    pub(super) grouping_by_id: HashMap<Uuid, u32>,
    /// Each album's tracks (by album position), by disc and track number.
    pub(super) album_tracks: Lists,
    /// Each artist's tracks (any role), newest release first.
    pub(super) artist_tracks: Lists,
    /// Each artist's albums (credited on the album), newest first.
    pub(super) artist_albums: Lists,
    /// Each grouping's tracks (newest release first) and albums (newest first).
    pub(super) grouping_tracks: Lists,
    pub(super) grouping_albums: Lists,

    /// Lowercase text to filter by: title, artists and album (tracks); title and artists
    /// (albums); name and aliases (artists).
    pub(super) track_text: Texts,
    pub(super) album_text: Texts,
    pub(super) artist_text: Texts,
    /// Places in sort order (by lowercase text, "the " left out; ties by position).
    pub(super) track_title_rank: Col<u32>,
    pub(super) track_artist_rank: Col<u32>,
    pub(super) album_title_rank: Col<u32>,
    pub(super) album_artist_rank: Col<u32>,
    pub(super) artist_name_rank: Col<u32>,
    /// Each track's place newest release first (see `newest_first`).
    pub(super) track_newest_rank: Col<u32>,
    /// Each track's main album's place in title order (`u32::MAX`: on none), so sorting
    /// tracks by album compares numbers.
    pub(super) track_album_rank: Col<u32>,
}

/// Lists of positions, one per key (an album's tracks, an artist's…), in one buffer.
#[derive(Default)]
pub(super) struct Lists {
    /// Where each key's list starts in `items` (and, one more, where the last ends).
    pub(super) starts: Col<u32>,
    pub(super) items: Col<u32>,
}

impl Lists {
    /// From (key, item) pairs, keys under `keys`; each item once per key.
    fn build(keys: usize, mut pairs: Vec<(u32, u32)>) -> Lists {
        pairs.sort_unstable();
        pairs.dedup();
        let mut starts = Vec::with_capacity(keys + 1);
        let mut at = 0;
        for key in 0..=keys as u32 {
            starts.push(at as u32);
            while at < pairs.len() && pairs[at].0 == key {
                at += 1;
            }
        }
        Lists { starts: starts.into(), items: pairs.into_iter().map(|p| p.1).collect::<Vec<u32>>().into() }
    }

    fn get(&self, key: u32) -> &[u32] {
        match (self.starts.get(key as usize), self.starts.get(key as usize + 1)) {
            (Some(&start), Some(&end)) => &self.items[start as usize..end as usize],
            _ => &[],
        }
    }

    /// Sorts each list.
    fn sort_each(&mut self, mut by: impl FnMut(&u32, &u32) -> Ordering) {
        let items = self.items.make_mut();
        for w in self.starts.windows(2) {
            items[w[0] as usize..w[1] as usize].sort_by(&mut by);
        }
    }

    #[cfg(test)]
    fn bytes(&self) -> usize {
        (self.starts.len() + self.items.len()) * 4
    }
}

/// Each of `n` things' place in order of `key` (ties by position).
fn ranks(n: usize, key: impl Fn(u32) -> String) -> Vec<u32> {
    let mut keys = Texts::with_capacity(n);
    for i in 0..n as u32 {
        keys.push_lowercase(&[key(i).as_str()]);
    }
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by(|a, b| keys.get(*a).cmp(keys.get(*b)).then(a.cmp(b)));
    let mut rank = vec![0u32; n];
    for (place, i) in order.into_iter().enumerate() {
        rank[i as usize] = place as u32;
    }
    rank
}

impl Catalog {
    /// A catalog from all its rows (a test's, or the server's parsed whole).
    #[cfg(any(test, feature = "test-support"))]
    pub fn build(raw: model::Raw) -> Catalog {
        Merger::from_raw(raw).build(None).expect("rows parsed already")
    }

    /// This catalog with a delta applied (`Raw::delta`): changed rows replaced by id,
    /// new ones added, removed ones dropped; then indexed again (a fraction of a second
    /// at 185k tracks: call it off the UI thread).
    pub fn apply(&self, delta: model::Raw) -> Catalog {
        let mut merger = Merger::from_catalog(self);
        merger.add(delta);
        merger.build(Some(self)).expect("rows parsed already")
    }

    /// Where each track, album, artist and grouping is, by id.
    pub(super) fn id_maps(&self) -> (HashMap<Uuid, u32>, HashMap<Uuid, u32>, HashMap<Uuid, u32>, HashMap<Uuid, u32>) {
        let by_id = |ids: &mut dyn Iterator<Item = Uuid>| -> HashMap<Uuid, u32> { ids.enumerate().map(|(i, id)| (id, i as u32)).collect() };
        (
            by_id(&mut self.tracks.iter().map(|t| t.id)),
            by_id(&mut self.albums.iter().map(|a| a.id)),
            by_id(&mut self.artists.iter().map(|a| a.id)),
            by_id(&mut self.groupings.iter().map(|g| g.id)),
        )
    }

    fn index(&mut self) {
        let track = |i: u32| Track::new(&self.pools, &self.tracks[i as usize]);
        let album = |i: u32| Album::new(&self.pools, &self.albums[i as usize]);
        let (n_tracks, n_albums, n_artists) = (self.tracks.len(), self.albums.len(), self.artists.len());
        let (track_by_id, album_by_id, artist_by_id, grouping_by_id) = self.id_maps();

        let (mut on_album, mut by_artist, mut in_grouping) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n_tracks as u32 {
            let t = track(i);
            on_album.extend(t.albums().iter().filter_map(|m| album_by_id.get(&m.0)).map(|&a| (a, i)));
            by_artist.extend(t.credits().iter().filter_map(|c| artist_by_id.get(&c.0)).map(|&a| (a, i)));
            in_grouping.extend(t.groupings().iter().filter_map(|g| grouping_by_id.get(g)).map(|&g| (g, i)));
        }
        let mut album_tracks = Lists::build(n_albums, on_album);
        // By disc and number on that album.
        for a in 0..n_albums as u32 {
            let id = self.albums[a as usize].id;
            let place = |t: u32| track(t).albums().iter().find(|m| m.0 == id).map(|m| (m.1, m.2)).unwrap_or((None, None));
            let (start, end) = (album_tracks.starts[a as usize] as usize, album_tracks.starts[a as usize + 1] as usize);
            album_tracks.items.make_mut()[start..end].sort_by(|x, y| album_order(place(*x), track(*x).title(), place(*y), track(*y).title()));
        }
        let mut newest: Vec<u32> = (0..n_tracks as u32).collect();
        newest.sort_by(|a, b| newest_first(track(*a), track(*b)));
        let mut track_newest_rank = vec![0u32; n_tracks];
        for (place, i) in newest.into_iter().enumerate() {
            track_newest_rank[i as usize] = place as u32;
        }
        let by_newest = |a: &u32, b: &u32| track_newest_rank[*a as usize].cmp(&track_newest_rank[*b as usize]);
        let mut artist_tracks = Lists::build(n_artists, by_artist);
        artist_tracks.sort_each(by_newest);
        let mut grouping_tracks = Lists::build(self.groupings.len(), in_grouping);
        grouping_tracks.sort_each(by_newest);

        let (mut by_artist, mut in_grouping) = (Vec::new(), Vec::new());
        for i in 0..n_albums as u32 {
            let a = album(i);
            by_artist.extend(a.credits().iter().filter_map(|c| artist_by_id.get(&c.0)).map(|&x| (x, i)));
            in_grouping.extend(a.groupings().iter().filter_map(|g| grouping_by_id.get(g)).map(|&g| (g, i)));
        }
        let newest_album = |x: &u32, y: &u32| {
            let (x, y) = (album(*x), album(*y));
            y.year().cmp(&x.year()).then_with(|| y.id().cmp(&x.id()))
        };
        let mut artist_albums = Lists::build(n_artists, by_artist);
        artist_albums.sort_each(newest_album);
        let mut grouping_albums = Lists::build(self.groupings.len(), in_grouping);
        grouping_albums.sort_each(newest_album);

        let album_title = |id: &Uuid| album_by_id.get(id).map(|&i| album(i).title()).unwrap_or("");
        let mut track_text = Texts::with_capacity(n_tracks);
        for i in 0..n_tracks as u32 {
            let t = track(i);
            track_text.push_lowercase(&[t.title(), t.artist_display(), t.albums().first().map(|m| album_title(&m.0)).unwrap_or("")]);
        }
        let mut album_text = Texts::with_capacity(n_albums);
        for i in 0..n_albums as u32 {
            album_text.push_lowercase(&[album(i).title(), album(i).artist_display()]);
        }
        let mut artist_text = Texts::with_capacity(n_artists);
        for a in &self.artists {
            let mut parts = vec![a.name.as_str()];
            parts.extend(a.aliases.iter().map(String::as_str));
            artist_text.push_lowercase(&parts);
        }

        let album_title_rank = ranks(n_albums, |i| sort_key(album(i).title()));
        let track_album_rank: Vec<u32> = (0..n_tracks as u32)
            .map(|i| track(i).albums().first().and_then(|m| album_by_id.get(&m.0)).map(|&a| album_title_rank[a as usize]).unwrap_or(u32::MAX))
            .collect();

        // Extras (not those taken off), found by their name, kind and what they belong to.
        let mut extra_text = Texts::with_capacity(self.extras.len());
        for e in &self.extras {
            let mut parts = vec![e.name.as_str(), e.kind.as_str()];
            parts.extend(e.links.iter().filter_map(|(kind, id)| match kind.as_str() {
                "track" => track_by_id.get(id).map(|&i| track(i).title()),
                "album" => album_by_id.get(id).map(|&i| album(i).title()),
                "grouping" => grouping_by_id.get(id).map(|&i| self.groupings[i as usize].name.as_str()),
                _ => artist_by_id.get(id).map(|&i| self.artists[i as usize].name.as_str()),
            }));
            extra_text.push_lowercase(&parts);
        }

        let track_title_rank = ranks(n_tracks, |i| sort_key(track(i).title()));
        let track_artist_rank = ranks(n_tracks, |i| sort_key(track(i).artist_display()));
        let album_artist_rank = ranks(n_albums, |i| sort_key(album(i).artist_display()));
        let artist_name_rank = ranks(n_artists, |i| sort_key(&self.artists[i as usize].name));

        self.track_by_id = track_by_id;
        self.album_by_id = album_by_id;
        self.artist_by_id = artist_by_id;
        self.grouping_by_id = grouping_by_id;
        self.album_tracks = album_tracks;
        self.artist_tracks = artist_tracks;
        self.artist_albums = artist_albums;
        self.grouping_tracks = grouping_tracks;
        self.grouping_albums = grouping_albums;
        self.track_text = track_text.finish();
        self.album_text = album_text.finish();
        self.artist_text = artist_text.finish();
        self.extra_text = extra_text.finish();
        self.track_title_rank = track_title_rank.into();
        self.track_artist_rank = track_artist_rank.into();
        self.album_title_rank = album_title_rank.into();
        self.album_artist_rank = album_artist_rank.into();
        self.artist_name_rank = artist_name_rank.into();
        self.track_album_rank = track_album_rank.into();
        self.track_newest_rank = track_newest_rank.into();
    }

    #[cfg(test)]
    /// Roughly what it takes in memory, by part (for measuring; not exact).
    pub fn bytes(&self) -> Vec<(&'static str, usize)> {
        use std::mem::size_of;
        let map = |m: &HashMap<Uuid, u32>| m.capacity() * (size_of::<(Uuid, u32)>() + 1);
        let ranks = [&self.track_title_rank, &self.track_artist_rank, &self.album_title_rank, &self.album_artist_rank, &self.artist_name_rank, &self.track_album_rank, &self.track_newest_rank];
        let mut parts = vec![
            ("track rows", self.tracks.len() * size_of::<TrackRow>()),
            ("album rows", self.albums.len() * size_of::<AlbumRow>()),

            ("search text", self.track_text.bytes() + self.album_text.bytes() + self.artist_text.bytes() + self.extra_text.bytes()),
            ("by id", map(&self.track_by_id) + map(&self.album_by_id) + map(&self.artist_by_id)),
            ("lists", self.album_tracks.bytes() + self.artist_tracks.bytes() + self.artist_albums.bytes() + self.grouping_tracks.bytes() + self.grouping_albums.bytes()),
            ("sort ranks", ranks.iter().map(|r| r.len() * 4).sum()),
        ];
        parts.extend(self.pools.bytes());
        parts
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    pub fn album_count(&self) -> usize {
        self.albums.len()
    }

    /// The track at a position in the catalog.
    pub fn track_at(&self, i: u32) -> Track<'_> {
        Track::new(&self.pools, &self.tracks[i as usize])
    }

    pub fn album_at(&self, i: u32) -> Album<'_> {
        Album::new(&self.pools, &self.albums[i as usize])
    }

    /// The track at a position, if there's one there.
    pub fn get_track(&self, i: u32) -> Option<Track<'_>> {
        self.tracks.get(i as usize).map(|row| Track::new(&self.pools, row))
    }

    pub fn get_album(&self, i: u32) -> Option<Album<'_>> {
        self.albums.get(i as usize).map(|row| Album::new(&self.pools, row))
    }

    pub fn tracks(&self) -> impl ExactSizeIterator<Item = Track<'_>> + '_ {
        self.tracks.iter().map(|row| Track::new(&self.pools, row))
    }

    pub fn albums(&self) -> impl ExactSizeIterator<Item = Album<'_>> + '_ {
        self.albums.iter().map(|row| Album::new(&self.pools, row))
    }

    /// A track's position in the catalog.
    pub fn track_index(&self, id: Uuid) -> Option<u32> {
        self.track_by_id.get(&id).copied()
    }

    pub fn album_index(&self, id: Uuid) -> Option<u32> {
        self.album_by_id.get(&id).copied()
    }

    pub fn track(&self, id: Uuid) -> Option<Track<'_>> {
        self.track_index(id).map(|i| self.track_at(i))
    }

    pub fn album(&self, id: Uuid) -> Option<Album<'_>> {
        self.album_index(id).map(|i| self.album_at(i))
    }

    pub fn artist(&self, id: Uuid) -> Option<&Artist> {
        self.artist_by_id.get(&id).map(|&i| &self.artists[i as usize])
    }

    /// An artist's name with its disambiguation ("Kai (UK producer)"), as it is now.
    pub fn artist_label(&self, id: Uuid) -> Option<String> {
        self.artist(id).map(Artist::label)
    }

    /// The album's tracks in order (positions in the catalog).
    pub fn album_tracks(&self, album: Uuid) -> &[u32] {
        self.album_by_id.get(&album).map(|&a| self.album_tracks.get(a)).unwrap_or(&[])
    }

    pub fn artist_tracks(&self, artist: Uuid) -> &[u32] {
        self.artist_by_id.get(&artist).map(|&a| self.artist_tracks.get(a)).unwrap_or(&[])
    }

    pub fn artist_albums(&self, artist: Uuid) -> &[u32] {
        self.artist_by_id.get(&artist).map(|&a| self.artist_albums.get(a)).unwrap_or(&[])
    }

    pub fn grouping(&self, id: Uuid) -> Option<&Grouping> {
        self.grouping_by_id.get(&id).map(|&i| &self.groupings[i as usize])
    }

    /// A grouping's tracks (newest release first) and albums (newest first).
    pub fn grouping_tracks(&self, grouping: Uuid) -> &[u32] {
        self.grouping_by_id.get(&grouping).map(|&g| self.grouping_tracks.get(g)).unwrap_or(&[])
    }

    pub fn grouping_albums(&self, grouping: Uuid) -> &[u32] {
        self.grouping_by_id.get(&grouping).map(|&g| self.grouping_albums.get(g)).unwrap_or(&[])
    }

    /// The track's main album, if it's on one.
    pub fn main_album(&self, track: Track) -> Option<Album<'_>> {
        track.albums().first().and_then(|m| self.album(m.0))
    }

    /// A track's text to find it by: title, artists and album, lowercase.
    pub fn track_text(&self, i: u32) -> &str {
        self.track_text.get(i)
    }

    /// The lowercase text of every track, album and artist, for searching.
    pub fn texts(&self) -> (&Texts, &Texts, &Texts) {
        (&self.track_text, &self.album_text, &self.artist_text)
    }

    /// The extras' search text (by place in `extras`).
    pub fn extra_texts(&self) -> &Texts {
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
        let t = |row: usize| self.track_at(track(row));
        let plays = |row: usize| self.annotations.get(&t(row).id()).map(|a| a.play_count).unwrap_or(0);
        let rank = |ranks: &[u32], row: usize| ranks[track(row) as usize];
        match sort {
            TrackSort::Natural => {}
            // Newest first: when it was added to the playlist, or to the library.
            TrackSort::Added => match rows.added {
                Some(added) => list.sort_by(|a, b| added[*b].cmp(&added[*a]).then(a.cmp(b))),
                None => list.sort_by_key(|r| std::cmp::Reverse(t(*r).id())),
            },
            // Ranks are unique (ties are by position), so these are whole orders.
            TrackSort::Title => list.sort_by_key(|r| rank(&self.track_title_rank, *r)),
            TrackSort::Artist => list.sort_by_key(|r| rank(&self.track_artist_rank, *r)),
            TrackSort::Album => list.sort_by_key(|r| (rank(&self.track_album_rank, *r), rank(&self.track_title_rank, *r))),
            TrackSort::Year => list.sort_by_key(|r| rank(&self.track_newest_rank, *r)),
            TrackSort::Length => list.sort_by_key(|r| std::cmp::Reverse(t(*r).duration_ms())),
            TrackSort::Plays => list.sort_by(|a, b| plays(*b).cmp(&plays(*a)).then(a.cmp(b))),
            TrackSort::Og => list.sort_by_key(|r| t(*r).original_filename().is_none()),
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
                let a = self.album_at(i);
                query.fields.iter().filter(|(k, v)| !v.is_empty() || *k == Key::Og).all(|(key, value)| {
                    let value = value.to_lowercase();
                    match key {
                        Key::Artist => a.artist_display().to_lowercase().contains(&value),
                        Key::Title | Key::Album => a.title().to_lowercase().contains(&value),
                        Key::Year => a.release_date().is_some_and(|d| d.starts_with(&value)) || a.year().is_some_and(|y| y.to_string() == value),
                        Key::Kind => a.kind().starts_with(&value),
                        // Albums with an original file on them (or, `og:no`, without any).
                        Key::Og => {
                            let tracks = self.album_tracks.get(i);
                            match value.as_str() {
                                "no" | "n" | "false" | "0" => tracks.iter().all(|&t| self.track_at(t).original_filename().is_none()),
                                _ => tracks.iter().any(|&t| super::query::og_matches(self.track_at(t).original_filename(), &value)),
                            }
                        }
                        // Keys about tracks don't apply to albums.
                        _ => true,
                    }
                })
            })
            .collect();
        let a = |i: u32| self.album_at(i);
        match sort {
            AlbumSort::Added => list.sort_by_key(|x| std::cmp::Reverse(a(*x).id())),
            AlbumSort::Title => list.sort_by_key(|x| self.album_title_rank[*x as usize]),
            AlbumSort::Artist => list.sort_by_key(|x| self.album_artist_rank[*x as usize]),
            AlbumSort::Year => list.sort_by(|x, y| {
                let (x, y) = (a(*x), a(*y));
                y.year().cmp(&x.year()).then_with(|| y.id().cmp(&x.id()))
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
            ArtistSort::Name => list.sort_by_key(|x| self.artist_name_rank[*x as usize]),
            ArtistSort::Tracks => list.sort_by(|x, y| {
                let (x, y) = (*x as usize, *y as usize);
                self.artists[y].track_count.cmp(&self.artists[x].track_count).then_with(|| self.artist_name_rank[x].cmp(&self.artist_name_rank[y]))
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

/// How an album's tracks are ordered: by disc and number (missing ones last), then title.
pub fn album_order(a: (Option<i16>, Option<i16>), a_title: &str, b: (Option<i16>, Option<i16>), b_title: &str) -> std::cmp::Ordering {
    let key = |x: (Option<i16>, Option<i16>)| (x.0.unwrap_or(i16::MAX), x.1.unwrap_or(i16::MAX));
    key(a).cmp(&key(b)).then_with(|| a_title.cmp(b_title))
}

/// Tracks to list: `tracks` (positions in the catalog; None: all of them), and when each
/// was added (for a playlist: its entries' dates, sortable as text), if known.
pub struct TrackRows<'a> {
    pub tracks: Option<&'a [u32]>,
    pub added: Option<&'a [String]>,
}

impl TrackRows<'_> {
    pub fn len(&self, catalog: &Catalog) -> usize {
        self.tracks.map(<[u32]>::len).unwrap_or(catalog.track_count())
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
fn matching(text: &Texts, filter: &str) -> Vec<u32> {
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

/// Rows kept by id, in their order: a change replaces a row where it is, a removal
/// leaves a hole, new rows come last. A delta costs what it holds, not the catalog's size.
pub(super) struct Rows<T> {
    rows: Vec<Option<T>>,
    at: HashMap<Uuid, usize>,
}

impl<T> Rows<T> {
    pub(super) fn new(rows: Vec<T>, id: impl Fn(&T) -> Uuid) -> Rows<T> {
        let at = rows.iter().enumerate().map(|(i, r)| (id(r), i)).collect();
        Rows { rows: rows.into_iter().map(Some).collect(), at }
    }

    pub(super) fn apply(&mut self, changed: Vec<T>, removed: &[Uuid], id: impl Fn(&T) -> Uuid) {
        for gone in removed {
            if let Some(i) = self.at.remove(gone) {
                self.rows[i] = None;
            }
        }
        for row in changed {
            match self.at.get(&id(&row)) {
                Some(&i) => self.rows[i] = Some(row),
                None => {
                    self.at.insert(id(&row), self.rows.len());
                    self.rows.push(Some(row));
                }
            }
        }
    }

    pub(super) fn finish(self) -> Vec<T> {
        self.rows.into_iter().flatten().collect()
    }
}

/// A track or album row on its way into a catalog: still the server's text (a whole
/// catalog's), parsed (a delta's), or a row of the catalog a delta is applied to.
enum Pending<'a, T> {
    Text(&'a RawValue),
    /// Boxed: a pending row is as small as the others until it's built.
    Row(Box<T>),
    Kept(u32),
}

/// A row's id with the row.
type Keyed<'a, T> = (Uuid, Pending<'a, T>);

fn keyed<T>(id: impl Fn(&T) -> Uuid) -> impl Fn(T) -> (Uuid, Pending<'static, T>) {
    move |row| (id(&row), Pending::Row(Box::new(row)))
}

/// A whole catalog with deltas merged in one after another, then built into a `Catalog`
/// once (indexing takes a moment at 185k tracks, and there can be thousands of deltas).
pub struct Merger<'a> {
    version: Option<String>,
    tracks: Rows<Keyed<'a, model::Track>>,
    albums: Rows<Keyed<'a, model::Album>>,
    artists: Rows<model::Artist>,
    extras: Rows<model::Extra>,
    annotations: HashMap<Uuid, model::Annotation>,
    issues: HashMap<Uuid, Vec<(Uuid, String)>>,
    groupings: Vec<model::Grouping>,
    playlists: Vec<model::Playlist>,
}

impl<'a> Merger<'a> {
    /// From a whole catalog, its tracks and albums still text (`model::parse_base`).
    pub fn from_base(base: model::Base<'a>) -> anyhow::Result<Merger<'a>> {
        fn text<'a, T>(rows: Vec<&'a RawValue>) -> anyhow::Result<Vec<Keyed<'a, T>>> {
            rows.into_iter().map(|r| Ok((first_id(r)?, Pending::Text(r)))).collect()
        }
        Ok(Merger {
            version: base.version,
            tracks: Rows::new(text(base.tracks.rows)?, |r| r.0),
            albums: Rows::new(text(base.albums.rows)?, |r| r.0),
            artists: Rows::new(base.artists.rows, |a| a.id),
            extras: Rows::new(base.extras.rows, |e| e.id),
            annotations: base.annotations.rows.into_iter().map(|a| (a.id, a)).collect(),
            issues: base.issues.rows.into_iter().map(|i| (i.0, i.1)).collect(),
            groupings: base.groupings.rows,
            playlists: base.playlists,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    fn from_raw(raw: model::Raw) -> Merger<'static> {
        Merger {
            version: raw.version,
            tracks: Rows::new(raw.tracks.rows.into_iter().map(keyed(|t: &model::Track| t.id)).collect(), |r| r.0),
            albums: Rows::new(raw.albums.rows.into_iter().map(keyed(|a: &model::Album| a.id)).collect(), |r| r.0),
            artists: Rows::new(raw.artists.rows, |a| a.id),
            extras: Rows::new(raw.extras.rows, |e| e.id),
            annotations: raw.annotations.rows.into_iter().map(|a| (a.id, a)).collect(),
            issues: raw.issues.rows.into_iter().map(|i| (i.0, i.1)).collect(),
            groupings: raw.groupings.rows,
            playlists: raw.playlists,
        }
    }

    /// From a catalog, to apply a delta to (its rows aren't copied until built).
    fn from_catalog(c: &Catalog) -> Merger<'static> {
        fn kept<T>(ids: impl Iterator<Item = Uuid>) -> Vec<Keyed<'static, T>> {
            ids.enumerate().map(|(i, id)| (id, Pending::Kept(i as u32))).collect()
        }
        Merger {
            version: c.version.clone(),
            tracks: Rows::new(kept(c.tracks.iter().map(|t| t.id)), |r| r.0),
            albums: Rows::new(kept(c.albums.iter().map(|a| a.id)), |r| r.0),
            artists: Rows::new(c.artists.clone(), |a| a.id),
            extras: Rows::new(c.extras.clone(), |e| e.id),
            annotations: c.annotations.clone(),
            issues: c.issues.clone(),
            groupings: c.groupings.clone(),
            playlists: c.playlists.clone(),
        }
    }

    pub fn add(&mut self, delta: model::Raw) {
        let removed = &delta.removed;
        self.tracks.apply(delta.tracks.rows.into_iter().map(keyed(|t: &model::Track| t.id)).collect(), &removed.tracks, |r| r.0);
        self.albums.apply(delta.albums.rows.into_iter().map(keyed(|a: &model::Album| a.id)).collect(), &removed.albums, |r| r.0);
        self.artists.apply(delta.artists.rows, &removed.artists, |a| a.id);
        // A changed extra replaces its row (one taken off comes flagged, and is left out
        // when indexed).
        self.extras.apply(delta.extras.rows, &removed.extras, |e| e.id);
        for id in &removed.annotations {
            self.annotations.remove(id);
        }
        for a in delta.annotations.rows {
            self.annotations.insert(a.id, a);
        }
        // A changed track's flags replace its old ones (none left: an empty list).
        for id in &removed.tracks {
            self.issues.remove(id);
        }
        for model::Issues(track, flags) in delta.issues.rows {
            if flags.is_empty() {
                self.issues.remove(&track);
            } else {
                self.issues.insert(track, flags);
            }
        }
        if delta.version.is_some() {
            self.version = delta.version;
        }
        // Sent whole with every delta.
        self.groupings = delta.groupings.rows;
        self.playlists = delta.playlists;
    }

    /// The catalog. `old`: the one `Pending::Kept` rows are from. A row that doesn't read
    /// is an error (the copy on disk is then fetched again).
    pub fn build(self, old: Option<&Catalog>) -> anyhow::Result<Catalog> {
        let mut builder = rows::Builder::default();
        let pending = self.tracks.finish();
        let mut tracks = Vec::with_capacity(pending.len());
        for (_, row) in pending {
            tracks.push(builder.track(match row {
                Pending::Text(text) => serde_json::from_str(text.get()).with_context(|| format!("a track: {}", short(text)))?,
                Pending::Row(t) => *t,
                Pending::Kept(i) => old.context("no catalog to keep rows from")?.track_at(i).to_model(),
            }));
        }
        let pending = self.albums.finish();
        let mut albums = Vec::with_capacity(pending.len());
        for (_, row) in pending {
            albums.push(builder.album(match row {
                Pending::Text(text) => serde_json::from_str(text.get()).with_context(|| format!("an album: {}", short(text)))?,
                Pending::Row(a) => *a,
                Pending::Kept(i) => old.context("no catalog to keep rows from")?.album_at(i).to_model(),
            }));
        }
        let mut catalog = Catalog {
            version: self.version,
            etag: None,
            pools: builder.finish(),
            tracks: tracks.into(),
            albums: albums.into(),
            artists: self.artists.finish(),
            playlists: self.playlists,
            annotations: self.annotations,
            issues: self.issues.into_iter().filter(|i| !i.1.is_empty()).collect(),
            groupings: self.groupings,
            extras: self.extras.finish().into_iter().filter(|e| !e.hidden).collect(),
            extra_text: Texts::default(),
            track_by_id: HashMap::new(),
            album_by_id: HashMap::new(),
            artist_by_id: HashMap::new(),
            grouping_by_id: HashMap::new(),
            album_tracks: Lists::default(),
            artist_tracks: Lists::default(),
            artist_albums: Lists::default(),
            grouping_tracks: Lists::default(),
            grouping_albums: Lists::default(),
            track_text: Texts::default(),
            album_text: Texts::default(),
            artist_text: Texts::default(),
            track_title_rank: Col::default(),
            track_artist_rank: Col::default(),
            album_title_rank: Col::default(),
            album_artist_rank: Col::default(),
            artist_name_rank: Col::default(),
            track_album_rank: Col::default(),
            track_newest_rank: Col::default(),
        };
        catalog.index();
        Ok(catalog)
    }
}

/// A row's id (rows are compact arrays whose first element is the id).
fn first_id(row: &RawValue) -> anyhow::Result<Uuid> {
    let text = row.get();
    text.strip_prefix("[\"")
        .and_then(|rest| rest.get(..36))
        .and_then(|id| Uuid::parse_str(id).ok())
        .with_context(|| format!("a row that doesn't start with an id: {}", &text[..text.floor_char_boundary(40)]))
}

/// The start of a row's text, for an error.
fn short(text: &RawValue) -> &str {
    let text = text.get();
    &text[..text.floor_char_boundary(60)]
}

/// Newest release first (no date last), then newest added.
fn newest_first(a: Track, b: Track) -> Ordering {
    b.release_date().unwrap_or("").cmp(a.release_date().unwrap_or("")).then_with(|| b.id().cmp(&a.id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        // Two albums; track 3 is on both, first on "B" (its main one).
        let body = serde_json::json!({
            "format": 8,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate"],
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
        list.iter().map(|i| c.track_at(*i).title().to_string()).collect()
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
            "format": 8, "delta": true, "version": "10:20:",
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
        let mut titles: Vec<String> = c.tracks().map(|t| t.title().to_string()).collect();
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
        let body = br#"{"format": 9}"#;
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
        let base = model::parse_base(&body).unwrap();
        println!("parse: {:?}", t.elapsed());
        let t = Instant::now();
        let c = Merger::from_base(base).unwrap().build(None).unwrap();
        println!("rows read and indexed: {:?} ({} tracks)", t.elapsed(), c.track_count());
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
        let mut changed = c.track_at(5000).to_model();
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
