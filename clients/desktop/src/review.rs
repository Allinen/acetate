//! Review flags (`GET /api/review`): tracks someone should look at. Ingest leaves them
//! (fix later, possible duplicates, ambiguous artist names), merges do, and the server
//! raises its own for what a track is missing ("automatic": no cover…).
//!
//! Which tracks have which open flags comes with the catalog (`Catalog::issues`). This
//! keeps the details of the others (notes, related tracks and artists, where the file
//! came from): all open ones but the automatic kinds, fetched 500 at a time, kept on disk
//! and shown from there at once; the server's list replaces them when it arrives.
//! Resolving flags takes them out right away (`done`), and puts them back if the server
//! refuses. Many are resolved in one request (the bulk tools resolve thousands).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::Api;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flag {
    pub id: Uuid,
    /// "fix_later", "possible_duplicate", "ambiguous_artist", "merged_artist".
    pub kind: String,
    pub track_id: Uuid,
    #[serde(default)]
    pub track_title: String,
    #[serde(default)]
    pub track_artist: String,
    /// Per kind: `note` (fix later), `reason`, `name`/`chosen` (ambiguous artist)…
    #[serde(default)]
    pub related: serde_json::Value,
    /// For possible duplicates: `[{id, title, artist_display}]`.
    #[serde(default)]
    pub related_tracks: serde_json::Value,
    /// For ambiguous artists: the candidates; for merges, both artists.
    #[serde(default)]
    pub related_artists: serde_json::Value,
    /// Where the file was imported from (a path), if known.
    pub imported_from: Option<String>,
}

impl Flag {
    /// The importer's note (fix later), or "".
    pub fn note(&self) -> &str {
        self.related.get("note").and_then(|n| n.as_str()).unwrap_or("")
    }
}

const PAGE: usize = 500;

#[derive(Default)]
pub struct Flags {
    pub list: Arc<Vec<Flag>>,
    /// Counts changes to `list` (pages rebuild their rows when it changes).
    pub version: u64,
    pub loading: bool,
    pub error: Option<String>,
    path: Option<PathBuf>,
    asked: u64,
    arrived: Arc<Mutex<Option<(u64, Result<Vec<Flag>, String>)>>>,
    /// Flags resolved here (this session): left out of what's shown until the server's
    /// lists (this one, and the catalog's) no longer have them.
    pub done: HashSet<Uuid>,
    /// Resolves the server refused: (the flags, the details this list had of them, the
    /// error), to put back.
    refused: Arc<Mutex<Vec<(Vec<Uuid>, Vec<Flag>, String)>>>,
}

impl Flags {
    /// Starts over for a login: `path` is where this server's and user's copy is kept.
    pub fn reset(&mut self, path: Option<PathBuf>) {
        let kept = path.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok());
        *self = Flags { path, asked: self.asked + 1, ..Flags::default() };
        self.list = Arc::new(kept.unwrap_or_default());
        self.version += 1;
    }

    /// Fetches every open flag in the background.
    pub fn refresh(&mut self, api: &Api, runtime: &tokio::runtime::Handle, ctx: &eframe::egui::Context) {
        self.asked += 1;
        self.loading = true;
        let (api, arrived, ctx, number) = (api.clone(), self.arrived.clone(), ctx.clone(), self.asked);
        runtime.spawn(async move {
            let result = async {
                let mut all: Vec<Flag> = Vec::new();
                loop {
                    let before = all.last().map(|f| format!("&before={}", f.id)).unwrap_or_default();
                    let page: Vec<Flag> = api.send_json(reqwest::Method::GET, &format!("/api/review?automatic=false&limit={PAGE}{before}"), None).await?;
                    let done = page.len() < PAGE;
                    all.extend(page);
                    if done {
                        return Ok(all);
                    }
                }
            }
            .await
            .map_err(|err: crate::api::ApiError| err.message);
            *arrived.lock().unwrap() = Some((number, result));
            ctx.request_repaint();
        });
    }

    /// Takes in what arrived. Call once per frame; returns errors to show.
    pub fn poll(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        if let Some((number, result)) = self.arrived.lock().unwrap().take()
            && number == self.asked
        {
            self.loading = false;
            match result {
                Ok(mut list) => {
                    list.retain(|f| !self.done.contains(&f.id));
                    self.list = Arc::new(list);
                    self.version += 1;
                    self.error = None;
                    self.keep();
                }
                Err(err) => self.error = Some(err),
            }
        }
        let refused = std::mem::take(&mut *self.refused.lock().unwrap());
        for (ids, kept, err) in refused {
            self.put_back(&ids, kept);
            let what = if ids.len() == 1 { "it".to_string() } else { format!("{} flags", ids.len()) };
            errors.push(format!("Couldn't mark {what} done. {err}"));
        }
        errors
    }

    /// Writes the list to disk (errors don't matter: it's a copy, the server has them).
    fn keep(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_vec(&*self.list).unwrap_or_default());
    }

    /// Marks these flags done (`resolution`: what was done), taking them out at once.
    /// Sent many at a time (`POST /api/review/resolve`); a batch the server refuses comes
    /// back (see `poll`).
    pub fn resolve(&mut self, api: &Api, runtime: &tokio::runtime::Handle, ids: &[Uuid], resolution: &str) {
        let ids: Vec<Uuid> = ids.iter().filter(|id| !self.done.contains(id)).copied().collect();
        let gone = self.take_out(&ids);
        for (batch, flags) in batches(ids, gone) {
            let (api, refused, resolution) = (api.clone(), self.refused.clone(), resolution.to_string());
            runtime.spawn(async move {
                let body = serde_json::json!({ "flag_ids": batch, "resolution": resolution });
                // How many were: fewer when some were resolved already (by someone else, or
                // by the server: fixed), gone either way.
                if let Err(err) = api.send_json::<serde_json::Value>(reqwest::Method::POST, "/api/review/resolve", Some(body)).await {
                    refused.lock().unwrap().push((batch, flags, err.message));
                }
            });
        }
    }

    /// Takes these flags out at once (done here, or by the server for them: settling
    /// ambiguous artists), returning the details this list had. `put_back` undoes it.
    pub fn take_out(&mut self, ids: &[Uuid]) -> Vec<Flag> {
        if ids.is_empty() {
            return Vec::new();
        }
        let set: HashSet<Uuid> = ids.iter().copied().collect();
        let (gone, kept): (Vec<Flag>, Vec<Flag>) = self.list.iter().cloned().partition(|f| set.contains(&f.id));
        self.list = Arc::new(kept);
        self.version += 1;
        self.keep();
        self.done.extend(set);
        gone
    }

    /// Puts flags taken out back (the server refused).
    pub fn put_back(&mut self, ids: &[Uuid], flags: Vec<Flag>) {
        for id in ids {
            self.done.remove(id);
        }
        if !flags.is_empty() {
            let mut list = (*self.list).clone();
            list.extend(flags);
            list.sort_by_key(|f| std::cmp::Reverse(f.id));
            self.list = Arc::new(list);
            self.keep();
        }
        self.version += 1;
    }
}

/// How many flags one request resolves (the server takes up to 10,000).
const BATCH: usize = 5_000;

/// `ids` in batches, each with the details of its flags.
fn batches(ids: Vec<Uuid>, mut flags: Vec<Flag>) -> Vec<(Vec<Uuid>, Vec<Flag>)> {
    ids.chunks(BATCH)
        .map(|batch| {
            let set: HashSet<&Uuid> = batch.iter().collect();
            let (these, rest): (Vec<Flag>, Vec<Flag>) = std::mem::take(&mut flags).into_iter().partition(|f| set.contains(&f.id));
            flags = rest;
            (batch.to_vec(), these)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server's list (saved with `curl …/api/review > /tmp/flags.json`), if there.
    #[test]
    #[ignore]
    fn flags_from_the_server_parse() {
        let Ok(body) = std::fs::read("/tmp/flags.json") else { return };
        let flags: Vec<Flag> = serde_json::from_slice(&body).unwrap();
        assert!(flags.iter().any(|f| f.kind == "fix_later" && !f.note().is_empty() && f.imported_from.is_some()));
    }
}
