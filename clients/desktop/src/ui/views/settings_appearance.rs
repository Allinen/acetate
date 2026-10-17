//! Settings, Appearance: the color theme. A built-in one, or one made here (a copy of
//! the one in use, with any of its colors changed); the app restyles as colors change.

use eframe::egui::{self, RichText};

use super::settings::{hint, section};
use crate::ui::theme::{self, CustomTheme, PALETTE as P};
use crate::ui::{widgets, Shell};

#[derive(Default)]
pub struct AppearanceTab {
    /// The name being typed for the theme being edited.
    name: String,
    /// The theme `name` is for (it's reset when another is chosen).
    named: String,
    /// Colors changed and not saved yet: saved when the mouse is let go (a dragged color
    /// changes every frame).
    unsaved: bool,
}

impl AppearanceTab {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        section(ui, "Theme");
        ui.horizontal_wrapped(|ui| {
            let names: Vec<String> = theme::PRESETS.iter().map(|(n, _)| n.to_string()).chain(shell.settings.custom_themes.iter().map(|t| t.name.clone())).collect();
            for name in names {
                if widgets::chip(ui, &name, name == shell.settings.theme) {
                    self.choose(ui.ctx(), shell, name);
                }
            }
        });
        hint(ui, "The colors of the whole app, changed at once.");

        section(ui, "Your own");
        let custom = shell.settings.custom_themes.iter().position(|t| t.name == shell.settings.theme);
        if widgets::chip(ui, "New theme from this one", false) {
            let name = self.free_name(shell, "My theme");
            shell.settings.custom_themes.push(CustomTheme::new(name.clone(), theme::find(&shell.settings.theme, &shell.settings.custom_themes)));
            self.choose(ui.ctx(), shell, name);
            return;
        }
        let Some(index) = custom else {
            hint(ui, "Makes a copy of the theme in use that you can change the colors of.");
            return;
        };

        if self.named != shell.settings.theme {
            self.named = shell.settings.theme.clone();
            self.name = self.named.clone();
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").color(P.muted));
            let field = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
            if field.lost_focus() {
                self.rename(ui.ctx(), shell, index);
            }
        });
        ui.add_space(6.0);
        let mut palette = shell.settings.custom_themes[index].palette();
        let mut changed = false;
        egui::Grid::new("theme-colors").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
            for (label, color) in palette.colors_mut() {
                ui.label(RichText::new(label).color(P.muted));
                // Opaque: the picker without opacity or blending.
                let mut rgb = [(*color >> 16) as u8, (*color >> 8) as u8, *color as u8];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    *color = u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]);
                    changed = true;
                }
                ui.end_row();
            }
        });
        if changed {
            shell.settings.custom_themes[index] = CustomTheme::new(shell.settings.theme.clone(), palette);
            theme::set_palette(ui.ctx(), palette.into());
            self.unsaved = true;
        }
        if self.unsaved && !ui.input(|i| i.pointer.any_down()) {
            self.unsaved = false;
            shell.settings.save(&shell.data_dir);
        }
        ui.add_space(10.0);
        if widgets::chip(ui, "Delete this theme", false) {
            shell.settings.custom_themes.remove(index);
            self.choose(ui.ctx(), shell, theme::DEFAULT_THEME.to_string());
        }
    }

    fn choose(&mut self, ctx: &egui::Context, shell: &mut Shell, name: String) {
        theme::set_palette(ctx, theme::find(&name, &shell.settings.custom_themes).into());
        shell.settings.theme = name;
        shell.settings.save(&shell.data_dir);
    }

    /// `base`, or `base 2`, `base 3`… if that's taken.
    fn free_name(&self, shell: &Shell, base: &str) -> String {
        let taken = |name: &str| acetate_core::theme::is_preset(name) || shell.settings.custom_themes.iter().any(|t| t.name == name);
        std::iter::once(base.to_string()).chain((2..).map(|n| format!("{base} {n}"))).find(|name| !taken(name)).unwrap_or_default()
    }

    /// Takes the typed name if it's not empty or another theme's; else shows the old one again.
    fn rename(&mut self, ctx: &egui::Context, shell: &mut Shell, index: usize) {
        let new = self.name.trim().to_string();
        let taken = acetate_core::theme::is_preset(&new) || shell.settings.custom_themes.iter().enumerate().any(|(i, t)| i != index && t.name == new);
        if new.is_empty() || taken {
            self.name = shell.settings.theme.clone();
            return;
        }
        shell.settings.custom_themes[index].name = new.clone();
        shell.settings.theme = new.clone();
        self.named = new;
        shell.settings.save(&shell.data_dir);
        ctx.request_repaint();
    }
}
