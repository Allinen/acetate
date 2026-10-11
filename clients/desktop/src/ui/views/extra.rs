//! An extra's page (vocals, stems, a sample…): its files (audio plays, anything can be
//! saved), what it belongs to, who sees it. In edit mode: its name and kind, who sees it
//! (`manage_access`), files added (chosen, pasted or dropped) or taken off, linked to a
//! track, album, artist or grouping (or not anymore), the extra taken off or brought back,
//! other extras merged into it (previewed first; the old ones go to the trash, where
//! taking them out undoes it), and what was done to it (`view_history`). See the server's `db::extras`.
//!
//! Keys: what it belongs to and its files are one list. ↑/↓ or j/k move, Enter opens a
//! link or plays (watches) a file, `d` saves the file, and in edit mode Delete takes the
//! file off or unlinks it (both kept, undone in the page).

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::details::Extra;
use crate::ui::edit::extras::{access_text, file_line, files_text, merge_request, upload_all, MergeReport, KINDS};
use super::extra_drafts::ExtraDrafts;
use crate::ui::edit::inline_search::{Choice, InlineSearch};
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

type Slot<T> = Arc<Mutex<Option<Result<T, String>>>>;

#[derive(Debug, Clone, Deserialize)]
struct LogEntry {
    what: String,
    #[serde(default)]
    reason: String,
    at: String,
    by: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Group {
    id: Uuid,
    name: String,
}

/// Extras being gathered into this one: chosen, then previewed, then merged.
struct Merging {
    search: InlineSearch,
    picked: Vec<(Uuid, String)>,
    preview: Option<MergeReport>,
}

#[derive(Default)]
pub struct ExtraPage {
    id: Option<Uuid>,
    loaded: Option<Result<Extra, String>>,
    arrived: Slot<Extra>,
    /// The form: name and kind (filled from what's loaded).
    name: String,
    kind: String,
    description: String,
    source: String,
    filled: bool,
    /// "Make it a track again" was clicked: asking to be sure.
    asked_back: bool,
    back: Slot<Uuid>,
    /// "Make it a track" (from its YouTube video) is open: the video's link, blank for the
    /// id at the end of its name.
    /// Once asked for, the editor over its draft.
    making: Option<(String, Option<ExtraDrafts>)>,
    /// Who sees it, being changed (`manage_access`): restricted, and the groups.
    access: Option<(bool, Vec<Uuid>)>,
    groups: Option<Result<Vec<Group>, String>>,
    groups_arrived: Slot<Vec<Group>>,
    /// Linking it to something: which kind is being searched.
    linking: Option<(Part, InlineSearch)>,
    history: Option<Result<Vec<LogEntry>, String>>,
    history_arrived: Slot<Vec<LogEntry>>,
    chosen: Arc<Mutex<Vec<PathBuf>>>,
    progress: Arc<AtomicUsize>,
    sending: bool,
    answer: Slot<String>,
    saved: Slot<String>,
    result: Option<Result<String, String>>,
    /// The row the keys are on: its links first, then its files.
    selected: usize,
    merging: Option<Merging>,
    previewing: bool,
    preview_arrived: Slot<MergeReport>,
}

/// The page of what an extra belongs to.
fn link_page(l: &crate::details::ExtraLink) -> Page {
    match l.kind.as_str() {
        "track" => Page::Track(l.id),
        "album" => Page::Album(l.id),
        "grouping" => Page::Grouping(l.id),
        _ => Page::Artist(l.id),
    }
}

impl ExtraPage {
    fn ask(&mut self, shell: &Shell, id: Uuid) {
        let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), self.arrived.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<Extra>(reqwest::Method::GET, &format!("/api/extras/{id}"), None).await.map_err(|e| e.message);
            *into.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// "Make it a track…": the video's link (or the `[id]` in its name), then the track
    /// editor on the draft, as for many extras at once.
    fn make_track(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, name: &str) {
        let Some((link, drafts)) = self.making.as_mut() else {
            if ui.button("Make it a track from its YouTube video…").clicked() {
                self.making = Some((String::new(), None));
            }
            ui.add_space(8.0);
            return;
        };
        let catalog = shell.catalog.value.clone();
        let mut close = false;
        let mut went = None;
        match (drafts, catalog) {
            (Some(drafts), Some(catalog)) => {
                if ui.add_enabled(!drafts.busy(), egui::Button::new("Cancel")).clicked() {
                    close = true;
                }
                went = drafts.show(ui, shell, &catalog, &[(id, name.to_string())]).first().map(|m| m.1);
            }
            (drafts, _) => {
                ui.label(RichText::new("It becomes a track with what YouTube says of the video (title, artist or channel, date, thumbnail), which you can change before it's made. It plays this file; this extra is then taken off (kept).").color(P.muted));
                ui.add(egui::TextEdit::singleline(link).hint_text("the video's link, or blank for the [id] at the end of its name").desired_width(420.0));
                ui.horizontal(|ui| {
                    if ui.button("Look").clicked() {
                        let links = Some(link.trim().to_string()).filter(|l| !l.is_empty()).map(|l| [(id, l)].into()).unwrap_or_default();
                        *drafts = Some(ExtraDrafts::new(links));
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            }
        }
        if close {
            self.making = None;
        }
        if let Some(track) = went {
            shell.act(Action::RefreshLibrary);
            shell.act(Action::Go(Page::Track(track)));
        }
        ui.add_space(8.0);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid) {
        if self.id != Some(id) {
            *self = ExtraPage { id: Some(id), ..Default::default() };
            self.ask(shell, id);
        }
        let arrived_taken = self.arrived.lock().unwrap().take();
        if let Some(result) = arrived_taken {
            if let Ok(e) = &result
                && !self.filled
            {
                self.name = e.name.clone();
                self.kind = e.kind.clone();
                self.description = e.description.clone();
                self.source = e.source.clone();
                self.filled = true;
            }
            self.loaded = Some(result);
        }
        let groups_arrived_taken = self.groups_arrived.lock().unwrap().take();
        if let Some(groups) = groups_arrived_taken {
            self.groups = Some(groups);
        }
        let history_arrived_taken = self.history_arrived.lock().unwrap().take();
        if let Some(history) = history_arrived_taken {
            self.history = Some(history);
        }
        // Made a track again: its page.
        let back_taken = self.back.lock().unwrap().take();
        match back_taken {
            Some(Ok(track)) => {
                shell.act(Action::RefreshLibrary);
                shell.act(Action::Go(Page::Track(track)));
                return;
            }
            Some(Err(err)) => {
                self.sending = false;
                self.result = Some(Err(err));
            }
            None => {}
        }
        let saved_taken = self.saved.lock().unwrap().take();
        if let Some(saved) = saved_taken {
            self.result = Some(saved);
        }
        let answer_taken = self.answer.lock().unwrap().take();
        if let Some(result) = answer_taken {
            self.sending = false;
            self.result = Some(result);
            self.filled = false;
            self.access = None;
            self.history = None;
            self.ask(shell, id);
            shell.act(Action::RefreshLibrary);
        }
        let extra = match &self.loaded {
            None => {
                ui.label(RichText::new("Loading…").color(P.muted));
                return;
            }
            Some(Err(err)) => {
                ui.label(RichText::new(format!("This extra isn't there (or you can't see it): {err}")).color(P.muted));
                return;
            }
            Some(Ok(e)) => e.clone(),
        };
        let can = shell.edit_mode && shell.account.can("edit_metadata");
        // Files given (dropped, pasted, chosen) in edit mode: added.
        let mut files = std::mem::take(&mut *self.chosen.lock().unwrap());
        if can && !shell.palette_open && !shell.modal_open {
            files.extend(crate::ui::files::given(ui.ctx()));
        }
        if can && !files.is_empty() && !self.sending {
            self.upload(shell, id, files);
        }

        // The keys: its links, then its files, as one list.
        let ctx = ui.ctx().clone();
        let shown_files: Vec<&crate::details::ExtraFile> = extra.files.iter().filter(|f| !f.hidden).collect();
        let rows = extra.links.len() + shown_files.len();
        let moved = widgets::move_selection(&ctx, &mut self.selected, rows, 1, 10);
        self.selected = self.selected.min(rows.saturating_sub(1));
        let free = !ctx.egui_wants_keyboard_input() && !shell.palette_open && !shell.modal_was_open;
        let link_at = |n: usize| extra.links.get(n);
        let file_at = |n: usize| n.checked_sub(extra.links.len()).and_then(|n| shown_files.get(n).copied());
        if moved.enter {
            if let Some(l) = link_at(self.selected) {
                shell.act(Action::Go(link_page(l)));
            } else if let Some(f) = file_at(self.selected) {
                if f.is_video() {
                    shell.watch(f.id, &extra.name);
                } else {
                    crate::ui::edit::extras::play_file(shell, &extra.name, f.id, f.sha256.clone(), &f.name, f.duration_ms);
                }
            }
        }
        if free && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::D))
            && let Some(f) = file_at(self.selected)
        {
            crate::ui::edit::extras::save_file(shell, f.id, &f.name, &self.saved);
        }
        if can && free && !self.sending && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
            if let Some(l) = link_at(self.selected) {
                let link = serde_json::json!({ "kind": l.kind, "id": l.id });
                self.send(shell, format!("/api/extras/{id}/links"), serde_json::json!({ "link": link, "undo": true }), "Unlinked.".into());
            } else if let Some(f) = file_at(self.selected) {
                self.send(shell, format!("/api/extra-files/{}/hidden", f.id), serde_json::json!({ "hidden": true }), "Taken off (kept).".into());
            }
        }
        // The row the keys are on: outlined, and brought into view when it moves.
        let selected = self.selected;
        let mark = |ui: &mut egui::Ui, response: &egui::Response, n: usize| {
            if n == selected && rows > 0 {
                ui.painter().rect_stroke(response.rect.expand(3.0), 6, egui::Stroke::new(1.5, P.accent), egui::StrokeKind::Outside);
                if moved.moved {
                    response.scroll_to_me(None);
                }
            }
        };

        egui::ScrollArea::vertical().id_salt(("extra-page", id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(format!("EXTRA  ·  {}", extra.kind.to_uppercase())).font(font(11.0, semibold())).color(P.accent));
            ui.label(RichText::new(&extra.name).font(font(34.0, bold())));
            if let Some(into) = extra.merged_into {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Merged into another extra: this one is in the trash (taking it out of there undoes the merge).").color(P.accent));
                    if ui.button("Go to the extra").clicked() {
                        shell.act(Action::Go(Page::Extra(into)));
                    }
                });
            }
            let shown = extra.files.iter().filter(|f| !f.hidden).count();
            let mut facts = vec![files_text(shown), access_text(&extra)];
            if extra.hidden {
                facts.push("taken off".into());
            }
            ui.label(RichText::new(facts.join("  ·  ")).color(P.muted));
            ui.add_space(6.0);
            crate::ui::edit::extras::about(ui, &extra.description, &extra.source, None);
            ui.add_space(8.0);
            // Made from a track (hidden while it's this): it can be a track again.
            if extra.from_track.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Made from a track (hidden while it's an extra: nothing was deleted).").color(P.muted));
                    if can && !self.asked_back && ui.button("Make it a track again…").clicked() {
                        self.asked_back = true;
                    }
                });
                if self.asked_back {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("The track comes back as it was (its albums too), and this extra is taken off (kept).").color(P.accent));
                        if ui.add_enabled(!self.sending, egui::Button::new("Make it a track again")).clicked() {
                            self.sending = true;
                            let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), self.back.clone());
                            shell.runtime.spawn(async move {
                                let result = api
                                    .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/extras/{id}/back-to-track"), Some(serde_json::json!({})))
                                    .await
                                    .map_err(|e| e.message)
                                    .and_then(|v| v["track_id"].as_str().and_then(|t| t.parse::<Uuid>().ok()).ok_or_else(|| "no track in the answer".to_string()));
                                *into.lock().unwrap() = Some(result);
                                ctx.request_repaint();
                            });
                        }
                        if ui.button("Cancel").clicked() {
                            self.asked_back = false;
                        }
                    });
                }
                ui.add_space(8.0);
            }
            // Its file was made a track: that can be undone (the extra comes back).
            if let Some(track) = extra.made_track {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Its file was made a track (this extra is taken off, kept).").color(P.muted));
                    if ui.button("Go to the track").clicked() {
                        shell.act(Action::Go(Page::Track(track)));
                    }
                    if can && ui.add_enabled(!self.sending, egui::Button::new("Undo: back to an extra")).on_hover_text("The track is hidden (it can be made a track again), and this extra is brought back").clicked() {
                        self.send(shell, format!("/api/extras/{id}/undo-track"), serde_json::json!({}), "The track is hidden; this extra is back.".into());
                    }
                });
                ui.add_space(8.0);
            }
            // A YouTube video kept as an extra can be a track, from what YouTube says of it.
            if can && extra.from_track.is_none() && !extra.hidden && shown == 1 {
                self.make_track(ui, shell, id, &extra.name);
            }
            // What it belongs to.
            if !extra.links.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Belongs to").color(P.faint));
                    for (n, l) in extra.links.iter().enumerate() {
                        let response = ui.add(egui::Label::new(RichText::new(format!("{} ({})", l.title, l.kind)).font(font(14.0, medium())).color(P.text)).sense(egui::Sense::click()));
                        mark(ui, &response, n);
                        if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            shell.act(Action::Go(link_page(l)));
                        }
                        if can && ui.small_button("×").on_hover_text("Unlink (Delete)").clicked() {
                            let link = serde_json::json!({ "kind": l.kind, "id": l.id });
                            self.send(shell, format!("/api/extras/{id}/links"), serde_json::json!({ "link": link, "undo": true }), "Unlinked.".into());
                        }
                    }
                });
                ui.add_space(8.0);
            }

            // ---- Its files ----
            ui.label(RichText::new("Files").font(font(17.0, bold())));
            let saved = self.saved.clone();
            let mut take_off = None;
            for (n, f) in shown_files.iter().enumerate() {
                let response = ui
                    .horizontal(|ui| {
                        file_line(ui, shell, &extra, f, &saved);
                        if can && ui.small_button("Take off").on_hover_text("Delete").clicked() {
                            take_off = Some(f.id);
                        }
                    })
                    .response;
                mark(ui, &response, extra.links.len() + n);
            }
            if let Some(file) = take_off {
                self.send(shell, format!("/api/extra-files/{file}/hidden"), serde_json::json!({ "hidden": true }), "Taken off (kept).".into());
            }
            if rows > 0 {
                let text = if can { "↑/↓ choose · Enter opens or plays · d saves the file · Delete takes it off (or unlinks)" } else { "↑/↓ choose · Enter opens or plays · d saves the file" };
                ui.label(RichText::new(text).color(P.faint).small());
            }
            if can {
                ui.horizontal_wrapped(|ui| {
                    if ui.add_enabled(!self.sending, egui::Button::new("Add files…")).clicked() {
                        crate::ui::files::choose(shell, "Files to add", ("Any file", &[]), true, &self.chosen);
                    }
                    ui.label(RichText::new("or paste copied files (Ctrl+V), or drop them on the window").color(P.faint).small());
                });
                let taken_off: Vec<_> = extra.files.iter().filter(|f| f.hidden).collect();
                if !taken_off.is_empty() {
                    egui::CollapsingHeader::new(RichText::new(format!("Files taken off ({})", taken_off.len())).color(P.muted)).show(ui, |ui| {
                        for f in taken_off {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&f.name).color(P.muted));
                                if ui.small_button("Bring back").clicked() {
                                    self.send(shell, format!("/api/extra-files/{}/hidden", f.id), serde_json::json!({ "hidden": false }), "Brought back.".into());
                                }
                            });
                        }
                    });
                }
            }
            self.result_line(ui);

            if can {
                ui.add_space(14.0);
                self.edit(ui, shell, &extra);
            }
            if shell.account.can("view_history") {
                ui.add_space(14.0);
                self.history_list(ui, shell, id);
            }
        });
    }

    fn result_line(&self, ui: &mut egui::Ui) {
        if self.sending {
            ui.label(RichText::new("Working…").color(P.muted));
        }
        match &self.result {
            Some(Ok(text)) => {
                ui.label(RichText::new(text.as_str()).color(P.accent));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            None => {}
        }
    }

    /// Edit mode: name and kind, linking, who sees it, taking it off.
    fn edit(&mut self, ui: &mut egui::Ui, shell: &mut Shell, extra: &Extra) {
        let id = extra.id;
        ui.label(RichText::new("Edit").font(font(17.0, bold())));
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(360.0));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Kind");
            for k in KINDS {
                if widgets::chip(ui, k, self.kind == *k) {
                    self.kind = k.to_string();
                }
            }
            ui.add(egui::TextEdit::singleline(&mut self.kind).desired_width(120.0));
        });
        ui.add(egui::TextEdit::multiline(&mut self.description).hint_text("Description (optional)").desired_rows(3).desired_width(f32::INFINITY));
        ui.add(egui::TextEdit::singleline(&mut self.source).hint_text("Source (optional): a link, or where it came from").desired_width(f32::INFINITY));
        let changed = self.name.trim() != extra.name || self.kind.trim() != extra.kind || self.description.trim() != extra.description || self.source.trim() != extra.source;
        if ui.add_enabled(changed && !self.sending && !self.name.trim().is_empty(), egui::Button::new("Save")).clicked() {
            let body = serde_json::json!({ "name": self.name.trim(), "kind": self.kind.trim(), "description": self.description.trim(), "source": self.source.trim() });
            self.send_with(shell, reqwest::Method::PUT, format!("/api/extras/{id}"), body, "Saved.".into());
        }
        ui.add_space(10.0);

        // Linking it to a track, album, artist (found by typing) or grouping (there are few).
        let mut to_grouping = None;
        ui.horizontal(|ui| {
            for (part, text) in [(Part::Tracks, "Link to a track…"), (Part::Albums, "Link to an album…"), (Part::Artists, "Link to an artist…")] {
                if ui.button(text).clicked() {
                    self.linking = Some((part, InlineSearch::new(part)));
                }
            }
            let catalog = shell.catalog.value.clone();
            let groupings: Vec<_> = catalog.iter().flat_map(|c| c.groupings.iter()).filter(|g| !extra.links.iter().any(|l| l.id == g.id)).collect();
            if !groupings.is_empty() {
                egui::ComboBox::from_id_salt(("extra-link-grouping", id)).selected_text("Link to a grouping…").show_ui(ui, |ui| {
                    for g in groupings {
                        if ui.selectable_label(false, format!("{} · {}", g.name, g.kind)).clicked() {
                            to_grouping = Some(g.id);
                        }
                    }
                });
            }
        });
        if let Some(grouping) = to_grouping {
            let body = serde_json::json!({ "link": { "kind": "grouping", "id": grouping } });
            self.send(shell, format!("/api/extras/{id}/links"), body, "Linked.".into());
        }
        if let Some((part, search)) = self.linking.as_mut() {
            let catalog = shell.catalog.value.clone();
            let hit = search.show(ui, shell, catalog.as_deref(), egui::Id::new("extra-link"), "type its name", &[]);
            if let (Some((Choice::Hit(i), _)), Some(c)) = (hit, catalog.as_deref()) {
                let (kind, target) = match part {
                    Part::Tracks => ("track", c.get_track(i).map(|t| t.id())),
                    Part::Albums => ("album", c.get_album(i).map(|a| a.id())),
                    _ => ("artist", c.artists.get(i as usize).map(|a| a.id)),
                };
                if let Some(target) = target {
                    self.linking = None;
                    let body = serde_json::json!({ "link": { "kind": kind, "id": target } });
                    self.send(shell, format!("/api/extras/{id}/links"), body, "Linked.".into());
                }
            }
        }
        ui.add_space(10.0);
        self.merge_editor(ui, shell, extra);
        ui.add_space(10.0);

        // Who sees it.
        if shell.account.can("manage_access") {
            self.access_editor(ui, shell, extra);
            ui.add_space(10.0);
        }
        let (text, hidden) = if extra.hidden { ("Bring it back", false) } else { ("Take it off", true) };
        if ui.add_enabled(!self.sending, egui::Button::new(text)).on_hover_text("Hidden, not deleted").clicked() {
            self.send(shell, format!("/api/extras/{id}/hidden"), serde_json::json!({ "hidden": hidden }), if hidden { "Taken off (kept)." } else { "Brought back." }.into());
        }
    }

    /// Other extras gathered into this one (the same file twice, or pieces of one thing):
    /// chosen by name, previewed (what moves, what stays), then merged in one go.
    fn merge_editor(&mut self, ui: &mut egui::Ui, shell: &mut Shell, extra: &Extra) {
        let id = extra.id;
        let preview_taken = self.preview_arrived.lock().unwrap().take();
        if let Some(result) = preview_taken {
            self.previewing = false;
            match result {
                Ok(report) => {
                    if let Some(m) = self.merging.as_mut() {
                        m.preview = Some(report);
                    }
                }
                Err(err) => self.result = Some(Err(err)),
            }
        }
        if extra.merged_into.is_some() || extra.hidden || extra.from_track.is_some() {
            return;
        }
        let Some(m) = self.merging.as_mut() else {
            if ui
                .button("Merge other extras into this one…")
                .on_hover_text("Their files and links come here; they go to the trash (taking one out undoes it)")
                .clicked()
            {
                self.merging = Some(Merging { search: InlineSearch::new(Part::Extras), picked: Vec::new(), preview: None });
            }
            return;
        };
        ui.label(RichText::new("Merge into this one").font(font(14.0, semibold())));
        ui.label(RichText::new("Their files and links come here (a file or link this one has already stays behind). Each goes to the trash; an admin can purge it after a week, and taking it out before undoes the merge. They must be seen by the same people.").color(P.muted).small());
        let catalog = shell.catalog.value.clone();
        m.search.skip = std::iter::once(id).chain(m.picked.iter().map(|(p, _)| *p)).collect();
        if let Some((Choice::Hit(i), _)) = m.search.show(ui, shell, catalog.as_deref(), egui::Id::new("extra-merge"), "type the name of an extra to add", &[])
            && let Some(e) = catalog.as_deref().and_then(|c| c.extras.get(i as usize))
            && e.id != id
            && !m.picked.iter().any(|(p, _)| *p == e.id)
        {
            m.picked.push((e.id, e.name.clone()));
            m.preview = None;
        }
        let mut remove = None;
        for (n, (_, name)) in m.picked.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(name.as_str());
                if ui.small_button("×").on_hover_text("Leave it out").clicked() {
                    remove = Some(n);
                }
            });
        }
        if let Some(n) = remove {
            m.picked.remove(n);
            m.preview = None;
        }
        if let Some(report) = &m.preview {
            report.show(ui);
        }
        let ready = m.preview.is_some();
        let nothing = m.picked.is_empty();
        let ids: Vec<Uuid> = m.picked.iter().map(|(p, _)| *p).collect();
        let (mut preview, mut merge, mut cancel) = (false, false, false);
        ui.horizontal(|ui| {
            preview = ui.add_enabled(!nothing && !self.previewing && !self.sending, egui::Button::new("Preview")).clicked();
            merge = ui.add_enabled(ready && !self.sending, egui::Button::new("Merge")).on_hover_text("Preview first, so you see what moves").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            self.merging = None;
        } else if preview {
            self.previewing = true;
            self.result = None;
            let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), self.preview_arrived.clone());
            shell.runtime.spawn(async move {
                let result = merge_request(&api, id, &ids, true).await;
                *into.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        } else if merge {
            self.merging = None;
            let body = serde_json::json!({ "sources": ids });
            self.send(shell, format!("/api/extras/{id}/merge"), body, "Merged. The old ones are in the trash (taking one out undoes it).".into());
        }
    }

    /// Restricted or not, and the groups that see it.
    fn access_editor(&mut self, ui: &mut egui::Ui, shell: &mut Shell, extra: &Extra) {
        ui.label(RichText::new("Who sees it").font(font(14.0, semibold())));
        let Some((restricted, groups)) = self.access.as_mut() else {
            ui.horizontal(|ui| {
                ui.label(RichText::new(access_text(extra)).color(P.muted));
                if ui.small_button("Change…").clicked() {
                    self.access = Some((extra.is_restricted, extra.groups.iter().map(|g| g.id).collect()));
                    if self.groups.is_none() {
                        let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), self.groups_arrived.clone());
                        shell.runtime.spawn(async move {
                            let result = api.send_json::<Vec<Group>>(reqwest::Method::GET, "/api/groups", None).await.map_err(|e| e.message);
                            *into.lock().unwrap() = Some(result);
                            ctx.request_repaint();
                        });
                    }
                }
            });
            return;
        };
        ui.checkbox(restricted, "Restricted (admins, and the groups chosen)");
        if *restricted {
            ui.horizontal_wrapped(|ui| match &self.groups {
                None => {
                    ui.label(RichText::new("Loading the groups…").color(P.muted));
                }
                Some(Err(err)) => {
                    ui.label(RichText::new(err.as_str()).color(P.danger));
                }
                Some(Ok(all)) => {
                    for g in all.iter().filter(|g| g.name != "admin") {
                        let on = groups.contains(&g.id);
                        if widgets::chip(ui, &g.name, on) {
                            if on {
                                groups.retain(|x| *x != g.id);
                            } else {
                                groups.push(g.id);
                            }
                        }
                    }
                }
            });
        }
        let (restricted, groups) = (*restricted, groups.clone());
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.sending, egui::Button::new("Save who sees it")).clicked() {
                let body = serde_json::json!({ "restricted": restricted, "groups": if restricted { groups.clone() } else { Vec::new() } });
                self.send_with(shell, reqwest::Method::PUT, format!("/api/extras/{}/access", extra.id), body, "Saved who sees it.".into());
            }
            if ui.button("Cancel").clicked() {
                self.access = None;
            }
        });
    }

    fn history_list(&mut self, ui: &mut egui::Ui, shell: &Shell, id: Uuid) {
        let open = egui::CollapsingHeader::new(RichText::new("History").font(font(17.0, bold()))).id_salt(("extra-history", id)).show(ui, |ui| match &self.history {
            None => {
                ui.label(RichText::new("Loading…").color(P.muted));
            }
            Some(Err(err)) => {
                ui.label(RichText::new(err.as_str()).color(P.danger));
            }
            Some(Ok(entries)) => {
                for e in entries {
                    let by = e.by.as_deref().map(|b| format!(" · {b}")).unwrap_or_default();
                    let why = if e.reason.is_empty() { String::new() } else { format!(" · “{}”", e.reason) };
                    ui.label(RichText::new(format!("{}{by} · {}{why}", e.at.get(..16).unwrap_or(&e.at).replace('T', " "), e.what)).color(P.muted));
                }
            }
        });
        if open.body_returned.is_some() && self.history.is_none() {
            self.history = Some(Ok(Vec::new()));
            let (api, ctx, into) = (shell.api.clone(), shell.ctx.clone(), self.history_arrived.clone());
            shell.runtime.spawn(async move {
                let result = api.send_json::<Vec<LogEntry>>(reqwest::Method::GET, &format!("/api/extras/{id}/history"), None).await.map_err(|e| e.message);
                *into.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        }
    }

    fn upload(&mut self, shell: &Shell, id: Uuid, files: Vec<PathBuf>) {
        self.sending = true;
        self.result = None;
        let (api, ctx, answer, progress) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone(), self.progress.clone());
        shell.runtime.spawn(async move {
            let result = upload_all(&api, &id.to_string(), &files, &progress, &ctx, |added| format!("{} added.", files_text(added))).await;
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    fn send(&mut self, shell: &Shell, path: String, body: serde_json::Value, text: String) {
        self.send_with(shell, reqwest::Method::POST, path, body, text);
    }

    fn send_with(&mut self, shell: &Shell, method: reqwest::Method, path: String, body: serde_json::Value, text: String) {
        self.sending = true;
        self.result = None;
        let (api, ctx, answer) = (shell.api.clone(), shell.ctx.clone(), self.answer.clone());
        shell.runtime.spawn(async move {
            let result = api.send_json::<serde_json::Value>(method, &path, Some(body)).await.map(|_| text).map_err(|e| e.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}
