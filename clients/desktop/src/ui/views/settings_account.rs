//! Settings, Account: the server (its address can be changed, to the same server), who's
//! logged in, logging out.

use std::sync::{Arc, Mutex};

use acetate_core::api::AddressCheck;
use eframe::egui::{self, RichText};

use super::settings::hint;
use crate::ui::theme::PALETTE as P;
use crate::ui::{widgets, Action, Shell};

#[derive(Default)]
pub struct AccountTab {
    /// The new address being typed; None: not changing it.
    editing: Option<String>,
    /// What the server at `checked` answered; None while it's asked.
    checked: Option<(String, Arc<Mutex<Option<AddressCheck>>>)>,
    /// The address was changed just now (until the next edit).
    changed: bool,
    /// Why the change failed, if it did.
    failed: Arc<Mutex<Option<String>>>,
}

impl AccountTab {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let session = shell.api.session();
        egui::Grid::new("account").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            if let Some(session) = &session {
                ui.label(RichText::new("Server").color(P.muted));
                ui.horizontal(|ui| {
                    ui.label(&session.server);
                    if self.editing.is_none() && widgets::chip(ui, "Change address", false) {
                        self.editing = Some(session.server.clone());
                        self.checked = None;
                        self.changed = false;
                        *self.failed.lock().unwrap() = None;
                    }
                });
                ui.end_row();
                ui.label(RichText::new("Logged in as").color(P.muted));
                ui.label(&session.username);
                ui.end_row();
            }
        });
        if let Some(address) = self.editing.clone() {
            self.editor(ui, shell, address);
        } else if let Some(why) = self.failed.lock().unwrap().clone() {
            ui.add_space(6.0);
            ui.label(RichText::new(why).color(P.danger));
        } else if self.changed {
            ui.add_space(6.0);
            ui.label(RichText::new("The address is changed. Nothing else was: the library, downloads and queue are the same.").color(P.muted));
        }
        ui.add_space(12.0);
        if widgets::chip(ui, "Log out", false) {
            shell.act(Action::LogOut);
        }
    }

    /// The address field, the check of it, and using it.
    fn editor(&mut self, ui: &mut egui::Ui, shell: &mut Shell, mut address: String) {
        ui.add_space(8.0);
        let before = address.clone();
        let field = ui.add(egui::TextEdit::singleline(&mut address).desired_width(360.0).hint_text("https://music.example.com"));
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if address != before {
            // Another address than the one asked about.
            self.checked = None;
            *self.failed.lock().unwrap() = None;
        }
        let answer = self.checked.as_ref().filter(|(asked, _)| *asked == address.trim().trim_end_matches('/')).map(|(_, answer)| answer.lock().unwrap().clone());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            match &answer {
                Some(Some(AddressCheck::Same)) => {
                    if widgets::chip(ui, "Use this address", true) || entered {
                        let (api, failed, ctx, target) = (shell.api.clone(), self.failed.clone(), ui.ctx().clone(), address.clone());
                        shell.runtime.spawn(async move {
                            if let Err(why) = api.change_address(&target).await {
                                *failed.lock().unwrap() = Some(why);
                            }
                            ctx.request_repaint();
                        });
                        self.changed = true;
                        self.editing = None;
                        self.checked = None;
                        return;
                    }
                }
                Some(None) => {
                    ui.label(RichText::new("Asking…").color(P.faint));
                }
                _ => {
                    if (widgets::chip(ui, "Check", true) || entered) && !address.trim().is_empty() {
                        let slot = Arc::new(Mutex::new(None));
                        let (api, out, ctx, target) = (shell.api.clone(), slot.clone(), ui.ctx().clone(), address.trim().trim_end_matches('/').to_string());
                        self.checked = Some((target.clone(), slot));
                        shell.runtime.spawn(async move {
                            *out.lock().unwrap() = Some(api.check_address(&target).await);
                            ctx.request_repaint();
                        });
                    }
                }
            }
            if widgets::chip(ui, "Cancel", false) || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.editing = None;
                self.checked = None;
            }
        });
        match &answer {
            Some(Some(AddressCheck::Same)) => {
                ui.label(RichText::new("The same server, and your login works there.").color(P.text));
            }
            Some(Some(other)) => {
                ui.label(RichText::new(other.to_string()).color(P.danger));
            }
            _ => {}
        }
        if let Some(why) = self.failed.lock().unwrap().clone() {
            ui.label(RichText::new(why).color(P.danger));
        }
        if self.editing.is_some() {
            self.editing = Some(address);
        }
        hint(ui, "Only the same server is accepted, found by its identity: nothing is fetched again, and the library, downloads and queue stay. A different server needs a log out and a log in.");
    }
}
