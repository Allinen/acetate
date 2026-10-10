//! Settings, in tabs (`h`/`l` or the arrows move between them): Sound; This computer (the
//! downloads, the library kept here, memory); Account; and for admins the Server (what it
//! stores) and the Mirror of its files; General. Each tab is its own file.

use eframe::egui::{self, RichText};

use super::settings_account::AccountTab;
use super::settings_computer::ComputerTab;
use super::settings_general::GeneralTab;
use super::settings_mirror::MirrorSection;
use super::settings_server::ServerTab;
use super::settings_sound::SoundTab;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, PALETTE as P};
use crate::ui::{widgets, Shell};

#[derive(Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Sound,
    Computer,
    Account,
    /// Admins.
    Server,
    /// Admins.
    Mirror,
    General,
}

impl Tab {
    const ALL: [Tab; 6] = [Tab::Sound, Tab::Computer, Tab::Account, Tab::Server, Tab::Mirror, Tab::General];

    fn name(self) -> &'static str {
        match self {
            Tab::Sound => "Sound",
            Tab::Computer => "This computer",
            Tab::Account => "Account",
            Tab::Server => "Server",
            Tab::Mirror => "Mirror",
            Tab::General => "General",
        }
    }

    fn admin_only(self) -> bool {
        matches!(self, Tab::Server | Tab::Mirror)
    }
}

#[derive(Default)]
pub struct SettingsPage {
    /// The tab shown (kept while the app runs).
    tab: Tab,
    sound: SoundTab,
    computer: ComputerTab,
    account: AccountTab,
    server: ServerTab,
    /// The mirror, and moving the folder everything's kept in (shown in This computer).
    mirror: MirrorSection,
    general: GeneralTab,
}

/// `12.3 MB`, `1.2 GB`, `3.4 TB`.
pub fn size(bytes: i64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e12 {
        format!("{:.1} TB", bytes / 1e12)
    } else if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else {
        format!("{:.1} MB", bytes / 1e6)
    }
}

impl SettingsPage {
    pub fn opened(&mut self, shell: &Shell) {
        self.mirror.opened(shell);
        self.sound.opened(shell);
        self.computer.opened(shell);
        self.general.opened(shell);
        if shell.account.can("admin") {
            self.server.opened(shell);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
        let admin = shell.account.can("admin");
        let tabs: Vec<Tab> = Tab::ALL.into_iter().filter(|t| admin || !t.admin_only()).collect();
        if !tabs.contains(&self.tab) {
            self.tab = Tab::Sound;
        }
        // Outside text fields (there the letters are for the text).
        if !ui.ctx().egui_wants_keyboard_input() {
            let key = |k: egui::Key| ui.ctx().input(|i| i.key_pressed(k) && i.modifiers.is_none());
            let at = tabs.iter().position(|&t| t == self.tab).unwrap_or(0);
            if (key(egui::Key::ArrowLeft) || key(egui::Key::H)) && at > 0 {
                self.tab = tabs[at - 1];
            } else if (key(egui::Key::ArrowRight) || key(egui::Key::L)) && at + 1 < tabs.len() {
                self.tab = tabs[at + 1];
            }
        }

        ui.label(RichText::new("Settings").font(font(30.0, bold())));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            for &tab in &tabs {
                if widgets::chip(ui, tab.name(), tab == self.tab) {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
        // The tab's page fades in when switched to.
        ui.multiply_opacity(widgets::appear(ui.ctx(), egui::Id::new(("settings-tab", self.tab.name())), 0.12));
        egui::ScrollArea::vertical().id_salt(self.tab.name()).auto_shrink([false, false]).show(ui, |ui| match self.tab {
            Tab::Sound => self.sound.show(ui, shell, player),
            Tab::Computer => self.computer.show(ui, shell, &mut self.mirror),
            Tab::Account => self.account.show(ui, shell),
            Tab::Server => self.server.show(ui, shell),
            Tab::Mirror => self.mirror.show(ui, shell),
            Tab::General => self.general.show(ui, shell),
        });
    }
}

pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(18.0);
    ui.label(RichText::new(title).font(font(18.0, bold())));
    ui.add_space(8.0);
}

pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).font(font(12.5, medium())).color(P.faint));
}
