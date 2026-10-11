//! Every grouping (collectives, eras, projects…), to browse: chips by kind (with counts),
//! a filter by name, and a row each with its members, tracks and releases. Enter (or a
//! click) opens one. In edit mode, a new one can be made here (its kind and name).
//! Reached from the Artists page ("Collectives & groupings") and the palette.


use std::sync::Arc;
use eframe::egui::{self, RichText, Sense, Vec2};
use serde_json::json;
use uuid::Uuid;

use super::grouping::KINDS;
use crate::catalog::Catalog;
use crate::ui::task::Task;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Default)]
pub struct GroupingsPage {
    filter: String,
    /// Only this kind (None: all).
    kind: Option<String>,
    selected: usize,
    /// "New grouping…": its kind and name, and the server's answer.
    new: Option<(String, String)>,
    answer: Task<Result<Uuid, String>>,
    error: Option<String>,
}

impl GroupingsPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        if let Some(result) = self.answer.poll() {
            match result {
                Ok(id) => {
                    self.new = None;
                    shell.act(Action::RefreshLibrary);
                    shell.act(Action::Go(Page::Grouping(id)));
                }
                Err(err) => self.error = Some(err),
            }
        }
        let words: Vec<String> = self.filter.to_lowercase().split_whitespace().map(str::to_string).collect();
        let shown: Vec<&crate::catalog::model::Grouping> = catalog
            .groupings
            .iter()
            .filter(|g| self.kind.as_ref().is_none_or(|k| &g.kind == k))
            .filter(|g| {
                let name = g.name.to_lowercase();
                words.iter().all(|w| name.contains(w.as_str()))
            })
            .collect();
        super::heading(ui, "Collectives & groupings", shown.len());
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if super::filter(ui, &mut self.filter, "Filter by name") {
                self.selected = 0;
            }
            ui.add_space(12.0);
            if widgets::chip(ui, &format!("All  {}", thousands(catalog.groupings.len())), self.kind.is_none()) {
                self.kind = None;
                self.selected = 0;
            }
            // The kinds there are (the usual ones first), with how many of each.
            let mut kinds: Vec<&str> = KINDS.iter().copied().filter(|k| catalog.groupings.iter().any(|g| g.kind == *k)).collect();
            for g in &catalog.groupings {
                if !kinds.contains(&g.kind.as_str()) {
                    kinds.push(&g.kind);
                }
            }
            for kind in kinds {
                let count = catalog.groupings.iter().filter(|g| g.kind == kind).count();
                if widgets::chip(ui, &format!("{}  {}", capitalized(kind), thousands(count)), self.kind.as_deref() == Some(kind)) {
                    self.kind = Some(kind.to_string());
                    self.selected = 0;
                }
            }
            if shell.edit_mode && shell.account.can("edit_metadata") && self.new.is_none() && ui.button("New grouping…").clicked() {
                self.new = Some((self.kind.clone().unwrap_or_else(|| "collective".into()), String::new()));
            }
        });
        self.new_form(ui, shell);
        ui.add_space(14.0);

        if catalog.groupings.is_empty() {
            ui.label(RichText::new("No groupings yet. Make one here in edit mode, from a track's or album's “Part of”, or by turning an artist that's really a collective into one (its page, edit mode).").color(P.muted));
            return;
        }
        let row_height = 52.0;
        let visible = ((ui.available_height() / row_height) as usize).max(1);
        let moved = widgets::move_selection(ui.ctx(), &mut self.selected, shown.len(), 1, visible);
        if moved.enter
            && let Some(g) = shown.get(self.selected)
        {
            shell.act(Action::Go(Page::Grouping(g.id)));
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_height, shown.len(), |ui, range| {
            for n in range {
                let g = shown[n];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::click());
                if n == self.selected || response.hovered() {
                    ui.painter().rect_filled(rect, 8, if n == self.selected { P.accent_soft } else { P.surface });
                }
                if n == self.selected && moved.moved {
                    response.scroll_to_me(None);
                }
                let painter = ui.painter();
                let left = rect.left() + 14.0;
                painter.text(egui::pos2(left, rect.top() + 8.0), egui::Align2::LEFT_TOP, &g.name, font(15.0, semibold()), P.text);
                let members = match g.artists.len() {
                    0 => String::new(),
                    1 => " · 1 member".to_string(),
                    n => format!(" · {n} members"),
                };
                let sub = format!("{}{members} · {} tracks · {} releases", capitalized(&g.kind), thousands(g.track_count as usize), thousands(g.album_count as usize));
                painter.text(egui::pos2(left, rect.top() + 29.0), egui::Align2::LEFT_TOP, sub, font(12.5, medium()), P.muted);
                if response.clicked() {
                    self.selected = n;
                    shell.act(Action::Go(Page::Grouping(g.id)));
                }
            }
        });
    }

    /// "New grouping…": its kind (the usual ones as chips) and name; Enter makes it.
    fn new_form(&mut self, ui: &mut egui::Ui, shell: &mut Shell) {
        let mut cancel = false;
        let mut make = false;
        if let Some((kind, name)) = self.new.as_mut() {
            ui.horizontal_wrapped(|ui| {
                for k in KINDS {
                    if widgets::chip(ui, k, kind == k) {
                        *kind = k.to_string();
                    }
                }
                ui.add(egui::TextEdit::singleline(kind).desired_width(100.0).hint_text("kind"));
                let field = ui.add(egui::TextEdit::singleline(name).desired_width(260.0).hint_text("its name"));
                make = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                make |= ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Make it")).clicked();
                cancel = ui.button("Cancel").clicked();
            });
        }
        if cancel {
            self.new = None;
        }
        if make && let Some((kind, name)) = self.new.clone().filter(|(_, n)| !n.trim().is_empty()) {
            #[derive(serde::Deserialize)]
            struct Made {
                id: Uuid,
            }
            self.error = None;
            self.answer.send_with(shell, |api| async move {
                let body = json!({ "kind": kind.trim(), "name": name.trim() });
                api.send_json::<Made>(reqwest::Method::POST, "/api/groupings", Some(body)).await.map(|m| m.id).map_err(|e| e.message)
            });
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
    }
}

/// "collective" → "Collective".
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}
