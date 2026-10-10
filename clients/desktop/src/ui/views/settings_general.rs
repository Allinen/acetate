//! Settings, General: acetate:// links, the keys, the version.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Action, Shell};

#[derive(Default)]
pub struct GeneralTab {
    /// Which app opens acetate:// links (the system's answer), asked in the background.
    links_handler: Arc<Mutex<Option<crate::links::Handler>>>,
}

impl GeneralTab {
    pub fn opened(&mut self, shell: &Shell) {
        let handler = self.links_handler.clone();
        shell.runtime.spawn_blocking(move || *handler.lock().unwrap() = Some(crate::links::handler()));
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        section(ui, "Links");
        use crate::links::Handler;
        let how = if crate::links::CAN_REGISTER { "“Open links here” sets that up" } else { "clients/desktop/install.sh sets that up" };
        let handler = self.links_handler.lock().unwrap().clone();
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
                Ok(()) => *self.links_handler.lock().unwrap() = Some(crate::links::handler()),
                Err(err) => shell.controller.report(err),
            }
        }
        hint(ui, "Ctrl+L (or “Share” on a page) copies a link to the track, album or artist shown, for someone with an account here; Ctrl+Shift+L (or Shift+click “Share”) opens a popup to make a share link anyone can open: its expiry, and whether it plays the original file or a lower-quality copy. Opened by someone logged in, either one goes to the app (or the web UI, if the app doesn't take it).");

        section(ui, "Keys");
        if widgets::chip(ui, "Show every key  ?", false) {
            shell.act(Action::ShowKeys);
        }
        ui.add_space(24.0);
        ui.label(RichText::new(format!("Acetate {}", env!("CARGO_PKG_VERSION"))).color(P.faint).small());
    }
}
