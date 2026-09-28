//! Review flags (`GET /api/review`): tracks someone should look at. Ingest leaves them
//! (fix later, possible duplicates, ambiguous artist names), merges do, and the server
//! raises its own for what a track is missing ("automatic": no cover…).
//!
//! Which tracks have which open flags comes with the catalog (`Catalog::issues`). This
//! keeps the details of the others (notes, related tracks and artists, where the file
//! came from): all open ones but the automatic kinds, fetched 500 at a time, kept on disk
//! and shown from there at once; the server's list replaces them when it arrives.
//! Resolving flags takes them out right away (`done`), and puts them back if the server
//! refuses.

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
    pub done: std::collections::HashSet<Uuid>,
    /// Resolves the server refused: (the flag, its details if this list had them, the
    /// error), to put back.
    refused: Arc<Mutex<Vec<(Uuid, Option<Flag>, String)>>>,
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
        for (flag, kept, err) in std::mem::take(&mut *self.refused.lock().unwrap()) {
            self.done.remove(&flag);
            if let Some(kept) = kept {
                let mut list = (*self.list).clone();
                list.push(kept);
                list.sort_by_key(|f| std::cmp::Reverse(f.id));
                self.list = Arc::new(list);
            }
            self.version += 1;
            errors.push(format!("Couldn't mark it done. {err}"));
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
    pub fn resolve(&mut self, api: &Api, runtime: &tokio::runtime::Handle, ids: &[Uuid], resolution: &str) {
        let ids: Vec<Uuid> = ids.iter().filter(|id| !self.done.contains(id)).copied().collect();
        if ids.is_empty() {
            return;
        }
        let (gone, kept): (Vec<Flag>, Vec<Flag>) = self.list.iter().cloned().partition(|f| ids.contains(&f.id));
        self.list = Arc::new(kept);
        self.version += 1;
        self.keep();
        self.done.extend(&ids);
        for id in ids {
            let details = gone.iter().find(|f| f.id == id).cloned();
            let (api, refused, resolution) = (api.clone(), self.refused.clone(), resolution.to_string());
            runtime.spawn(async move {
                let path = format!("/api/review/{id}/resolve");
                let result = api.send(reqwest::Method::POST, &path, Some(serde_json::json!({ "resolution": resolution }))).await;
                match result {
                    // Already resolved (by someone else, or by the server: fixed): gone either way.
                    Err(err) if err.status == Some(404) => {}
                    Err(err) => refused.lock().unwrap().push((id, details, err.message)),
                    Ok(()) => {}
                }
            });
        }
    }
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
