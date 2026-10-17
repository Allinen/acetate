//! Together: a session to listen in with others (everyone hears the same, anyone can pause,
//! skip, seek or change the queue), and what others are listening to. A session is started
//! here and joined with its invite code. Showing others what you play is off until switched
//! on here.

use eframe::egui::{self, RichText};

use crate::catalog::Catalog;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

#[derive(Default)]
pub struct TogetherPage {
    code: String,
}

impl TogetherPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: Option<&Catalog>) {
        shell.together.page_shown();
        let view = shell.together.view();
        ui.label(RichText::new("Together").font(font(30.0, bold())));
        ui.add_space(4.0);
        if shell.connection.is_offline() {
            ui.label(RichText::new("Listening together needs the server, which can't be reached.").color(P.muted));
            return;
        }
        if let Some(err) = &view.error {
            ui.label(RichText::new(err).color(P.danger));
            ui.add_space(4.0);
        }

        match &view.room {
            Some(room) => {
                ui.label(RichText::new("You're listening together. What you play is played by everyone here, and what they play is played by you.").color(P.muted));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Invite code").color(P.muted));
                    ui.label(RichText::new(&room.code).font(font(22.0, bold())).monospace());
                    if widgets::chip(ui, "Copy", false) {
                        ui.ctx().copy_text(room.code.clone());
                    }
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Here").font(font(16.0, semibold())));
                for member in &room.members {
                    ui.label(&member.username);
                }
                if let Some(by) = &room.changed_by {
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Last change by {by}")).color(P.muted));
                }
                ui.add_space(8.0);
                if widgets::chip(ui, "Leave", false) {
                    shell.together.leave();
                }
            }
            None => {
                ui.label(RichText::new("Start a session and send its code to whoever should join, or join one with a code you were sent.").color(P.muted));
                ui.add_space(8.0);
                if widgets::chip(ui, "Start a session", false) {
                    shell.together.start();
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let edit = ui.add(egui::TextEdit::singleline(&mut self.code).hint_text("Invite code").desired_width(140.0));
                    let go = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (widgets::chip(ui, "Join", false) || go) && !self.code.trim().is_empty() {
                        shell.together.join(&self.code);
                        self.code.clear();
                    }
                });
            }
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Listening now").font(font(20.0, bold())));
            if widgets::chip(ui, "Refresh", false) {
                shell.together.refresh_now();
            }
        });
        let mut sharing = view.sharing;
        if ui.checkbox(&mut sharing, "Let others see what I'm listening to").changed() {
            shell.settings.share_listening = sharing;
            shell.settings.save(&crate::root::root());
            shell.together.set_sharing(sharing);
        }
        ui.add_space(6.0);
        if view.listening.is_empty() {
            ui.label(RichText::new("Nobody else is sharing what they listen to right now.").color(P.muted));
            return;
        }
        let mut play = None;
        let mut open = None;
        for l in &view.listening {
            let track = catalog.and_then(|c| c.track_index(l.track_id).map(|i| (i, c.track_at(i))));
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&l.username).font(font(14.0, semibold())));
                match track {
                    Some((i, t)) => {
                        if widgets::link(ui, &format!("{} · {}", t.title(), t.artist_display()), font(14.0, semibold()), P.accent) {
                            open = Some(l.track_id);
                        }
                        if widgets::chip(ui, "Play", false) {
                            play = Some(i);
                        }
                    }
                    None => {
                        ui.label(RichText::new("a track you can't see").color(P.muted));
                    }
                }
                if l.paused {
                    ui.label(RichText::new("paused").color(P.muted));
                }
            });
        }
        if let Some(i) = play {
            shell.act(Action::Play { tracks: vec![i], start: 0 });
        }
        if let Some(id) = open {
            shell.act(Action::Go(Page::Track(id)));
        }
    }
}
