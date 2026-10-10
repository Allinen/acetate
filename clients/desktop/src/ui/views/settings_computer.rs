//! Settings, This computer: the downloads, the library kept here (how much, where, moving
//! it, checking it against the server) and the memory the app uses.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use super::settings::size;
use super::settings_mirror::MirrorSection;
use crate::ui::theme::PALETTE as P;
use crate::ui::{thousands, widgets, Action, Shell};

#[derive(Default)]
pub struct ComputerTab {
    /// The size of the kept covers (small, standard, and an earlier version's no longer
    /// used), measured in the background: (files, bytes) each.
    covers_size: Arc<Mutex<Option<[(usize, u64); 3]>>>,
}

impl ComputerTab {
    pub fn opened(&mut self, shell: &Shell) {
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

    /// `mirror` has the moving of the folder everything's kept in.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, mirror: &mut MirrorSection) {
        section(ui, "Downloads");
        match shell.offline.clone() {
            None => {
                ui.label(RichText::new("Log in to keep music on this computer.").color(P.faint));
            }
            Some(offline) => {
                let status = offline.status();
                let paused = if status.paused { " · stopped" } else { "" };
                let coming = if status.queued > 0 { format!(" · {} coming", status.queued) } else { String::new() };
                ui.label(format!("{} files · {}{coming}{paused}", status.files, size(status.bytes as i64)));
                hint(ui, "Download from an album's, playlist's or track's page. It plays from here, with or without the server, and follows the quality chosen above.");
                if status.failed > 0 {
                    ui.label(RichText::new(format!("{} couldn't be fetched", status.failed)).color(P.danger));
                }
                if status.files > 0 || !offline.pins().is_empty() {
                    ui.add_space(6.0);
                    if widgets::chip(ui, "Remove all downloads (Shift+click)", false) && ui.input(|i| i.modifiers.shift) {
                        offline.remove_all();
                    }
                }
            }
        }

        section(ui, "Library on this computer");
        let catalog = shell.catalog.value.clone();
        egui::Grid::new("library").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            if let Some(c) = &catalog {
                ui.label(RichText::new("Kept here").color(P.muted));
                ui.label(format!(
                    "{} tracks · {} albums · {} artists · {} playlists",
                    thousands(c.track_count()),
                    thousands(c.album_count()),
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
                ui.label(RichText::new("Everything: settings, the library, playlists, the mirror").color(P.faint).small());
                ui.add(egui::Label::new(RichText::new(shell.cache_dir.display().to_string()).monospace().small()).selectable(true));
                ui.label(RichText::new("Scaled covers (a cache, made again when needed)").color(P.faint).small());
            });
            ui.end_row();
        });
        mirror.show_move(ui, shell);
        ui.add_space(6.0);
        if widgets::chip(ui, "Check for changes now  F5", false) {
            shell.act(Action::RefreshLibrary);
        }
        hint(ui, "The library shown is the copy kept here: it's there at once, and the server is asked for changes in the background (the server's copy wins).");

        section(ui, "Memory");
        self.memory(ui, shell);
    }

    /// What the app holds: in all (measured), and its big parts.
    fn memory(&mut self, ui: &mut egui::Ui, shell: &Shell) {
        let mb = |bytes: u64| format!("{} MB", thousands((bytes / 1_000_000) as usize));
        egui::Grid::new("memory").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            if let Some((now, peak)) = crate::memory::resident() {
                ui.label(RichText::new("In use").color(P.muted));
                ui.label(format!("{} (at most {} since it started)", mb(now), mb(peak)));
                ui.end_row();
            }
            if let Some(c) = &shell.catalog.value {
                // Measured once on a real library (`catalog::measure`): about 810 bytes a
                // track with everything.
                ui.label(RichText::new("The library").color(P.muted));
                ui.label(format!("about {} ({} tracks; twice that for a moment while it's refreshed)", mb(c.track_count() as u64 * 810), thousands(c.track_count())));
                ui.end_row();
            }
            let (covers, cover_bytes) = shell.covers.textures();
            ui.label(RichText::new("Covers shown").color(P.muted));
            ui.label(format!("{} ({} images, let go when not shown for a while)", mb(cover_bytes as u64), thousands(covers)));
            ui.end_row();
            ui.label(RichText::new("Playing").color(P.muted));
            let held = crate::playback::stream::HELD.load(std::sync::atomic::Ordering::Relaxed);
            ui.label(format!("{} (what plays and what's next, each file whole)", mb(held)));
            ui.end_row();
        });
        if ui.small_button("Give freed memory back now").clicked() {
            crate::memory::trim();
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        }
        hint(ui, "Freed memory is given back to the system after the library is refreshed, when covers are let go, and once a minute.");
    }
}
