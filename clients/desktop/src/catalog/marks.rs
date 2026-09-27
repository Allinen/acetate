//! The user's own stars, ratings and play counts ("marks"), as shown: the catalog's,
//! with this session's changes on top.
//!
//! A change shows at once (kept here) and goes to the server; its answer replaces it.
//! When a catalog arrives from the server that was asked for after a change, the
//! catalog has it too, so the change is forgotten here: the server's copy wins, as
//! everywhere.

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

    /// A catalog asked for at `asked` arrived from the server: it has every change made
    /// before then.
    pub fn catalog_arrived(&mut self, asked: Instant) {
        self.local.retain(|_, (_, at)| *at > asked);
    }

    /// Ids starred (or unstarred) in this session, newest first.
    pub fn changed_stars(&self) -> Vec<(Uuid, bool)> {
        let mut changed: Vec<_> = self.local.iter().map(|(id, (mark, at))| (*id, mark.starred, *at)).collect();
        changed.sort_by(|a, b| b.2.cmp(&a.2));
        changed.into_iter().map(|(id, starred, _)| (id, starred)).collect()
    }

    pub fn clear(&mut self) {
        self.local.clear();
    }
}
