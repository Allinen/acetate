//! A grouping's page: a collective (an account several artists post under), an era, a
//! project… Its releases (a row; the heading shows them all) and its tracks, with the
//! artists credited on them (who's in it, as far as the credits say). In edit mode: its
//! kind, name and description; its sources (the accounts: ingest puts songs from them in
//! it, see the server's `db::groupings`); every change made to it.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use serde_json::json;
use uuid::Uuid;

use super::album::play_button;
use super::track_table::{Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

/// The kinds offered (any lowercase word is one).
pub const KINDS: [&str; 3] = ["collective", "era", "project"];

/// Its kind, name and description as typed (edit mode).
#[derive(Clone, PartialEq, Default)]
struct Fields {
    kind: String,
    name: String,
    description: String,
}

#[derive(Default)]
pub struct GroupingPage {
    list: TrackTable,
    /// All its releases shown, alone.
    all_releases: bool,
    sources: crate::ui::edit::sources::SourcesEditor,
    /// Edit mode: what's typed, and what it was filled from.
    fields: Option<(Fields, Fields)>,
    reason: String,
    saving: bool,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
    result: Option<Result<(), String>>,
}

impl GroupingPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let Some(grouping) = catalog.grouping(id).cloned() else {
            ui.label(RichText::new("This grouping isn't in the library (anymore).").color(P.muted));
            return;
        };
        let detail = shell.grouping_details.get(id).value.clone();
        let tracks = catalog.grouping_tracks(id).to_vec();
        let releases = catalog.grouping_albums(id).to_vec();

        // ---- All its releases, alone ----
        if self.all_releases {
            shell.page_takes_back = true;
            let ctx = ui.ctx().clone();
            let mut back = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Backspace));
            ui.horizontal(|ui| {
                back |= ui.button(format!("‹ {}", grouping.name)).on_hover_text("Back to the whole page (Esc)").clicked();
                ui.label(RichText::new(format!("Releases  {}", thousands(releases.len()))).font(font(22.0, bold())));
            });
            ui.add_space(10.0);
            if back {
                self.all_releases = false;
                return;
            }
            egui::ScrollArea::vertical().id_salt(("grouping-releases", id)).auto_shrink([false, false]).show(ui, |ui| {
                super::artist::release_cards(ui, shell, catalog, &releases);
            });
            return;
        }

        // ---- What it is ----
        ui.label(RichText::new(grouping.kind.to_uppercase()).font(font(11.0, semibold())).color(P.accent));
        ui.label(RichText::new(&grouping.name).font(font(40.0, bold())));
        if let Some(d) = detail.as_ref().filter(|d| !d.description.is_empty()) {
            ui.label(RichText::new(&d.description).font(font(14.0, medium())).color(P.muted));
        }
        ui.label(RichText::new(format!("{} tracks · {} releases", thousands(tracks.len()), thousands(releases.len()))).font(font(14.0, medium())).color(P.muted));
        // Who's in it: the artists credited on its tracks.
        if let Some(d) = detail.as_ref().filter(|d| !d.members.is_empty()) {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new("With ").font(font(14.0, medium())).color(P.faint));
                for (n, m) in d.members.iter().enumerate() {
                    if n > 0 {
                        ui.label(RichText::new(", ").font(font(14.0, medium())).color(P.faint));
                    }
                    if widgets::link(ui, &m.name, font(14.0, medium()), P.text) {
                        shell.act(Action::Go(Page::Artist(m.artist_id)));
                    }
                    ui.label(RichText::new(format!(" ({})", m.tracks)).font(font(14.0, medium())).color(P.faint));
                }
            });
        }
        ui.add_space(12.0);
        let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
        if play_button(ui, "grouping-play", play_rect) && !tracks.is_empty() {
            shell.act(Action::Play { tracks: tracks.clone(), start: 0 });
        }
        ui.add_space(18.0);

        if shell.edit_mode {
            self.edit_panel(ui, shell, id, &grouping, detail.as_deref());
            ui.add_space(12.0);
        }

        // ---- Its releases: a row of them (the heading shows all) ----
        if !releases.is_empty() {
            let fit = (((ui.available_width() + 18.0) / 168.0) as usize).max(1);
            let more = releases.len() > fit;
            let heading = if more { format!("Releases  {}  ›", thousands(releases.len())) } else { format!("Releases  {}", thousands(releases.len())) };
            let response = ui.add(egui::Label::new(RichText::new(heading).font(font(18.0, bold()))).sense(Sense::click()));
            if more && response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.all_releases = true;
            }
            ui.add_space(8.0);
            super::artist::release_cards(ui, shell, catalog, &releases[..releases.len().min(fit)]);
            ui.add_space(18.0);
        }

        // ---- Its tracks ----
        if tracks.is_empty() && releases.is_empty() {
            ui.label(RichText::new("Nothing in it yet: add tracks or albums from their pages (edit mode, “Part of”).").color(P.muted));
            return;
        }
        if !tracks.is_empty() {
            ui.label(RichText::new(format!("Tracks  {}", thousands(tracks.len()))).font(font(18.0, bold())));
            ui.add_space(8.0);
            let rows = TrackRows { tracks: Some(&tracks), added: None };
            let options = Options {
                key: "grouping",
                columns: super::tracks::LIBRARY_COLUMNS,
                added: None,
                filter_hint: Some("Filter: words, or artist:… album:… year:…"),
                album: None,
                keys: true,
                scroll: true,
            };
            self.list.show(ui, shell, catalog, &rows, shell.generation, player, &options);
        }
    }

    /// Edit mode: its kind, name and description; its sources; its history.
    fn edit_panel(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, grouping: &crate::catalog::model::Grouping, detail: Option<&crate::details::GroupingDetail>) {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.saving = false;
            if result.is_ok() {
                self.fields = None;
                self.reason.clear();
                shell.grouping_details.refresh(id);
                shell.act(Action::RefreshLibrary);
            }
            self.result = Some(result);
        }
        let description = detail.map(|d| d.description.clone()).unwrap_or_default();
        let was = Fields { kind: grouping.kind.clone(), name: grouping.name.clone(), description };
        // Filled from what it is (again, when that changes and nothing's typed).
        if self.fields.as_ref().is_none_or(|(typed, from)| typed == from && *from != was) {
            self.fields = Some((was.clone(), was.clone()));
        }
        let Some((typed, from)) = self.fields.as_mut() else { return };
        let dirty = typed != from;
        if dirty {
            shell.form_dirty = true;
        }
        super::album::details_panel(ui, "grouping-edit", |ui| {
            egui::Grid::new("grouping-form").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
                ui.label(RichText::new("Kind").color(P.faint));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut typed.kind).desired_width(160.0).hint_text("collective, era, project…"));
                    for kind in KINDS {
                        if widgets::chip(ui, kind, typed.kind == kind) {
                            typed.kind = kind.to_string();
                        }
                    }
                });
                ui.end_row();
                ui.label(RichText::new("Name").color(P.faint));
                ui.add(egui::TextEdit::singleline(&mut typed.name).desired_width(420.0));
                ui.end_row();
                ui.label(RichText::new("Description").color(P.faint));
                ui.add(egui::TextEdit::multiline(&mut typed.description).desired_width(420.0).desired_rows(2));
                ui.end_row();
            });
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional: kept with the change)").desired_width(420.0));
            ui.horizontal(|ui| {
                if ui.add_enabled(dirty && !self.saving, egui::Button::new("Save")).clicked() {
                    self.saving = true;
                    self.result = None;
                    let body = json!({ "kind": typed.kind.trim(), "name": typed.name.trim(), "description": typed.description.trim(), "reason": self.reason.trim() });
                    let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
                    shell.runtime.spawn(async move {
                        let result = api.send_json::<serde_json::Value>(reqwest::Method::PUT, &format!("/api/groupings/{id}"), Some(body)).await.map(|_| ()).map_err(|e| e.message);
                        *answer.lock().unwrap() = Some(result);
                        ctx.request_repaint();
                    });
                }
                if ui.add_enabled(dirty && !self.saving, egui::Button::new("Discard")).clicked() {
                    *typed = from.clone();
                }
                match &self.result {
                    Some(Ok(())) if !dirty => {
                        ui.label(RichText::new("Saved").color(P.accent));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(err.as_str()).color(P.danger));
                    }
                    _ => {}
                }
            });
        });
        ui.add_space(12.0);
        let Some(detail) = detail else { return };
        super::album::details_panel(ui, "grouping-sources", |ui| {
            ui.label(RichText::new("Sources").font(font(14.0, semibold())));
            ui.label(RichText::new("The accounts it is: a song imported with one of these as its artist goes in here, credited to no one as it.").color(P.faint).small());
            self.sources.show(ui, shell, crate::ui::edit::sources::Owner::Grouping(id), &detail.sources);
        });
        ui.add_space(12.0);
        super::album::details_panel(ui, "grouping-history", |ui| {
            ui.label(RichText::new(format!("History ({})", detail.changes.len())).font(font(14.0, semibold())));
            let catalog = shell.catalog.value.clone();
            for c in &detail.changes {
                let what = c
                    .track_id
                    .and_then(|t| catalog.as_ref()?.track_index(t).map(|i| format!("“{}”", catalog.as_ref().unwrap().tracks[i as usize].title)))
                    .or_else(|| c.album_id.and_then(|a| catalog.as_ref()?.album(a).map(|a| format!("“{}” (album)", a.title))))
                    .unwrap_or_default();
                let action = match c.action.as_str() {
                    "created" => "made".to_string(),
                    "edited" => "changed its name or kind".to_string(),
                    "added" => format!("added {what}"),
                    "removed" => format!("took out {what}"),
                    "from_artist" => "made from an artist (its credits, tracks and sources)".to_string(),
                    other => other.to_string(),
                };
                let by = c.by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
                let why = Some(c.reason.trim()).filter(|r| !r.is_empty()).map(|r| format!(" (“{r}”)")).unwrap_or_default();
                ui.label(RichText::new(format!("{} · {action}{by}{why}", c.at.get(..10).unwrap_or(&c.at))).color(P.muted).small());
            }
        });
    }
}
