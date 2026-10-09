//! Editing credits (a track's or an album's): each artist with its role, the name it's
//! credited under ("credited as": its own, or one of its aliases), in order; artists typed
//! in place (matches from the library as you type, or a new one, made when the form is
//! saved). The artist text people see is made from these by the server, never typed.

use eframe::egui::{self, RichText};

use super::inline_search::{Choice, InlineSearch};
use super::{capitalized, combo};

use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::ui::theme::PALETTE as P;
use crate::ui::Shell;

pub use acetate_core::editor::forms::CreditRow;

/// The editor's own state: the field artists are typed in.
pub struct CreditsEditor {
    pub search: InlineSearch,
}

impl Default for CreditsEditor {
    fn default() -> Self {
        CreditsEditor { search: InlineSearch::new(Part::Artists) }
    }
}

enum RowAction {
    Replace(usize),
    Swap(usize, usize),
    Remove(usize),
}

impl CreditsEditor {
    /// The rows and the field (`field`: its id, for putting the cursor there; `salt`: the
    /// form's, so two editors don't mix). Returns a row whose artist button was clicked:
    /// the form asks the palette for another artist for it.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        shell: &mut Shell,
        catalog: Option<&Catalog>,
        salt: &str,
        field: egui::Id,
        rows: &mut Vec<CreditRow>,
        roles: &[String],
    ) -> Option<usize> {
        let mut action = None;
        egui::Grid::new((salt, "credits")).num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            let count = rows.len();
            for (i, row) in rows.iter_mut().enumerate() {
                let name = if row.artist_id.is_none() { format!("{}  (new)", row.name) } else { row.name.clone() };
                if ui.button(name).on_hover_text("Choose another artist").clicked() {
                    action = Some(RowAction::Replace(i));
                }
                combo(ui, (salt, "credit-role", i), &mut row.role, roles, capitalized);
                // Credited as: the artist's name, or one of its aliases.
                let aliases = match row.artist_id {
                    Some(id) => shell.artist_details.get(id).value.as_ref().map(|a| a.aliases.clone()).unwrap_or_default(),
                    None => Vec::new(),
                };
                let shown = row.alias.clone().map(|a| format!("as {a}")).unwrap_or_else(|| "as its name".into());
                egui::ComboBox::from_id_salt((salt, "credit-alias", i)).selected_text(shown).show_ui(ui, |ui| {
                    if ui.selectable_label(row.alias_id.is_none(), format!("as {}", row.name)).clicked() {
                        row.alias_id = None;
                        row.alias = None;
                    }
                    for alias in &aliases {
                        if ui.selectable_label(row.alias_id == Some(alias.id), format!("as {}", alias.name)).clicked() {
                            row.alias_id = Some(alias.id);
                            row.alias = Some(alias.name.clone());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Earlier").clicked() {
                        action = Some(RowAction::Swap(i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < count, egui::Button::new("↓").small()).on_hover_text("Later").clicked() {
                        action = Some(RowAction::Swap(i, i + 1));
                    }
                    if ui.small_button("×").on_hover_text("Remove").clicked() {
                        action = Some(RowAction::Remove(i));
                    }
                });
                ui.end_row();
            }
        });
        let typed = self.search.query.trim().to_string();
        let extras = if typed.is_empty() { vec![] } else { vec![format!("New artist “{typed}”")] };
        let hint = "Add an artist: type a name (Enter adds it, Shift+Enter as featured)";
        match self.search.show(ui, shell, catalog, field, hint, &extras) {
            Some((Choice::Hit(i), shift)) => {
                if let Some(a) = catalog.and_then(|c| c.artists.get(i as usize)) {
                    add_artist(rows, Some(a.id), a.name.clone(), shift);
                }
            }
            Some((Choice::Extra(_), shift)) => add_artist(rows, None, typed, shift),
            None => {}
        }
        ui.label(RichText::new("The artist text is made from these.").color(P.faint).small());
        match action {
            Some(RowAction::Replace(i)) => return Some(i),
            Some(RowAction::Swap(a, b)) => rows.swap(a, b),
            Some(RowAction::Remove(i)) => {
                rows.remove(i);
            }
            None => {}
        }
        None
    }
}

pub use acetate_core::editor::forms::{add_artist, make_new_artists, rows_from, take_ids};
