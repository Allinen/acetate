//! Review flags (`GET /api/review`): tracks someone should look at. Ingest leaves them
//! (fix later, possible duplicates, ambiguous artist names), and so do merges. All open
//! flags are fetched (500 at a time), kept on disk and shown from there at once; the
//! server's list replaces them when it arrives. Resolving one takes it out right away,
//! and puts it back if the server refuses.

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

    /// As the page says it.
    pub fn kind_name(&self) -> &str {
        match self.kind.as_str() {
            "fix_later" => "Fix later",
            "possible_duplicate" => "Possible duplicate",
            "ambiguous_artist" => "Ambiguous artist",
            "merged_artist" => "Artist merged",
            other => other,
        }
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
    /// Flags resolved here since the list was last asked for: a list that was being read
    /// meanwhile may still have them.
    resolved: std::collections::HashSet<Uuid>,
    /// Resolves the server refused: (the flag, the error), to put back.
    refused: Arc<Mutex<Vec<(Flag, String)>>>,
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
        self.resolved.clear();
        let (api, arrived, ctx, number) = (api.clone(), self.arrived.clone(), ctx.clone(), self.asked);
        runtime.spawn(async move {
            let result = async {
                let mut all: Vec<Flag> = Vec::new();
                loop {
                    let before = all.last().map(|f| format!("&before={}", f.id)).unwrap_or_default();
                    let page: Vec<Flag> = api.send_json(reqwest::Method::GET, &format!("/api/review?limit={PAGE}{before}"), None).await?;
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
                    list.retain(|f| !self.resolved.contains(&f.id));
                    self.list = Arc::new(list);
                    self.version += 1;
                    self.error = None;
                    self.keep();
                }
                Err(err) => self.error = Some(err),
            }
        }
        for (flag, err) in std::mem::take(&mut *self.refused.lock().unwrap()) {
            let mut list = (*self.list).clone();
            list.push(flag);
            list.sort_by_key(|f| std::cmp::Reverse(f.id));
            self.list = Arc::new(list);
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
        let (gone, kept): (Vec<Flag>, Vec<Flag>) = self.list.iter().cloned().partition(|f| ids.contains(&f.id));
        if gone.is_empty() {
            return;
        }
        self.list = Arc::new(kept);
        self.version += 1;
        self.keep();
        self.resolved.extend(ids);
        for flag in gone {
            let (api, refused, resolution) = (api.clone(), self.refused.clone(), resolution.to_string());
            runtime.spawn(async move {
                let path = format!("/api/review/{}/resolve", flag.id);
                let result = api.send(reqwest::Method::POST, &path, Some(serde_json::json!({ "resolution": resolution }))).await;
                match result {
                    // Already resolved (by someone else): gone either way.
                    Err(err) if err.status == Some(404) => {}
                    Err(err) => refused.lock().unwrap().push((flag, err.message)),
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
