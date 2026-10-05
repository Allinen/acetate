//! A grouping's page: a collective (an account several artists post under), an era, a
//! project… Its members (said by hand: being credited on one of its songs isn't enough),
//! its releases (a row; the heading shows them all), its extras (the first few; the heading
//! shows them all) and its tracks. In edit mode: members added (from the palette, or one of
//! those credited on its songs) and taken out; its kind, name and description; extras
//! attached, linked and unlinked; its sources (the accounts: ingest puts songs from them in
//! it, see the server's `db::groupings`); every change made to it.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use serde_json::json;
use uuid::Uuid;

use super::album::play_button;
use super::artist::{section_heading, PREVIEW_EXTRAS};
use super::track_table::{Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::Catalog;
use crate::ui::edit::extras::{ExtrasView, Owner};
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
    /// All its extras shown, alone.
    all_extras: bool,
    extras: ExtrasView,
    sources: crate::ui::edit::sources::SourcesEditor,
    /// Edit mode: what's typed, and what it was filled from.
    fields: Option<(Fields, Fields)>,
    reason: String,
    saving: bool,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
    result: Option<Result<(), String>>,
    /// "Add a member…" asked the palette for an artist.
    picking_member: bool,
    member_answer: Arc<Mutex<Option<Result<(), String>>>>,
    member_error: Option<String>,
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
        let extras = detail.as_ref().map(|d| d.extras.clone()).unwrap_or_default();

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

        // ---- All its extras, alone ----
        if self.all_extras {
            shell.page_takes_back = true;
            let ctx = ui.ctx().clone();
            let mut back = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Backspace));
            ui.horizontal(|ui| {
                back |= ui.button(format!("‹ {}", grouping.name)).on_hover_text("Back to the whole page (Esc)").clicked();
                ui.label(RichText::new(format!("Extras  {}", thousands(extras.iter().filter(|e| !e.hidden).count()))).font(font(22.0, bold())));
            });
            ui.add_space(10.0);
            if back {
                self.all_extras = false;
                return;
            }
            egui::ScrollArea::vertical().id_salt(("grouping-extras", id)).auto_shrink([false, false]).show(ui, |ui| {
                self.extras.show(ui, shell, Owner::Grouping(id), &extras, false, true);
            });
            return;
        }

        // One scroll for the whole page (header, edit panels, releases, extras, the tracks as
        // tall as they are): nothing in it scrolls on its own.
        egui::ScrollArea::vertical().id_salt(("grouping-page", id)).auto_shrink([false, false]).show(ui, |ui| {
            // ---- What it is ----
            ui.label(RichText::new(grouping.kind.to_uppercase()).font(font(11.0, semibold())).color(P.accent));
            ui.label(RichText::new(&grouping.name).font(font(40.0, bold())));
            if let Some(d) = detail.as_ref().filter(|d| !d.description.is_empty()) {
                ui.label(RichText::new(&d.description).font(font(14.0, medium())).color(P.muted));
            }
            ui.label(RichText::new(format!("{} tracks · {} releases", thousands(tracks.len()), thousands(releases.len()))).font(font(14.0, medium())).color(P.muted));
            // Who's in it (said by hand), and in edit mode who else is credited on its songs.
            if let Some(d) = detail.as_ref() {
                self.members(ui, shell, id, d);
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

            // ---- Its extras (a collective's stems, an era's clips…): the first few ----
            if detail.is_some() && ExtrasView::worth_showing(shell, &extras) {
                let shown = extras.iter().filter(|e| !e.hidden).count();
                // (Always `›`: the section is there even with none, for attaching one.)
                if section_heading(ui, "Extras", shown, true) {
                    self.all_extras = true;
                }
                // The first few shown (and those taken off, for edit mode's "Taken off").
                let preview: Vec<crate::details::Extra> = extras.iter().filter(|e| !e.hidden).take(PREVIEW_EXTRAS).chain(extras.iter().filter(|e| e.hidden)).cloned().collect();
                self.extras.show(ui, shell, Owner::Grouping(id), &preview, false, true);
                if shown > PREVIEW_EXTRAS && ui.small_button(format!("All {} ›", thousands(shown))).clicked() {
                    self.all_extras = true;
                }
                ui.add_space(18.0);
            }

            // ---- Its tracks ----
            if tracks.is_empty() && releases.is_empty() {
                ui.label(RichText::new("Nothing in it yet: add tracks or albums from their pages (edit mode, “Part of”).").color(P.muted));
            } else if !tracks.is_empty() {
                ui.label(RichText::new(format!("Tracks  {}", thousands(tracks.len()))).font(font(18.0, bold())));
                ui.add_space(8.0);
                let rows = TrackRows { tracks: Some(&tracks), added: None };
                let options = Options {
                    key: "grouping",
                    columns: super::tracks::LIBRARY_COLUMNS,
                    added: None,
                    filter_hint: Some("Filter: words, or artist:… album:… year:…"),
                    album: None,
                    places: None,
                    keys: true,
                    scroll: false,
                };
                self.list.show(ui, shell, catalog, &rows, shell.generation, player, &options);
            }
        });
    }

    /// Its members, as links (with on how many of its tracks each is credited). In edit
    /// mode: × takes one out; "Add a member…" picks one in the palette; those credited on
    /// its songs who aren't members are offered, one click each.
    fn members(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, d: &crate::details::GroupingDetail) {
        if let Some(result) = self.member_answer.lock().unwrap().take() {
            match result {
                Ok(()) => {
                    self.member_error = None;
                    shell.grouping_details.refresh(id);
                    shell.act(Action::RefreshLibrary);
                }
                Err(err) => self.member_error = Some(err),
            }
        }
        if self.picking_member
            && let Some(picked) = shell.picker.take()
        {
            self.picking_member = false;
            if let (crate::ui::edit::PickKind::Artist, Some(artist)) = (picked.kind, picked.id) {
                self.send_members(shell, id, "add", artist);
            }
        }
        let editing = shell.edit_mode && shell.account.can("edit_metadata");
        if d.members.is_empty() && !editing {
            return;
        }
        let mut remove = None;
        let mut add = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new("Members ").font(font(14.0, medium())).color(P.faint));
            if d.members.is_empty() {
                ui.label(RichText::new("none yet").font(font(14.0, medium())).color(P.faint));
            }
            for (n, m) in d.members.iter().enumerate() {
                if n > 0 {
                    ui.label(RichText::new(", ").font(font(14.0, medium())).color(P.faint));
                }
                if widgets::link(ui, &m.name, font(14.0, medium()), P.text) {
                    shell.act(Action::Go(Page::Artist(m.artist_id)));
                }
                ui.label(RichText::new(format!(" ({})", m.tracks)).font(font(14.0, medium())).color(P.faint));
                if editing && ui.small_button("×").on_hover_text("Not a member").clicked() {
                    remove = Some(m.artist_id);
                }
            }
            if editing {
                ui.add_space(8.0);
                if ui.small_button("Add a member…").clicked() {
                    self.picking_member = true;
                    shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: None });
                }
            }
        });
        if editing && !d.credited.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Also credited on its songs (not members):").color(P.faint).small());
                for m in d.credited.iter().take(20) {
                    if widgets::chip(ui, &format!("+ {}  {}", m.name, m.tracks), false) {
                        add = Some(m.artist_id);
                    }
                }
            });
        }
        if let Some(artist) = remove {
            self.send_members(shell, id, "remove", artist);
        }
        if let Some(artist) = add {
            self.send_members(shell, id, "add", artist);
        }
        if let Some(err) = &self.member_error {
            ui.label(RichText::new(err.as_str()).color(P.danger));
        }
    }

    fn send_members(&mut self, shell: &Shell, id: Uuid, action: &'static str, artist: Uuid) {
        let (api, answer, ctx) = (shell.api.clone(), self.member_answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let path = format!("/api/groupings/{id}/members/{action}");
            let result = api.send_json::<serde_json::Value>(reqwest::Method::POST, &path, Some(json!({ "artist_ids": [artist] }))).await.map(|_| ()).map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
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
                    "added" if c.detail.get("along_with").is_some() => format!("added {what} (along with its single or its track)"),
                    "added" => format!("added {what}"),
                    "removed" if c.detail.get("along_with").is_some() => format!("took out {what} (along with its single or its track)"),
                    "removed" => format!("took out {what}"),
                    "member_added" | "member_removed" => {
                        let name = c.artist_id.and_then(|a| catalog.as_ref()?.artist(a).map(|a| a.name.clone())).unwrap_or_else(|| "an artist".into());
                        if c.action == "member_added" { format!("{name} is a member") } else { format!("{name} isn't a member") }
                    }
                    "from_artist" => "made from an artist (its credits, tracks, sources and extras)".to_string(),
                    other => other.to_string(),
                };
                let by = c.by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
                let why = Some(c.reason.trim()).filter(|r| !r.is_empty()).map(|r| format!(" (“{r}”)")).unwrap_or_default();
                ui.label(RichText::new(format!("{} · {action}{by}{why}", c.at.get(..10).unwrap_or(&c.at))).color(P.muted).small());
            }
        });
    }
}
