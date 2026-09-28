//! Logging in: the server's address, a username and a password.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Key, Margin, Rect, RichText, Shadow, Vec2};

use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Shell};

pub struct Login {
    /// Server, username, password.
    form: (String, String, String),
    error: String,
    busy: bool,
    /// The login's outcome, from the background.
    result: Arc<Mutex<Option<Result<(), String>>>>,
}

impl Default for Login {
    fn default() -> Self {
        Login { form: ("http://127.0.0.1:3000".into(), String::new(), String::new()), error: String::new(), busy: false, result: Arc::default() }
    }
}

impl Login {
    /// Takes in the login's outcome (every frame: it may arrive after the login screen is
    /// gone).
    pub fn poll(&mut self, shell: &mut Shell) {
        if let Some(outcome) = self.result.lock().unwrap().take() {
            self.busy = false;
            match outcome {
                Ok(()) => {
                    self.error.clear();
                    self.form.2.clear();
                    shell.act(Action::LoggedIn);
                }
                Err(err) => self.error = err,
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let card = Rect::from_center_size(ui.max_rect().center(), Vec2::new(340.0, 330.0));
        ui.painter().add(Shadow { offset: [0, 18], blur: 60, spread: 0, color: Color32::from_black_alpha(160) }.as_shape(card, 16));
        ui.painter().rect_filled(card, 16, P.surface);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card.shrink(28.0)));
        let ui = &mut child;
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                widgets::logo(ui, &shell.logo, 30.0);
                ui.label(RichText::new("acetate").font(font(24.0, bold())));
            });
            ui.add_space(18.0);
            let field = |ui: &mut egui::Ui, text: &mut String, hint: &str, password: bool| {
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .password(password)
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(10, 8)),
                )
            };
            field(ui, &mut self.form.0, "http://nas:3000", false);
            field(ui, &mut self.form.1, "username", false);
            let password = field(ui, &mut self.form.2, "password", true);
            ui.add_space(8.0);
            let enter = password.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let button = egui::Button::new(RichText::new("Log in").color(P.bg).font(font(14.0, semibold())))
                .fill(P.accent)
                .min_size(Vec2::new(ui.available_width(), 36.0));
            if (ui.add(button).clicked() || enter) && !self.busy {
                let (api, result, ctx) = (shell.api.clone(), self.result.clone(), ui.ctx().clone());
                let (server, user, pass) = self.form.clone();
                self.busy = true;
                shell.runtime.spawn(async move {
                    let outcome = api.login(&server, &user, &pass).await.map(|_| ()).map_err(|err| format!("{err:#}"));
                    *result.lock().unwrap() = Some(outcome);
                    ctx.request_repaint();
                });
            }
            ui.colored_label(Color32::from_rgb(255, 120, 120), &self.error);
        });
    }
}
