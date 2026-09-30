//! An extra's page (vocals, stems, a sample…): its files (audio plays, anything can be
//! saved), what it belongs to, who sees it. In edit mode: its name and kind, who sees it
//! (`manage_access`), files added (chosen, pasted or dropped) or taken off, linked to a
//! track, album or artist (or not anymore), the extra taken off or brought back, and what
//! was done to it (`view_history`). See the server's `db::extras`.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::search::Part;
use crate::details::Extra;
use crate::ui::edit::extras::{access_text, file_line, files_text, upload_all, KINDS};
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

#[derive(Default)]
pub struct ExtraPage {
    id: Option<Uuid>,
    loaded: Option<Result<Extra, String>>,
    arrived: Slot<Extra>,
    /// The form: name and kind (filled from what's loaded).
    name: String,
    kind: String,
    filled: bool,
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

        egui::ScrollArea::vertical().id_salt(("extra-page", id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(format!("EXTRA  ·  {}", extra.kind.to_uppercase())).font(font(11.0, semibold())).color(P.accent));
            ui.label(RichText::new(&extra.name).font(font(34.0, bold())));
            let shown = extra.files.iter().filter(|f| !f.hidden).count();
            let mut facts = vec![files_text(shown), access_text(&extra)];
            if extra.hidden {
                facts.push("taken off".into());
            }
            ui.label(RichText::new(facts.join("  ·  ")).color(P.muted));
            ui.add_space(8.0);
            // What it belongs to.
            if !extra.links.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Belongs to").color(P.faint));
                    for l in &extra.links {
                        let page = match l.kind.as_str() {
                            "track" => Page::Track(l.id),
                            "album" => Page::Album(l.id),
                            _ => Page::Artist(l.id),
                        };
                        if widgets::link(ui, &format!("{} ({})", l.title, l.kind), font(14.0, medium()), P.text) {
                            shell.act(Action::Go(page));
                        }
                        if can && ui.small_button("×").on_hover_text("Unlink").clicked() {
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
            for f in extra.files.iter().filter(|f| !f.hidden) {
                ui.horizontal(|ui| {
                    file_line(ui, shell, &extra, f, &saved);
                    if can && ui.small_button("Take off").clicked() {
                        self.send(shell, format!("/api/extra-files/{}/hidden", f.id), serde_json::json!({ "hidden": true }), "Taken off (kept).".into());
                    }
                });
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
        let changed = self.name.trim() != extra.name || self.kind.trim() != extra.kind;
        if ui.add_enabled(changed && !self.sending && !self.name.trim().is_empty(), egui::Button::new("Save")).clicked() {
            let body = serde_json::json!({ "name": self.name.trim(), "kind": self.kind.trim() });
            self.send_with(shell, reqwest::Method::PUT, format!("/api/extras/{id}"), body, "Saved.".into());
        }
        ui.add_space(10.0);

        // Linking it to a track, album or artist.
        ui.horizontal(|ui| {
            for (part, text) in [(Part::Tracks, "Link to a track…"), (Part::Albums, "Link to an album…"), (Part::Artists, "Link to an artist…")] {
                if ui.button(text).clicked() {
                    self.linking = Some((part, InlineSearch::new(part)));
                }
            }
        });
        if let Some((part, search)) = self.linking.as_mut() {
            let catalog = shell.catalog.value.clone();
            let hit = search.show(ui, shell, catalog.as_deref(), egui::Id::new("extra-link"), "type its name", &[]);
            if let (Some((Choice::Hit(i), _)), Some(c)) = (hit, catalog.as_deref()) {
                let (kind, target) = match part {
                    Part::Tracks => ("track", c.tracks.get(i as usize).map(|t| t.id)),
                    Part::Albums => ("album", c.albums.get(i as usize).map(|a| a.id)),
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
        let count = files.len();
        shell.runtime.spawn(async move {
            let result = upload_all(&api, &id.to_string(), &files, &progress, &ctx).await.map(|_| format!("{} added.", files_text(count)));
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
