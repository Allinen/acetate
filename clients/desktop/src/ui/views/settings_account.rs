//! Settings, Account: the server (its address can be changed, to the same server), who's
//! logged in, changing the password, the devices logged in (each can be ended), logging out.

use std::sync::{Arc, Mutex};

use acetate_core::api::AddressCheck;
use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use crate::ui::task::Task;
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
    /// The password being changed: the current one, the new one twice.
    password: [String; 3],
    changing: Task<Result<String, String>>,
    /// The devices logged in (`GET /api/auth/sessions`), asked when the tab opens.
    sessions: Task<Result<Vec<Device>, String>>,
    devices: Option<Result<Vec<Device>, String>>,
    ending: Task<Result<String, String>>,
    said: Option<Result<String, String>>,
    /// The share links (`GET /api/shares`: yours, everyone's for an admin).
    shares: Task<Result<Vec<Share>, String>>,
    share_list: Option<Result<Vec<Share>, String>>,
    revoking: Task<Result<String, String>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Share {
    id: uuid::Uuid,
    title: String,
    created_by: String,
    created_at: String,
    expires_at: Option<String>,
    original: bool,
    kbps: Option<i32>,
    /// "active", or why it doesn't work.
    state: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Device {
    id: uuid::Uuid,
    user_agent: Option<String>,
    ip: Option<String>,
    created_at: String,
    last_used_at: String,
    current: bool,
}

impl AccountTab {
    pub fn opened(&mut self, shell: &Shell) {
        self.sessions.start_with(shell, |api| async move { api.send_json::<Vec<Device>>(reqwest::Method::GET, "/api/auth/sessions", None).await.map_err(|e| e.message) });
        if shell.account.can("share") {
            self.shares.start_with(shell, |api| async move { api.send_json::<Vec<Share>>(reqwest::Method::GET, "/api/shares", None).await.map_err(|e| e.message) });
        }
    }

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
        self.security(ui, shell);
        if shell.account.can("share") {
            self.share_links(ui, shell);
        }
        ui.add_space(12.0);
        if widgets::chip(ui, "Log out", false) {
            shell.act(Action::LogOut);
        }
    }

    /// The share links: what each plays, until when, and a way to end it.
    fn share_links(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(list) = self.shares.poll() {
            self.share_list = Some(list);
        }
        if let Some(done) = self.revoking.poll() {
            if done.is_ok() {
                self.opened(shell);
            }
            self.said = Some(done);
        }
        section(ui, "Share links");
        match &self.share_list {
            None => {
                ui.label(RichText::new("Asking…").color(P.faint));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(RichText::new("None yet: Ctrl+Shift+L on a track or album makes one.").color(P.faint));
            }
            Some(Ok(list)) => {
                let busy = self.revoking.is_running();
                let day = |t: &str| t.get(..10).unwrap_or(t).to_string();
                for share in list {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&share.title).color(P.text));
                        let plays = if share.original { "the original".to_string() } else { share.kbps.map_or("a lower-quality copy".to_string(), |k| format!("a {k} kbps copy")) };
                        let until = share.expires_at.as_deref().map_or("no end".to_string(), |t| format!("until {}", day(t)));
                        let state = if share.state == "active" { String::new() } else { format!(" · not working: {}", share.state.replace('_', " ")) };
                        ui.label(RichText::new(format!("plays {plays} · {until} · by {} on {}{state}", share.created_by, day(&share.created_at))).color(P.faint));
                        if share.state == "active" && ui.add_enabled(!busy, egui::Button::new("Revoke").small()).clicked() {
                            let path = format!("/api/shares/{}", share.id);
                            self.said = None;
                            self.revoking.send_with(shell, |api| async move { api.send(reqwest::Method::DELETE, &path, None).await.map(|_| "The link is revoked.".to_string()).map_err(|e| e.message) });
                        }
                    });
                }
            }
        }
    }

    /// Changing the password, and the devices that are logged in.
    fn security(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        if let Some(devices) = self.sessions.poll() {
            self.devices = Some(devices);
        }
        if let Some(done) = self.changing.poll() {
            if done.is_ok() {
                self.password = Default::default();
                // (Every other device was logged out by it.)
                self.opened(shell);
            }
            self.said = Some(done);
        }
        if let Some(done) = self.ending.poll() {
            if done.is_ok() {
                self.opened(shell);
            }
            self.said = Some(done);
        }
        section(ui, "Password");
        let labels = ["Current password", "New password (at least 8 characters)", "The new one again"];
        for (field, label) in self.password.iter_mut().zip(labels) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(P.muted));
                ui.add(egui::TextEdit::singleline(field).password(true).desired_width(240.0));
            });
        }
        let [current, new, again] = &self.password;
        let mismatch = !new.is_empty() && !again.is_empty() && new != again;
        if mismatch {
            ui.label(RichText::new("The new passwords don't match.").color(P.danger));
        }
        let ready = !current.is_empty() && new.chars().count() >= 8 && new == again && !self.changing.is_running();
        if ui.add_enabled(ready, egui::Button::new("Change password")).clicked() {
            let body = serde_json::json!({ "current_password": current, "new_password": new });
            self.said = None;
            self.changing.send_with(shell, |api| async move {
                api.send(reqwest::Method::POST, "/api/auth/password", Some(body)).await.map(|_| "Password changed. Your other devices are logged out.".to_string()).map_err(|e| e.message)
            });
        }
        match &self.said {
            Some(Ok(text)) => {
                ui.label(RichText::new(text.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }

        section(ui, "Logged in on");
        match &self.devices {
            None => {
                ui.label(RichText::new("Asking…").color(P.faint));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("Couldn't ask the server: {err}")).color(P.danger));
            }
            Some(Ok(devices)) => {
                let busy = self.ending.is_running();
                for device in devices {
                    ui.horizontal_wrapped(|ui| {
                        let what = device.user_agent.as_deref().unwrap_or("a device");
                        ui.label(RichText::new(if device.current { format!("{what} (this one)") } else { what.to_string() }).color(P.text));
                        let day = |t: &str| t.get(..10).unwrap_or(t).to_string();
                        let ip = device.ip.as_deref().map(|ip| format!(" · {ip}")).unwrap_or_default();
                        ui.label(RichText::new(format!("last used {}{ip} · since {}", day(&device.last_used_at), day(&device.created_at))).color(P.faint));
                        if !device.current && ui.add_enabled(!busy, egui::Button::new("Log out").small()).clicked() {
                            let path = format!("/api/auth/sessions/{}", device.id);
                            self.said = None;
                            self.ending.send_with(shell, |api| async move { api.send(reqwest::Method::DELETE, &path, None).await.map(|_| "Logged out.".to_string()).map_err(|e| e.message) });
                        }
                    });
                }
                if devices.len() > 1 && ui.add_enabled(!busy, egui::Button::new("Log out all the others")).clicked() {
                    self.said = None;
                    self.ending.send_with(shell, |api| async move { api.send(reqwest::Method::DELETE, "/api/auth/sessions", None).await.map(|_| "Every other device is logged out.".to_string()).map_err(|e| e.message) });
                }
            }
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
