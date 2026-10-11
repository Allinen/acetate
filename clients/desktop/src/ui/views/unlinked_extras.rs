//! Review's "Unlinked extras": extras linked to nothing (music videos imported from disk…),
//! to link in bulk. Click rows to select (Shift+click a range, the buttons select all or
//! none), choose what they go to (a track, album, artist or grouping), type its name and
//! Enter: every selected extra is linked to it. With one extra selected, the search starts
//! with its name typed in (it's only text: change it).
//!
//! Links are the same `POST /api/extras/{id}/links` the extra's own page uses, one per extra.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::catalog::model::Extra;
use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::ui::edit::extras::{files_text, Owner};
use crate::ui::edit::inline_search::{describe, Choice, InlineSearch};
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Track,
    Album,
    Artist,
    Grouping,
}

impl Target {
    const ALL: [Target; 4] = [Target::Track, Target::Album, Target::Artist, Target::Grouping];

    fn name(self) -> &'static str {
        match self {
            Target::Track => "track",
            Target::Album => "album",
            Target::Artist => "artist",
            Target::Grouping => "grouping",
        }
    }

    fn part(self) -> Option<Part> {
        match self {
            Target::Track => Some(Part::Tracks),
            Target::Album => Some(Part::Albums),
            Target::Artist => Some(Part::Artists),
            Target::Grouping => None,
        }
    }
}

pub struct UnlinkedExtras {
    selected: HashSet<Uuid>,
    /// The row last clicked (a Shift+click goes from it).
    anchor: Option<usize>,
    target: Target,
    search: InlineSearch,
    /// The name typed into the search for a single selected extra (to tell it from the
    /// user's own typing).
    prefilled: Option<(Uuid, String)>,
    /// Linked here, hidden at once (the library catches up a moment later).
    linked: HashSet<Uuid>,
    sending: bool,
    result: Arc<Mutex<Option<Result<String, String>>>>,
    said: Option<Result<String, String>>,
}

impl Default for UnlinkedExtras {
    fn default() -> Self {
        UnlinkedExtras {
            selected: HashSet::new(),
            anchor: None,
            target: Target::Track,
            search: InlineSearch::new(Part::Tracks),
            prefilled: None,
            linked: HashSet::new(),
            sending: false,
            result: Arc::default(),
            said: None,
        }
    }
}

/// An extra's name as a search ("Song (Official Video).mp4" → "Song").
fn search_text(name: &str) -> String {
    let name = name.rsplit_once('.').filter(|(_, ext)| ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric())).map_or(name, |(stem, _)| stem);
    let name = name.split(['(', '[']).next().unwrap_or(name);
    name.trim().to_string()
}

impl UnlinkedExtras {
    /// `filter`: words in the name or kind.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, filter: &str) {
        if let Some(result) = self.result.lock().unwrap().take() {
            self.sending = false;
            self.said = Some(result);
            shell.act(Action::RefreshLibrary);
        }
        // Linked ones leave the list at once; forget them once the library says so too.
        self.linked.retain(|id| catalog.extras.iter().any(|e| e.id == *id && e.links.is_empty()));
        let words: Vec<String> = filter.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut shown: Vec<&Extra> = catalog
            .extras
            .iter()
            .filter(|e| e.links.is_empty() && !self.linked.contains(&e.id))
            .filter(|e| {
                let text = format!("{} {}", e.name, e.kind).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
        shown.sort_by_key(|e| e.name.to_lowercase());
        let everyone: HashSet<Uuid> = catalog.extras.iter().filter(|e| e.links.is_empty()).map(|e| e.id).collect();
        self.selected.retain(|id| everyone.contains(id) && !self.linked.contains(id));

        ui.label(RichText::new("Linked to nothing yet. Click to select (Shift+click for a range), choose what they belong to, type its name, Enter.").color(P.muted));
        ui.add_space(6.0);
        if shown.is_empty() {
            if let Some(said) = &self.said {
                self.say(ui, said.clone());
            }
            ui.label(RichText::new("None (or nothing matches the filter).").color(P.faint));
            return;
        }
        self.toolbar(ui, shell, catalog, &shown);
        ui.add_space(6.0);
        self.rows(ui, shell, &shown);
    }

    fn say(&self, ui: &mut egui::Ui, said: Result<String, String>) {
        match said {
            Ok(text) => ui.label(RichText::new(text).color(P.accent)),
            Err(err) => ui.label(RichText::new(err).color(P.danger)),
        };
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, shown: &[&Extra]) {
        ui.horizontal(|ui| {
            if ui.button("Select all shown").clicked() {
                self.selected.extend(shown.iter().map(|e| e.id));
            }
            if ui.add_enabled(!self.selected.is_empty(), egui::Button::new("Select none")).clicked() {
                self.selected.clear();
            }
            ui.label(RichText::new(format!("{} selected", self.selected.len())).color(P.muted));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Link to").font(font(13.0, semibold())));
            for target in Target::ALL {
                if widgets::chip(ui, target.name(), self.target == target) && self.target != target {
                    self.target = target;
                    if let Some(part) = target.part() {
                        self.search = InlineSearch::new(part);
                        self.prefilled = None;
                    }
                }
            }
        });
        ui.add_space(4.0);
        if self.selected.is_empty() {
            ui.label(RichText::new("Select some extras first.").color(P.faint));
            return;
        }
        // One extra selected: its name is typed in, as a start.
        if let (Some(part), true) = (self.target.part(), self.selected.len() == 1) {
            let only = *self.selected.iter().next().unwrap();
            if self.prefilled.as_ref().map(|p| p.0) != Some(only) {
                if let Some(e) = catalog.extras.iter().find(|e| e.id == only) {
                    let text = search_text(&e.name);
                    if self.search.query.is_empty() || self.prefilled.is_some() {
                        self.search = InlineSearch::new(part);
                        self.search.set_query(&text, shell);
                    }
                    self.prefilled = Some((only, text));
                }
            }
        }
        let chosen: Vec<Uuid> = shown.iter().map(|e| e.id).filter(|id| self.selected.contains(id)).collect();
        let sending = self.sending;
        match self.target.part() {
            Some(part) => {
                let hint = format!("type the {}'s name, Enter links {} extras", self.target.name(), chosen.len());
                let hit = self.search.show(ui, shell, Some(catalog), egui::Id::new("unlinked-extras-search"), &hint, &[]);
                if let (Some((Choice::Hit(i), _)), false) = (hit, sending) {
                    let (link, label) = match part {
                        Part::Tracks => (catalog.get_track(i).map(|t| Owner::Track(t.id())), describe(catalog, part, i)),
                        Part::Albums => (catalog.get_album(i).map(|a| Owner::Album(a.id())), describe(catalog, part, i)),
                        _ => (catalog.artists.get(i as usize).map(|a| Owner::Artist(a.id)), describe(catalog, part, i)),
                    };
                    if let Some(link) = link {
                        self.link_all(shell, chosen, link, format!("{} {}", label.0, label.1).trim().to_string());
                    }
                }
            }
            None => {
                let mut picked = None;
                egui::ComboBox::from_id_salt("unlinked-extras-grouping").selected_text("Choose a grouping…").show_ui(ui, |ui| {
                    for g in &catalog.groupings {
                        if ui.selectable_label(false, format!("{} · {}", g.name, g.kind)).clicked() {
                            picked = Some((g.id, g.name.clone()));
                        }
                    }
                });
                if let (Some((id, name)), false) = (picked, sending) {
                    self.link_all(shell, chosen, Owner::Grouping(id), name);
                }
            }
        }
        if self.sending {
            ui.label(RichText::new("Linking…").color(P.muted));
        } else if let Some(said) = self.said.clone() {
            self.say(ui, said);
        }
    }

    fn link_all(&mut self, shell: &Shell, extras: Vec<Uuid>, to: Owner, label: String) {
        if extras.is_empty() {
            return;
        }
        self.sending = true;
        self.said = None;
        self.prefilled = None;
        self.linked.extend(extras.iter().copied());
        self.selected.clear();
        let (api, ctx, result) = (shell.api.clone(), shell.ctx.clone(), self.result.clone());
        shell.runtime.spawn(async move {
            let (mut done, mut failed) = (0, Vec::new());
            for id in &extras {
                let body = serde_json::json!({ "link": to.link() });
                match api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/extras/{id}/links"), Some(body)).await {
                    Ok(_) => done += 1,
                    Err(e) => failed.push(e.message),
                }
            }
            let said = if failed.is_empty() {
                Ok(format!("Linked {} to {label}.", if done == 1 { "1 extra".to_string() } else { format!("{done} extras") }))
            } else {
                Err(format!("Linked {done} to {label}, {} failed: {}", failed.len(), failed[0]))
            };
            *result.lock().unwrap() = Some(said);
            ctx.request_repaint();
        });
    }

    fn rows(&mut self, ui: &mut egui::Ui, shell: &mut Shell, shown: &[&Extra]) {
        egui::ScrollArea::vertical().id_salt("unlinked-extras").auto_shrink([false, false]).show_rows(ui, 52.0, shown.len(), |ui, range| {
            for n in range {
                let e = shown[n];
                let on = self.selected.contains(&e.id);
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(760.0), 52.0), Sense::click());
                if on {
                    ui.painter().rect_filled(rect, 8, P.surface);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 8, P.surface.gamma_multiply(0.5));
                }
                let tick = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), Vec2::splat(16.0));
                ui.painter().rect_stroke(tick, 3, egui::Stroke::new(1.5, if on { P.accent } else { P.line }), egui::StrokeKind::Inside);
                if on {
                    ui.painter().rect_filled(tick.shrink(4.0), 2, P.accent);
                }
                let thumb = egui::Rect::from_min_size(egui::pos2(rect.left() + 34.0, rect.top() + 6.0), Vec2::new(71.0, 40.0));
                match e.thumb() {
                    Some(cover) => shell.cover(ui, Some(cover), e.id, thumb, 4, 0.0),
                    None => {
                        ui.painter().rect_stroke(thumb, 4, egui::Stroke::new(1.0, P.line), egui::StrokeKind::Inside);
                    }
                }
                let left = thumb.right() + 12.0;
                ui.painter().text(egui::pos2(left, rect.top() + 9.0), egui::Align2::LEFT_TOP, &e.name, font(14.0, semibold()), P.text);
                let files = files_text(e.files as usize);
                ui.painter().text(egui::pos2(left, rect.top() + 29.0), egui::Align2::LEFT_TOP, format!("{} · {files}", e.kind), font(12.0, medium()), P.muted);

                // "Open": its page (double-clicking the row does the same).
                let open = egui::Rect::from_center_size(egui::pos2(rect.right() - 30.0, rect.center().y), Vec2::new(48.0, 24.0));
                let open_response = ui.interact(open, ui.id().with(("open-extra", e.id)), Sense::click());
                let color = if open_response.hovered() { P.text } else { P.faint };
                ui.painter().text(open.center(), egui::Align2::CENTER_CENTER, "Open", font(12.0, medium()), color);
                if open_response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() || response.double_clicked() {
                    shell.act(Action::Go(Page::Extra(e.id)));
                    continue;
                }
                if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    let shift = ui.input(|i| i.modifiers.shift);
                    match (shift, self.anchor) {
                        (true, Some(from)) if from < shown.len() => {
                            let (a, b) = (from.min(n), from.max(n));
                            self.selected.extend(shown[a..=b].iter().map(|e| e.id));
                        }
                        _ if on => {
                            self.selected.remove(&e.id);
                        }
                        _ => {
                            self.selected.insert(e.id);
                        }
                    }
                    self.anchor = Some(n);
                    self.said = None;
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("unlinked-extras-search")));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_searches() {
        assert_eq!(search_text("Song (Official Video).mp4"), "Song");
        assert_eq!(search_text("Some Song [4K]"), "Some Song");
        assert_eq!(search_text("Mr. Brightside"), "Mr. Brightside");
    }
}
