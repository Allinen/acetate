//! Settings, Account: the server, who's logged in, logging out.

use eframe::egui::{self, RichText};

use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Action, Shell};

pub fn show(ui: &mut egui::Ui, shell: &mut Shell) {
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
}
