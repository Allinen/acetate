//! Merging two artists that are the same (from the artist page in edit mode): the left
//! one is merged into the right one (`s` swaps). What happens is said first: its credits
//! (tracks and releases, each rewritten as a revision) move over, and so do its other
//! names ("credited as" keeps working: an alias both have is joined, one spelled like the
//! kept artist's name becomes that name). Two choices, both on by default: keep the old
//! name as an alias (its tracks keep showing it), and flag the tracks whose credits
//! change for review. Ctrl+Enter merges (`POST /api/artists/{left}/merge`).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{thousands, Shell};

pub enum Outcome {
    /// Merged: `kept` stays.
    Merged { kept: Uuid, gone: Uuid },
    Closed,
}

pub struct ArtistMergeView {
    /// Merged away.
    pub from: Uuid,
    /// Stays.
    pub into: Uuid,
    keep_name: bool,
    flag: bool,
    reason: String,
    merging: bool,
    error: Option<String>,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
}

impl ArtistMergeView {
    pub fn new(from: Uuid, into: Uuid) -> ArtistMergeView {
        ArtistMergeView { from, into, keep_name: true, flag: true, reason: String::new(), merging: false, error: None, answer: Arc::default() }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog) -> Option<Outcome> {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.merging = false;
            match result {
                Ok(()) => return Some(Outcome::Merged { kept: self.into, gone: self.from }),
                Err(err) => self.error = Some(err),
            }
        }
        let (Some(from), Some(into)) = (catalog.artist(self.from), catalog.artist(self.into)) else {
            ui.label(RichText::new("One of the two artists isn't in the library (anymore).").color(P.muted));
            return ui.button("Close").clicked().then_some(Outcome::Closed);
        };

        if !shell.palette_open {
            let ctx = ui.ctx().clone();
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                self.merge(shell);
            }
            if !ctx.egui_wants_keyboard_input() {
                let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
                if key(Key::S) {
                    std::mem::swap(&mut self.from, &mut self.into);
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                } else if key(Key::Escape) {
                    return Some(Outcome::Closed);
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Merge artists").font(font(20.0, bold())));
            ui.label(RichText::new("the left one is merged into the right one, which stays").color(P.muted));
            if ui.small_button("Swap  (s)").clicked() {
                std::mem::swap(&mut self.from, &mut self.into);
            }
        });
        ui.add_space(8.0);
        let width = ((ui.available_width() - 16.0) / 2.0).max(220.0);
        ui.horizontal_top(|ui| {
            for (artist, label, color) in [(from, "Merged away", P.muted), (into, "Stays", P.accent)] {
                egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(RichText::new(label).font(font(12.0, semibold())).color(color));
                    let name = if artist.disambiguation.is_empty() { artist.name.clone() } else { format!("{} ({})", artist.name, artist.disambiguation) };
                    ui.label(RichText::new(name).font(font(16.0, semibold())));
                    if !artist.aliases.is_empty() {
                        ui.label(RichText::new(format!("also {}", artist.aliases.join(", "))).color(P.muted));
                    }
                    ui.label(RichText::new(format!("{} tracks · {} releases", thousands(artist.track_count as usize), thousands(artist.album_count as usize))).color(P.faint));
                });
            }
        });

        // ---- What happens ----
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!(
                "“{}”'s credits (on {} tracks and {} releases, each rewritten as a revision) and sources move to “{}”; “{}” then only leads to it.",
                from.name,
                thousands(from.track_count as usize),
                thousands(from.album_count as usize),
                into.name,
                from.name
            ))
            .color(P.text),
        );
        if !from.aliases.is_empty() {
            ui.label(
                RichText::new(format!(
                    "Its other names ({}) become “{}”'s, and credits under them stay under them. A name both have is joined into one; one spelled like “{}” becomes simply its name.",
                    from.aliases.join(", "),
                    into.name,
                    into.name
                ))
                .color(P.text),
            );
        }
        ui.add_space(4.0);
        ui.checkbox(&mut self.keep_name, format!("Keep “{}” as a name of “{}”", from.name, into.name));
        let keep = if self.keep_name {
            format!("Its tracks keep showing “{}”.", from.name)
        } else {
            format!("Its tracks show “{}” instead.", into.name)
        };
        ui.label(RichText::new(keep).color(P.muted).small());
        ui.checkbox(&mut self.flag, "Flag the tracks whose credits change, for review");
        ui.label(
            RichText::new(format!(
                "For checking afterwards: say “{}” was really “{} feat. someone”, that someone may need crediting again.",
                from.name, into.name
            ))
            .color(P.muted)
            .small(),
        );

        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional)").desired_width(420.0));
        let mut outcome = None;
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.merging, egui::Button::new("Merge  (Ctrl+Enter)")).clicked() {
                self.merge(shell);
            }
            if ui.button("Close  (Esc)").clicked() {
                outcome = Some(Outcome::Closed);
            }
            if self.merging {
                ui.label(RichText::new("Merging…").color(P.muted));
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(P.danger));
        }
        outcome
    }

    fn body(&self) -> serde_json::Value {
        serde_json::json!({
            "into": self.into,
            "keep_name_as_alias": self.keep_name,
            "flag_for_review": self.flag,
            "reason": self.reason.trim(),
        })
    }

    fn merge(&mut self, shell: &Shell) {
        if self.merging {
            return;
        }
        self.merging = true;
        self.error = None;
        let (api, answer, ctx, from, body) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), self.from, self.body());
        shell.runtime.spawn(async move {
            let result = api
                .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/artists/{from}/merge"), Some(body))
                .await
                .map(|_| ())
                .map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_choices_are_on_to_start_and_sent() {
        let mut view = ArtistMergeView::new(Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(view.body()["keep_name_as_alias"], true);
        assert_eq!(view.body()["flag_for_review"], true);
        view.flag = false;
        std::mem::swap(&mut view.from, &mut view.into);
        assert_eq!(view.body()["into"], Uuid::from_u128(1).to_string());
        assert_eq!(view.body()["flag_for_review"], false);
    }
}
