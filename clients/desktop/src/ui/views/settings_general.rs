//! Settings, General: acetate:// links, the keys, the version.


use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use crate::account::Release;
use crate::update::{Progress, Update};
use crate::ui::task::Fetched;
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Action, Shell};

#[derive(Default)]
pub struct GeneralTab {
    /// Which app opens acetate:// links (the system's answer), asked in the background.
    links_handler: Fetched<crate::links::Handler>,
    update: Update,
}

impl GeneralTab {
    pub fn opened(&mut self, shell: &Shell) {
        self.links_handler.start(shell, async { tokio::task::spawn_blocking(crate::links::handler).await.unwrap_or(crate::links::Handler::Nobody) });
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        section(ui, "Links");
        use crate::links::Handler;
        let how = if crate::links::CAN_REGISTER { "“Open links here” sets that up" } else { "clients/desktop/install.sh sets that up" };
        let handler = self.links_handler.get().cloned();
        match &handler {
            Some(Handler::Here) => {
                ui.label(RichText::new("Links to tracks, albums and artists open here.").color(P.text));
            }
            Some(Handler::Other(other)) => {
                ui.label(RichText::new(format!("Links to tracks, albums and artists open in {other}, not here: {how}.")).color(P.danger));
            }
            Some(Handler::Nobody) => {
                ui.label(RichText::new(format!("Links to tracks, albums and artists don't open here yet: {how}.")).color(P.muted));
            }
            None => {
                ui.label(RichText::new("Checking…").color(P.faint));
            }
        }
        if crate::links::CAN_REGISTER && matches!(handler, Some(Handler::Other(_) | Handler::Nobody)) && ui.small_button("Open links here").clicked() {
            match crate::links::register() {
                Ok(()) => self.links_handler.set(crate::links::handler()),
                Err(err) => shell.controller.report(err),
            }
        }
        hint(ui, "Ctrl+L (or “Share” on a page) copies a link to the track, album or artist shown, for someone with an account here; Ctrl+Shift+L (or Shift+click “Share”) opens a popup to make a share link anyone can open: its expiry, and whether it plays the original file or a lower-quality copy. Opened by someone logged in, either one goes to the app (or the web UI, if the app doesn't take it).");

        section(ui, "Keys");
        if widgets::chip(ui, "Show every key  ?", false) {
            shell.act(Action::ShowKeys);
        }
        ui.add_space(24.0);
        let server = shell.account.server.clone();
        let line = match &server {
            Some(server) => format!("Acetate {} · server {}", env!("CARGO_PKG_VERSION"), server.version),
            None => format!("Acetate {}", env!("CARGO_PKG_VERSION")),
        };
        ui.label(RichText::new(line).color(P.faint).small());
        if let Some(server) = server {
            self.update(ui, shell, &server);
        }
    }

    /// The server's release against this app's: a button to bring the app up to it, or
    /// (the server is older) only the warning: the app doesn't go back.
    fn update(&mut self, ui: &mut egui::Ui, shell: &Shell, server: &crate::account::ServerVersion) {
        let progress = self.update.progress();
        match (server.release(), &progress) {
            (_, Progress::Done(version)) => {
                ui.label(RichText::new(format!("Updated to Acetate {version}. Restart Acetate to use it.")).color(P.text));
                return;
            }
            (Release::ServerNewer, Progress::Idle | Progress::Failed(_)) => {
                if let Some(warning) = server.warning() {
                    ui.label(RichText::new(warning).color(P.danger));
                }
                ui.add_space(6.0);
                if widgets::chip(ui, &format!("Update to {}", server.version), false) {
                    self.update.start(shell.runtime.handle(), server, &shell.waker);
                }
                hint(ui, "Downloads that release from GitHub and puts it in place of this program; it starts the next time Acetate does.");
            }
            _ => {
                if let Some(warning) = server.warning() {
                    ui.label(RichText::new(warning).color(P.danger));
                }
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
    }
}
