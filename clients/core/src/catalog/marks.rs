//! The user's own stars, ratings and play counts ("marks"), as shown: the catalog's,
//! with this session's changes on top.
//!
//! A change shows at once (kept here) and goes to the server; its answer replaces it.
//! When a catalog arrives from the server that was asked for after a change, the
//! catalog has it too, so the change is forgotten here: the server's copy wins, as
//! everywhere. A change goes to the server through the outbox (see `outbox`): one still
//! waiting there stays, and one is counted as made when it reached the server (`touch`).

use std::collections::HashMap;
use std::time::Instant;

use uuid::Uuid;

use super::Catalog;

/// What a mark is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl Kind {
    /// Its part of the API's paths.
    pub fn path(self) -> &'static str {
        match self {
            Kind::Track => "tracks",
            Kind::Album => "albums",
            Kind::Artist => "artists",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mark {
    pub play_count: i32,
    /// 0 to 10 (half stars); 0: not rated.
    pub rating: i16,
    pub starred: bool,
}

#[derive(Default)]
pub struct Marks {
    /// This session's changes, with when they were made.
    local: HashMap<Uuid, (Mark, Instant)>,
}

impl Marks {
    pub fn get(&self, catalog: Option<&Catalog>, id: Uuid) -> Mark {
        if let Some((mark, _)) = self.local.get(&id) {
            return *mark;
        }
        catalog
            .and_then(|c| c.annotations.get(&id))
            .map(|a| Mark { play_count: a.play_count, rating: a.rating, starred: a.starred_at.is_some() })
            .unwrap_or_default()
    }

    pub fn set(&mut self, id: Uuid, mark: Mark) {
        self.local.insert(id, (mark, Instant::now()));
    }

    /// A change reached the server now (it was made earlier, offline perhaps).
    pub fn touch(&mut self, id: Uuid) {
        if let Some((_, at)) = self.local.get_mut(&id) {
            *at = Instant::now();
        }
    }

    /// A catalog asked for at `asked` arrived from the server: it has every change that
    /// reached the server before then (not the ones in `waiting`, still to be sent).
    pub fn catalog_arrived(&mut self, asked: Instant, waiting: &std::collections::HashSet<Uuid>) {
        self.local.retain(|id, (_, at)| *at > asked || waiting.contains(id));
    }

    /// Ids starred (or unstarred) in this session, newest first.
    pub fn changed_stars(&self) -> Vec<(Uuid, bool)> {
        let mut changed: Vec<_> = self.local.iter().map(|(id, (mark, at))| (*id, mark.starred, *at)).collect();
        changed.sort_by_key(|c| std::cmp::Reverse(c.2));
        changed.into_iter().map(|(id, starred, _)| (id, starred)).collect()
    }

    /// The starred tracks (as indexes into the catalog), newest first: starred in this
    /// session, then by when they were starred. Unstarred in this session: left out.
    pub fn liked(&self, catalog: &Catalog) -> Vec<u32> {
        let mut changed: Vec<_> = self
            .local
            .iter()
            .filter(|(id, (mark, _))| mark.starred && !catalog.annotations.get(id).is_some_and(|a| a.starred_at.is_some()))
            .filter_map(|(id, (_, at))| Some((catalog.track_index(*id)?, *at)))
            .collect();
        changed.sort_by_key(|c| std::cmp::Reverse(c.1));
        let mut tracks: Vec<u32> = changed.into_iter().map(|(t, _)| t).collect();
        let mut earlier: Vec<(&str, u32)> = catalog
            .annotations
            .values()
            .filter(|a| a.kind == "track")
            .filter_map(|a| Some((a.starred_at.as_deref()?, catalog.track_index(a.id)?)))
            .filter(|(_, t)| self.get(Some(catalog), catalog.track_at(*t).id()).starred)
            .collect();
        earlier.sort_by(|a, b| b.0.cmp(a.0));
        tracks.extend(earlier.into_iter().map(|(_, t)| t));
        tracks
    }

    pub fn clear(&mut self) {
        self.local.clear();
    }
}
