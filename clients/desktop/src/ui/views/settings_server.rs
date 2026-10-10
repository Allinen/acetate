//! Settings, Server (admins): what the server stores on each volume.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use super::settings::{hint, size};
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{thousands, Shell};

#[derive(Default)]
pub struct ServerTab {
    /// What the server stores on each volume (`GET /api/storage`), asked when the page opens.
    storage: Arc<Mutex<Option<Result<Vec<StorageVolume>, String>>>>,
}

#[derive(Clone, serde::Deserialize)]
struct StorageVolume {
    name: String,
    default: bool,
    parts: Vec<StoragePart>,
}

#[derive(Clone, serde::Deserialize)]
struct StoragePart {
    kind: String,
    blobs: i64,
    bytes: i64,
    unmeasured: i64,
}

impl ServerTab {
    pub fn opened(&mut self, shell: &Shell) {
        let (api, storage, ctx) = (shell.api.clone(), self.storage.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let answer = api.send_json::<Vec<StorageVolume>>(reqwest::Method::GET, "/api/storage", None).await.map_err(|e| e.message);
            *storage.lock().unwrap() = Some(answer);
            ctx.request_repaint();
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, _shell: &mut Shell) {
        match self.storage.lock().unwrap().clone() {
            None => {
                ui.label(RichText::new("Adding up…").color(P.faint));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
            }
            Some(Ok(volumes)) => {
                egui::Grid::new("storage").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                    for volume in &volumes {
                        let total: i64 = volume.parts.iter().map(|p| p.bytes).sum();
                        let name = if volume.default { format!("{} (new files go here)", volume.name) } else { volume.name.clone() };
                        ui.label(RichText::new(name).color(P.muted));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(size(total)).font(font(14.0, medium())));
                            for part in &volume.parts {
                                let unmeasured = if part.unmeasured > 0 { format!(" ({} not measured yet)", thousands(part.unmeasured as usize)) } else { String::new() };
                                ui.label(RichText::new(format!("{}: {} · {}{unmeasured}", part.kind, size(part.bytes), thousands(part.blobs as usize))).color(P.faint));
                            }
                        });
                        ui.end_row();
                    }
                });
                hint(ui, "Audio counts tracks' current and previous files; “other” is what nothing refers to anymore. Sizes the server never recorded (older covers) are filled in by acetate measure-covers.");
            }
        }
    }
}
