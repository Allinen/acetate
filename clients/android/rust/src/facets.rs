//! What the artist and album pages offer to narrow by: the names an artist is credited
//! under, original files, and the groupings something is part of.

use std::collections::HashMap;

use acetate_core::catalog::model::Role;
use uuid::Uuid;

use crate::groupings::GroupingRow;
use crate::{album_row, parse_id, AlbumRow, Core};

/// A name an artist is credited under, with on how many tracks (None: its own name).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NameCount {
    pub name: Option<String>,
    pub tracks: u64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ArtistFacets {
    /// Every name it's credited under: its own first, then the others, most used first.
    pub names: Vec<NameCount>,
    /// The other names it has, for when it's credited under only one.
    pub aliases: Vec<String>,
    /// Tracks of it that are original files.
    pub original_files: u64,
    /// Groupings it's a member of (said on the grouping).
    pub member_of: Vec<GroupingRow>,
    /// Groupings its tracks are in (the ones it isn't a member of), with how many tracks.
    pub tracks_in: Vec<GroupingCount>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GroupingCount {
    pub grouping: GroupingRow,
    pub tracks: u64,
}

/// What the artist page shows once narrowed.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ArtistNarrowed {
    pub albums: Vec<AlbumRow>,
    /// The tracks to play, in the albums' order.
    pub tracks: Vec<String>,
}

fn names_in(credits: acetate_core::catalog::Credits<'_>, artist: Uuid) -> impl Iterator<Item = Option<&str>> {
    credits.iter().filter(move |c| c.0 == artist).map(|c| c.2)
}

#[uniffi::export]
impl Core {
    pub fn artist_facets(&self, id: String) -> Option<ArtistFacets> {
        let catalog = self.shared.catalog()?;
        let id = parse_id(&id)?;
        let artist = catalog.artist(id)?;
        let tracks = catalog.artist_tracks(id);
        let mut counts: HashMap<Option<String>, u64> = HashMap::new();
        let mut in_groupings: Vec<(Uuid, u64)> = Vec::new();
        let mut original_files = 0;
        for &t in tracks {
            let track = catalog.track_at(t);
            let mut names: Vec<Option<&str>> = names_in(track.credits(), id).collect();
            names.dedup();
            for name in names {
                *counts.entry(name.map(str::to_string)).or_default() += 1;
            }
            if track.original_filename().is_some() {
                original_files += 1;
            }
            for g in track.groupings() {
                match in_groupings.iter_mut().find(|(i, _)| i == g) {
                    Some((_, n)) => *n += 1,
                    None => in_groupings.push((*g, 1)),
                }
            }
        }
        let mut names: Vec<NameCount> = counts.into_iter().map(|(name, tracks)| NameCount { name, tracks }).collect();
        names.sort_by(|a, b| a.name.is_some().cmp(&b.name.is_some()).then(b.tracks.cmp(&a.tracks)).then(a.name.cmp(&b.name)));
        let member_of: Vec<&acetate_core::catalog::model::Grouping> = catalog.groupings.iter().filter(|g| g.artists.contains(&id)).collect();
        in_groupings.retain(|(g, _)| !member_of.iter().any(|m| m.id == *g));
        in_groupings.sort_by_key(|g| std::cmp::Reverse(g.1));
        Some(ArtistFacets {
            names,
            aliases: artist.aliases.clone(),
            original_files,
            member_of: member_of.into_iter().map(crate::groupings::row).collect(),
            tracks_in: in_groupings
                .into_iter()
                .filter_map(|(g, tracks)| Some(GroupingCount { grouping: crate::groupings::row(catalog.grouping(g)?), tracks }))
                .collect(),
        })
    }

    /// The artist's releases (credited as primary under the names chosen, none: all) and the
    /// tracks to play, with only original files if asked. With nothing chosen: its albums,
    /// and every track on them.
    pub fn artist_narrowed(&self, id: String, names: Vec<Option<String>>, original_only: bool) -> ArtistNarrowed {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return ArtistNarrowed { albums: vec![], tracks: vec![] } };
        let narrowed = !names.is_empty() || original_only;
        let shows = |name: Option<&str>| names.is_empty() || names.iter().any(|n| n.as_deref() == name);
        let is_og = |t: u32| catalog.track_at(t).original_filename().is_some();
        let mut albums = Vec::new();
        let mut tracks = Vec::new();
        for &a in catalog.artist_albums(id) {
            let album = catalog.album_at(a);
            let in_album = catalog.album_tracks(album.id());
            if !narrowed {
                albums.push(album_row(album));
                tracks.extend(in_album.iter().map(|&t| catalog.track_at(t).id().to_string()));
                continue;
            }
            let mine: Vec<u32> = in_album
                .iter()
                .copied()
                .filter(|&t| names_in(catalog.track_at(t).credits(), id).any(&shows) && (!original_only || is_og(t)))
                .collect();
            let primary = album.credits().iter().any(|c| c.0 == id && c.1 == Role::Primary && shows(c.2));
            if primary && (!original_only || in_album.iter().any(|&t| is_og(t))) {
                albums.push(album_row(album));
            }
            tracks.extend(mine.into_iter().map(|t| catalog.track_at(t).id().to_string()));
        }
        ArtistNarrowed { albums, tracks }
    }

    /// An album's tracks that are original files.
    pub fn album_original_files(&self, id: String) -> Vec<String> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        catalog
            .album_tracks(id)
            .iter()
            .map(|&t| catalog.track_at(t))
            .filter(|t| t.original_filename().is_some())
            .map(|t| t.id().to_string())
            .collect()
    }

    /// The groupings an album is part of.
    pub fn album_groupings(&self, id: String) -> Vec<GroupingRow> {
        let (Some(catalog), Some(id)) = (self.shared.catalog(), parse_id(&id)) else { return vec![] };
        let Some(album) = catalog.album(id) else { return vec![] };
        album.groupings().iter().filter_map(|g| catalog.grouping(*g)).map(crate::groupings::row).collect()
    }
}
