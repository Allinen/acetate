//! Extras becoming tracks from their YouTube videos: each is previewed once (the server
//! asks YouTube, `POST /api/extras/{id}/to-track` with `preview`), and the track editor
//! works over all the drafts, as it does for files that didn't import: what's staged
//! goes on every one, each keeping its own title and source. Importing makes the tracks
//! (one request each) and takes the extras off (kept).
//!
//! Titles are YouTube's as they are: the line starts with `parse:prefix feat prod` typed
//! (not staged: Enter stages it, and the check shows what each title becomes).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::ui::edit::editor::{DraftOf, Imported, TrackEditor};
use crate::ui::theme::PALETTE as P;
use crate::ui::Shell;

/// The rules the line starts with.
const PARSE: &str = "parse:prefix feat prod";

pub struct ExtraDrafts {
    /// The video's link, where the extra's name doesn't end with its `[id]`.
    links: HashMap<Uuid, String>,
    /// Each extra's draft as it's previewed (its track, or why there's none).
    drafts: HashMap<Uuid, Result<Uuid, String>>,
    asking: HashSet<Uuid>,
    arrived: Arc<Mutex<Vec<(Uuid, Result<Imported, String>)>>>,
    /// Why a draft has no cover.
    no_cover: HashMap<Uuid, String>,
    on_albums: HashSet<Uuid>,
    started: bool,
    editor: TrackEditor,
}

impl ExtraDrafts {
    pub fn new(links: HashMap<Uuid, String>) -> Self {
        ExtraDrafts {
            links,
            drafts: HashMap::new(),
            asking: HashSet::new(),
            arrived: Arc::default(),
            no_cover: HashMap::new(),
            on_albums: HashSet::new(),
            started: false,
            editor: TrackEditor::default(),
        }
    }

    /// Applying or applied: the extras can't change.
    pub fn busy(&self) -> bool {
        self.editor.applying_or_applied()
    }

    /// Shows the drafts of `extras` (their ids and names), the editor over them. The extras
    /// made tracks (extra, track), once.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, extras: &[(Uuid, String)]) -> Vec<(Uuid, Uuid)> {
        let arrived = std::mem::take(&mut *self.arrived.lock().unwrap());
        for (extra, answer) in arrived {
            self.asking.remove(&extra);
            let result = match answer {
                Ok(imported) => {
                    if let Some(why) = imported.no_cover.clone() {
                        self.no_cover.insert(extra, why);
                    }
                    match (imported.track, imported.draft) {
                        (Some(track), Some(ids)) => {
                            let id = track.id;
                            if track.albums.is_empty() {
                                self.on_albums.remove(&extra);
                            } else {
                                self.on_albums.insert(extra);
                            }
                            let flags = imported.edited.map(|e| e.flags).unwrap_or_default();
                            let link = self.links.get(&extra).filter(|l| !l.trim().is_empty());
                            self.editor.draft(DraftOf { problem: extra, path: format!("/api/extras/{extra}/to-track"), ids, with: serde_json::json!({ "link": link }), flags }, track);
                            Ok(id)
                        }
                        _ => Err("a track has this exact file already".to_string()),
                    }
                }
                Err(err) => Err(err),
            };
            self.drafts.insert(extra, result);
        }
        // The library changed since: previewed again.
        for extra in self.editor.take_stale_drafts() {
            self.drafts.remove(&extra);
        }
        // Each previewed once (a few at a time; the server asks YouTube once for each).
        if !self.editor.applying_or_applied() {
            let limit = Arc::new(tokio::sync::Semaphore::new(2));
            let ask: Vec<Uuid> = extras.iter().map(|e| e.0).filter(|id| !self.drafts.contains_key(id) && !self.asking.contains(id)).collect();
            for extra in ask {
                self.asking.insert(extra);
                let link = self.links.get(&extra).filter(|l| !l.trim().is_empty());
                let body = serde_json::json!({ "preview": true, "link": link });
                let (api, arrived, ctx, limit, extra) = (shell.api.clone(), self.arrived.clone(), shell.ctx.clone(), limit.clone(), extra);
                shell.runtime.spawn(async move {
                    let _permit = limit.acquire().await;
                    let answer = api.send_json::<Imported>(reqwest::Method::POST, &format!("/api/extras/{extra}/to-track"), Some(body)).await.map_err(|e| e.message);
                    arrived.lock().unwrap().push((extra, answer));
                    ctx.request_repaint();
                });
            }
        }
        let waiting = extras.iter().filter(|(id, _)| !self.drafts.contains_key(id)).count();
        if waiting > 0 {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.label(RichText::new(format!("Asking YouTube… {} of {}", extras.len() - waiting, extras.len())).color(P.faint));
            });
            return Vec::new();
        }
        let refused: Vec<String> = extras.iter().filter_map(|(id, name)| Some(format!("{name}: {}", self.drafts.get(id)?.as_ref().err()?))).collect();
        if !refused.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("{} of them left out", refused.len())).color(P.danger)).id_salt("extra-drafts-left-out").default_open(extras.len() == 1).show(ui, |ui| {
                for line in &refused {
                    ui.label(RichText::new(line).color(P.muted).small());
                }
            });
        }
        let without_cover: Vec<String> = extras.iter().filter(|(id, _)| matches!(self.drafts.get(id), Some(Ok(_)))).filter_map(|(id, name)| Some(format!("{name}: no cover, {}", self.no_cover.get(id)?))).collect();
        for line in &without_cover {
            ui.label(RichText::new(line).color(P.muted).small());
        }
        let targets: Vec<Uuid> = extras.iter().filter_map(|(id, _)| self.drafts.get(id)?.as_ref().ok().copied()).collect();
        if targets.is_empty() {
            return Vec::new();
        }
        if !self.started {
            self.started = true;
            // A loose track becomes a single of its own: when none of them is on an album.
            if extras.iter().all(|(id, _)| !self.on_albums.contains(id)) {
                self.editor.start_single();
            }
            self.editor.prefill(PARSE);
        }
        ui.label(RichText::new("YouTube's titles as they are: Enter on the line below moves “Artist -”, feat. and prod. out of them (look at the check before applying).").color(P.muted).small());
        self.editor.offer(&[]);
        self.editor.show(ui, shell, catalog, &targets, None);
        let imported = self.editor.take_imported();
        if !imported.is_empty() {
            shell.act(crate::ui::Action::RefreshLibrary);
        }
        imported
    }
}
