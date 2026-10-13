//! Shown instead of the app when it and the server aren't the same API version and catalog
//! format (`ServerVersion::compat`): they can't work together, and the server turns the app
//! away anyway. The way out depends on which is older:
//!
//! * the server is newer: this app is updated to the server's release (one button);
//! * the app is newer: the server needs updating (the app doesn't go back to an older
//!   release by itself: what it keeps may not be readable by one), and the page of the app
//!   that goes with the server is linked, to install by hand.

use eframe::egui::{self, RichText};

use crate::account::{Compat, ServerVersion};
use crate::ui::theme::{bold, font, PALETTE as P};
use crate::ui::{widgets, Action, Shell};
use crate::update::Progress;

#[derive(Default)]
pub struct Incompatible;

impl Incompatible {
    pub fn show(&self, ui: &mut egui::Ui, shell: &mut Shell, server: &ServerVersion) {
        let compat = server.compat();
        ui.add_space(60.0);
        ui.label(RichText::new("This app and the server aren't the same version").font(font(26.0, bold())));
        ui.add_space(10.0);
        let numbers = |api: u32, catalog: Option<u32>| match catalog {
            Some(c) => format!("API {api}, catalog {c}"),
            None => format!("API {api}"),
        };
        ui.label(RichText::new(format!("This app: Acetate {} ({})", env!("CARGO_PKG_VERSION"), numbers(crate::account::api_version(), Some(acetate_core::catalog::model::FORMAT)))).color(P.muted));
        ui.label(RichText::new(format!("The server: Acetate {} ({})", server.version, numbers(server.api, server.catalog_format))).color(P.muted));
        ui.add_space(14.0);
        let progress = shell.update.progress();
        if let Progress::Done(version) = &progress {
            ui.label(RichText::new(format!("Updated to Acetate {version}. Restart Acetate to use it.")).color(P.text));
            return;
        }
        match compat {
            Compat::AppBehind => {
                ui.label(RichText::new("The server is newer than this app. Update the app to the server's release to go on.").color(P.text));
                ui.add_space(8.0);
                let busy = shell.update.busy();
                ui.add_enabled_ui(!busy, |ui| {
                    if widgets::chip(ui, &format!("Update to {}", server.version), false) {
                        shell.update.start(shell.runtime.handle(), server, &shell.waker);
                    }
                });
                ui.label(RichText::new("Downloads that release from GitHub and puts it in place of this program; it starts the next time Acetate does.").color(P.faint).small());
            }
            _ => {
                ui.label(RichText::new(format!("The server is older than this app. Ask whoever runs it to update it to Acetate {} (or newer), or install the app that goes with it:", env!("CARGO_PKG_VERSION"))).color(P.text));
                ui.add_space(8.0);
                ui.hyperlink_to(format!("Acetate {} on GitHub", server.version), server.release_url());
                ui.label(RichText::new("(The app doesn't go back to an older release by itself: what it keeps on this computer may not be readable by it.)").color(P.faint).small());
            }
        }
        match progress {
            Progress::Downloading(done, Some(total)) if total > 0 => {
                ui.label(RichText::new(format!("Downloading… {} of {}", super::settings::size(done as i64), super::settings::size(total as i64))).color(P.muted));
            }
            Progress::Downloading(..) => {
                ui.label(RichText::new("Downloading…").color(P.muted));
            }
            Progress::Installing => {
                ui.label(RichText::new("Installing…").color(P.muted));
            }
            Progress::Failed(err) => {
                ui.label(RichText::new(format!("The update failed: {err}")).color(P.danger));
            }
            Progress::Idle | Progress::Done(_) => {}
        }
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            if widgets::chip(ui, "Check again", false) {
                shell.account.fetch(&shell.api, shell.runtime.handle(), &shell.waker);
            }
            if widgets::chip(ui, "Log out", false) {
                shell.act(Action::LogOut);
            }
        });
    }
}
