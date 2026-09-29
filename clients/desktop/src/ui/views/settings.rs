//! Settings: the sound output and volume, the account, and the library kept on this
//! computer (how much, where, and checking it against the server).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Shell};

#[derive(Default)]
pub struct SettingsPage {
    /// The sound outputs there are, listed in the background when the page opens (it can
    /// take a moment).
    outputs: Arc<Mutex<Option<Vec<String>>>>,
    /// The size of the kept covers (small, standard, and an earlier version's no longer
    /// used), measured in the background: (files, bytes) each.
    covers_size: Arc<Mutex<Option<[(usize, u64); 3]>>>,
}

impl SettingsPage {
    pub fn opened(&mut self, shell: &Shell) {
        let outputs = self.outputs.clone();
        shell.runtime.spawn_blocking(move || *outputs.lock().unwrap() = Some(crate::playback::engine::outputs()));
        let (size, dir) = (self.covers_size.clone(), shell.cache_dir.clone());
        shell.runtime.spawn_blocking(move || {
            let measure = |name: &str| {
                let files: Vec<u64> = std::fs::read_dir(dir.join(name))
                    .map(|entries| entries.filter_map(|e| e.ok()?.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).collect())
                    .unwrap_or_default();
                (files.len(), files.iter().sum())
            };
            *size.lock().unwrap() = Some([measure("covers-small"), measure("covers-standard"), measure("covers")]);
        });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
        ui.label(RichText::new("Settings").font(font(30.0, bold())));
        ui.add_space(12.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Sound");
            egui::Grid::new("sound").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                ui.label(RichText::new("Output").color(P.muted));
                let outputs = self.outputs.lock().unwrap().clone();
                let current = shell.settings.output.clone();
                let shown = current.clone().unwrap_or_else(|| "The system's default".into());
                egui::ComboBox::from_id_salt("output").selected_text(shown).width(320.0).show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "The system's default").clicked() {
                        shell.act(Action::SetOutput(None));
                    }
                    match &outputs {
                        None => {
                            ui.label(RichText::new("Looking for outputs…").color(P.faint));
                        }
                        Some(names) => {
                            for name in names {
                                if ui.selectable_label(current.as_deref() == Some(name.as_str()), name).clicked() {
                                    shell.act(Action::SetOutput(Some(name.clone())));
                                }
                            }
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Volume").color(P.muted));
                ui.horizontal(|ui| {
                    let mut volume = player.volume;
                    let slider = egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false);
                    if ui.add_sized([240.0, 20.0], slider).changed() {
                        shell.act(Action::SetVolume(volume));
                    }
                    ui.label(RichText::new(format!("{}%", (player.volume * 100.0).round())).color(P.muted));
                    let mut muted = player.muted;
                    if ui.checkbox(&mut muted, "Muted").changed() {
                        shell.act(Action::ToggleMute);
                    }
                });
                ui.end_row();
            });
            hint(ui, "9 and 0 turn it down and up, m mutes, anywhere outside a text field.");

            section(ui, "Account");
            let session = shell.api.session();
            egui::Grid::new("account").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                if let Some(session) = &session {
                    ui.label(RichText::new("Server").color(P.muted));
                    ui.label(&session.server);
                    ui.end_row();
                    ui.label(RichText::new("Logged in as").color(P.muted));
                    ui.label(&session.username);
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            if widgets::chip(ui, "Log out", false) {
                shell.act(Action::LogOut);
            }

            section(ui, "Library on this computer");
            let catalog = shell.catalog.value.clone();
            egui::Grid::new("library").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
                if let Some(c) = &catalog {
                    ui.label(RichText::new("Kept here").color(P.muted));
                    ui.label(format!(
                        "{} tracks · {} albums · {} artists · {} playlists",
                        thousands(c.tracks.len()),
                        thousands(c.albums.len()),
                        thousands(c.artists.len()),
                        thousands(c.playlists.len())
                    ));
                    ui.end_row();
                }
                ui.label(RichText::new("State").color(P.muted));
                ui.label(shell.library_status());
                ui.end_row();
                ui.label(RichText::new("Covers kept").color(P.muted));
                let measured = *self.covers_size.lock().unwrap();
                let mb = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1_000_000.0);
                match measured {
                    Some([small, standard, _]) => ui.label(format!(
                        "{} small ({}) · {} standard ({})",
                        thousands(small.0),
                        mb(small.1),
                        thousands(standard.0),
                        mb(standard.1)
                    )),
                    None => ui.label(RichText::new("Measuring…").color(P.faint)),
                };
                ui.end_row();
                // Copies an earlier version made (one size, scaled here), no longer used.
                if let Some([_, _, (count, bytes)]) = measured
                    && count > 0
                {
                    ui.label(RichText::new("No longer used").color(P.muted));
                    ui.label(format!(
                        "{} covers from an earlier version ({}), in {}: safe to delete",
                        thousands(count),
                        mb(bytes),
                        shell.cache_dir.join("covers").display()
                    ));
                    ui.end_row();
                }
                ui.label(RichText::new("Where").color(P.muted));
                ui.vertical(|ui| {
                    ui.add(egui::Label::new(RichText::new(shell.data_dir.display().to_string()).monospace().small()).selectable(true));
                    ui.add(egui::Label::new(RichText::new(shell.cache_dir.display().to_string()).monospace().small()).selectable(true));
                });
                ui.end_row();
            });
            ui.add_space(6.0);
            if widgets::chip(ui, "Check for changes now  F5", false) {
                shell.act(Action::RefreshLibrary);
            }
            hint(ui, "The library shown is the copy kept here: it's there at once, and the server is asked for changes in the background (the server's copy wins).");

            section(ui, "Keys");
            if widgets::chip(ui, "Show every key  ?", false) {
                shell.act(Action::ShowKeys);
            }
            ui.add_space(24.0);
            ui.label(RichText::new(format!("Acetate {}", env!("CARGO_PKG_VERSION"))).color(P.faint).small());
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).font(font(18.0, bold())));
    ui.add_space(8.0);
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(font(12.5, medium())).color(P.faint));
}
