//! Review's "Unlinked extras": extras linked to nothing (music videos imported from disk…),
//! to link in bulk. Marked like tracks are (`Marking`: the box in front, `x`, Ctrl+click,
//! Shift+click or Shift+↑/↓ for a range, Ctrl+A, Esc), then: choose what they go to (a
//! track, album, artist or grouping), type its name and Enter, and every marked extra is
//! linked to it. With none marked the cursor's extra is the one (as a track table's quick
//! fix), and its name is typed into the search to start with (it's only text: change it).
//!
//! Enter (or a click on a row) puts the cursor in the search, Esc takes it out again; `i`
//! opens the cursor's extra.
//!
//! Links are the same `POST /api/extras/{id}/links` the extra's own page uses, one per extra.
//!
//! The sixth choice makes tracks of them, from their YouTube videos (the id at the end of the
//! name, "Title [id].mp4"): Look asks the server what each would become (title, artists,
//! video; `POST /api/extras/{id}/to-track` with `preview`), and Make does it for those that
//! can be. Each extra is taken off (kept), its file now a track's.
//!
//! The fifth choice merges them into another extra (the same file twice, say): the extra is
//! chosen by name, what moves is shown, and Merge does it (`POST /api/extras/{id}/merge`; the
//! marked ones go to the trash, where taking one out undoes it).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Sense, Vec2};
use uuid::Uuid;

use super::marking::{paint_box, Key, Marking};
use crate::catalog::model::Extra;
use crate::catalog::search::Part;
use crate::catalog::Catalog;
use crate::ui::edit::extras::{files_text, merge_request, MergeReport, Owner};
use super::extra_drafts::ExtraDrafts;
use crate::ui::edit::inline_search::{describe, Choice, InlineSearch};
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

const ROW: f32 = 52.0;
const SEARCH_ID: &str = "unlinked-extras-search";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Track,
    Album,
    Artist,
    Grouping,
    /// Merged into another extra.
    Merge,
    /// Made into tracks, from their YouTube videos.
    Tracks,
}

/// A merge being looked over: what would move, before it's done.
struct Pending {
    into: (Uuid, String),
    sources: Vec<Uuid>,
    report: Option<MergeReport>,
}

impl Target {
    const ALL: [Target; 6] = [Target::Track, Target::Album, Target::Artist, Target::Grouping, Target::Merge, Target::Tracks];

    fn name(self) -> &'static str {
        match self {
            Target::Track => "track",
            Target::Album => "album",
            Target::Artist => "artist",
            Target::Grouping => "grouping",
            Target::Merge => "merge into an extra",
            Target::Tracks => "make tracks (YouTube)",
        }
    }

    fn part(self) -> Option<Part> {
        match self {
            Target::Track => Some(Part::Tracks),
            Target::Album => Some(Part::Albums),
            Target::Artist => Some(Part::Artists),
            Target::Grouping | Target::Tracks => None,
            Target::Merge => Some(Part::Extras),
        }
    }
}

pub struct UnlinkedExtras {
    marking: Marking,
    /// The row the cursor is on (of those shown).
    cursor: usize,
    /// Bring the cursor into view.
    scroll_to: bool,
    /// The list's scroll position and height, as last drawn.
    view: (f32, f32),
    target: Target,
    search: InlineSearch,
    /// The one extra the search was last started for, and the text put in.
    prefilled: Option<(Uuid, String)>,
    /// Linked here, hidden at once (the library catches up a moment later).
    linked: HashSet<Uuid>,
    sending: bool,
    result: Arc<Mutex<Option<Result<String, String>>>>,
    said: Option<Result<String, String>>,
    pending: Option<Pending>,
    preview: Arc<Mutex<Option<Result<MergeReport, String>>>>,
    /// Making tracks: these extras (and their names), the editor over their drafts.
    drafts: Option<(Vec<(Uuid, String)>, ExtraDrafts)>,
}

impl Default for UnlinkedExtras {
    fn default() -> Self {
        UnlinkedExtras {
            marking: Marking::default(),
            cursor: 0,
            scroll_to: false,
            view: (0.0, 400.0),
            target: Target::Track,
            search: InlineSearch::new(Part::Tracks),
            prefilled: None,
            linked: HashSet::new(),
            sending: false,
            result: Arc::default(),
            said: None,
            pending: None,
            preview: Arc::default(),
            drafts: None,
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
        if let Some(result) = self.preview.lock().unwrap().take() {
            match result {
                Ok(report) => {
                    if let Some(p) = self.pending.as_mut() {
                        p.report = Some(report);
                    }
                }
                Err(err) => {
                    self.pending = None;
                    self.said = Some(Err(err));
                }
            }
        }
        // Linked ones leave the list at once; forget them once the library says so too.
        self.linked.retain(|id| catalog.extras.iter().any(|e| e.id == *id && e.links.is_empty()));
        if self.drafts.is_some() {
            self.drafts_ui(ui, shell, catalog);
            return;
        }
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
        let unlinked: HashSet<Uuid> = catalog.extras.iter().filter(|e| e.links.is_empty()).map(|e| e.id).collect();
        self.marking.retain(|id| unlinked.contains(id) && !self.linked.contains(id));
        self.cursor = self.cursor.min(shown.len().saturating_sub(1));

        ui.label(RichText::new("Linked to nothing yet. Mark some (x, or the boxes), choose what they belong to, type its name, Enter. Nothing marked: the one under the cursor.").color(P.muted));
        ui.add_space(6.0);
        if shown.is_empty() {
            if let Some(said) = self.said.clone() {
                self.say(ui, said);
            }
            ui.label(RichText::new("None (or nothing matches the filter).").color(P.faint));
            return;
        }
        let id_at = |n: usize| shown.get(n).map(|e| (e.id, 0u32));
        self.keys(ui.ctx(), shell, &shown, id_at);
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

    /// The extras an action is for: the marked ones (in the order shown), or the cursor's.
    fn targets(&self, shown: &[&Extra]) -> Vec<Uuid> {
        if self.marking.is_empty() {
            return shown.get(self.cursor).map(|e| e.id).into_iter().collect();
        }
        self.marking.ordered(shown.len(), |n| shown.get(n).map(|e| (e.id, 0)))
    }

    fn keys(&mut self, ctx: &egui::Context, shell: &mut Shell, shown: &[&Extra], id_at: impl Fn(usize) -> Option<Key> + Copy) {
        if shell.palette_open || shell.modal_open {
            return;
        }
        let before = self.cursor;
        let page = ((self.view.1 / ROW) as usize).max(1);
        let moved = widgets::move_selection(ctx, &mut self.cursor, shown.len(), 1, page);
        if moved.moved {
            self.scroll_to = true;
            if ctx.input(|i| i.modifiers.shift) {
                self.marking.range_to(before, self.cursor, id_at);
            } else {
                self.marking.cursor_at(self.cursor);
            }
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (toggle, all, clear, info) = ctx.input_mut(|i| {
            (
                i.key_pressed(egui::Key::X) && i.modifiers.is_none(),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                !self.marking.is_empty() && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.key_pressed(egui::Key::I) && i.modifiers.is_none(),
            )
        });
        if toggle {
            self.marking.toggle(self.cursor, id_at);
        }
        if all {
            for n in 0..shown.len() {
                if let Some(key) = id_at(n)
                    && !self.marking.contains(&key)
                {
                    self.marking.toggle(n, id_at);
                }
            }
        }
        if clear {
            self.marking.clear();
        }
        if info && let Some(e) = shown.get(self.cursor) {
            shell.act(Action::Go(Page::Extra(e.id)));
        }
        if moved.enter {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, shown: &[&Extra]) {
        let targets = self.targets(shown);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if self.marking.is_empty() { "Cursor row".to_string() } else { format!("{} marked", targets.len()) }).font(font(13.0, semibold())));
            if ui.small_button("Mark all shown").clicked() {
                for n in 0..shown.len() {
                    if !self.marking.contains(&(shown[n].id, 0)) {
                        self.marking.toggle(n, |n| shown.get(n).map(|e| (e.id, 0)));
                    }
                }
            }
            if ui.add_enabled(!self.marking.is_empty(), egui::Button::new("Clear marks").small()).clicked() {
                self.marking.clear();
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Link to").font(font(13.0, semibold())));
            ui.spacing_mut().item_spacing.x = 6.0;
            for target in Target::ALL {
                if widgets::chip(ui, target.name(), self.target == target) && self.target != target {
                    self.target = target;
                    self.prefilled = None;
                    self.pending = None;
                    if let Some(part) = target.part() {
                        self.search = InlineSearch::new(part);
                    }
                }
            }
        });
        ui.add_space(4.0);
        // Never offered itself (set before the name is typed in, so the first answer is clean).
        self.search.skip = if self.target == Target::Merge { targets.clone() } else { Vec::new() };
        // One extra: its name is typed in, as a start (moving to another changes it).
        let one = if let [only] = targets[..] { catalog.extras.iter().find(|e| e.id == only) } else { None };
        if self.target.part().is_some() && self.prefilled.as_ref().map(|p| p.0) != one.map(|e| e.id) {
            let had = self.prefilled.take();
            if had.is_some_and(|(_, text)| text == self.search.query) {
                self.search.set_query("", shell);
            }
            if let Some(e) = one {
                let text = search_text(&e.name);
                self.search.set_query(&text, shell);
                self.prefilled = Some((e.id, text));
            }
        }
        let sending = self.sending;
        match self.target.part() {
            Some(part) => {
                let hint = if self.target == Target::Merge {
                    "type the name of the extra to keep, Enter shows what would move".to_string()
                } else {
                    format!("type the {}'s name, Enter links {}", self.target.name(), if targets.len() == 1 { "it".to_string() } else { format!("{} extras", targets.len()) })
                };
                let hit = self.search.show(ui, shell, Some(catalog), egui::Id::new(SEARCH_ID), &hint, &[]);
                if let (Some((Choice::Hit(i), _)), false) = (hit, sending)
                    && self.target == Target::Merge
                {
                    if let Some(e) = catalog.extras.get(i as usize).filter(|_| !targets.is_empty()) {
                        self.said = None;
                        self.pending = Some(Pending { into: (e.id, e.name.clone()), sources: targets.clone(), report: None });
                        let (api, ctx, into, sources, id) = (shell.api.clone(), shell.ctx.clone(), self.preview.clone(), targets.clone(), e.id);
                        shell.runtime.spawn(async move {
                            *into.lock().unwrap() = Some(merge_request(&api, id, &sources, true).await);
                            ctx.request_repaint();
                        });
                    }
                } else if let (Some((Choice::Hit(i), _)), false) = (hit, sending) {
                    let link = match part {
                        Part::Tracks => catalog.get_track(i).map(|t| Owner::Track(t.id())),
                        Part::Albums => catalog.get_album(i).map(|a| Owner::Album(a.id())),
                        _ => catalog.artists.get(i as usize).map(|a| Owner::Artist(a.id)),
                    };
                    let (title, sub) = describe(catalog, part, i);
                    if let Some(link) = link {
                        self.link_all(shell, targets, link, format!("{title} {sub}").trim().to_string());
                    }
                }
            }
            None if self.target == Target::Tracks => self.tracks_ui(ui, catalog, &targets),
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
                    self.link_all(shell, targets, Owner::Grouping(id), name);
                }
            }
        }
        let mut decided = None;
        if let Some(p) = &self.pending {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Merge into “{}”", p.into.1)).font(font(13.0, semibold())));
            match &p.report {
                None => {
                    ui.label(RichText::new("Looking…").color(P.muted));
                }
                Some(report) => {
                    report.show(ui);
                    ui.label(RichText::new("The merged ones go to the trash; taking one out of there undoes the merge.").color(P.faint).small());
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!self.sending, egui::Button::new("Merge")).clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                }
            }
        }
        match decided {
            Some(true) => {
                if let Some(p) = self.pending.take() {
                    self.merge_all(shell, p);
                }
            }
            Some(false) => self.pending = None,
            None => {}
        }
        if self.sending {
            ui.label(RichText::new("Working…").color(P.muted));
        } else if let Some(said) = self.said.clone() {
            self.say(ui, said);
        }
    }

    /// Making tracks: the drafts of the marked extras, the editor over them.
    fn tracks_ui(&mut self, ui: &mut egui::Ui, catalog: &Arc<Catalog>, targets: &[Uuid]) {
        ui.label(RichText::new("Each becomes a track from its YouTube video, found by the [id] at the end of its name: title, artist (or channel), date and thumbnail as YouTube says. You edit the tracks before they're made; the extras are taken off (kept) and the tracks play their files.").color(P.muted));
        let named: Vec<(Uuid, String)> = targets.iter().filter_map(|id| catalog.extras.iter().find(|e| e.id == *id)).map(|e| (e.id, e.name.clone())).collect();
        let label = if named.len() == 1 { "Make it a track…".to_string() } else { format!("Make {} tracks…", named.len()) };
        if ui.add_enabled(!named.is_empty() && !self.sending, egui::Button::new(label)).clicked() {
            self.said = None;
            self.drafts = Some((named, ExtraDrafts::new(Default::default())));
        }
    }

    /// The editor over the drafts, in place of the list.
    fn drafts_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        let busy = self.drafts.as_ref().is_some_and(|(_, d)| d.busy());
        if ui.add_enabled(!busy, egui::Button::new("← Back to the list")).clicked() {
            self.drafts = None;
            return;
        }
        let Some((extras, drafts)) = self.drafts.as_mut() else { return };
        ui.add_space(4.0);
        let made = drafts.show(ui, shell, catalog, extras);
        if !made.is_empty() {
            let gone: HashSet<Uuid> = made.iter().map(|m| m.0).collect();
            self.linked.extend(gone.iter().copied());
            self.marking.retain(|id| !gone.contains(id));
            extras.retain(|(id, _)| !gone.contains(id));
            let tracks = crate::ui::count(made.len(), "track");
            self.said = Some(Ok(format!("Made {tracks}.")));
            if extras.is_empty() {
                self.drafts = None;
            }
        }
    }

    fn merge_all(&mut self, shell: &Shell, pending: Pending) {
        self.sending = true;
        self.said = None;
        self.prefilled = None;
        self.linked.extend(pending.sources.iter().copied());
        self.marking.clear();
        let (api, ctx, result) = (shell.api.clone(), shell.ctx.clone(), self.result.clone());
        shell.runtime.spawn(async move {
            let (into, name) = pending.into;
            let said = merge_request(&api, into, &pending.sources, false).await.map(|r| {
                let n = r.merged.len();
                format!("Merged {} into “{name}”.", if n == 1 { "1 extra".to_string() } else { format!("{n} extras") })
            });
            *result.lock().unwrap() = Some(said);
            ctx.request_repaint();
        });
    }

    fn link_all(&mut self, shell: &Shell, extras: Vec<Uuid>, to: Owner, label: String) {
        if extras.is_empty() {
            return;
        }
        self.sending = true;
        self.said = None;
        self.prefilled = None;
        self.linked.extend(extras.iter().copied());
        self.marking.clear();
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
        let id_at = |n: usize| shown.get(n).map(|e| (e.id, 0u32));
        let modifiers = ui.input(|i| i.modifiers);
        let (mut boxed, mut clicked) = (None, None);
        let mut area = egui::ScrollArea::vertical().id_salt("unlinked-extras").auto_shrink([false, false]);
        // The cursor out of view: bring it in (an end of the view).
        if std::mem::take(&mut self.scroll_to) {
            let step = ROW + ui.spacing().item_spacing.y;
            let (top, height) = self.view;
            let (row_top, row_bottom) = (self.cursor as f32 * step, (self.cursor + 1) as f32 * step);
            if row_top < top {
                area = area.vertical_scroll_offset(row_top);
            } else if row_bottom > top + height {
                area = area.vertical_scroll_offset(row_bottom - height);
            }
        }
        let out = area.show_rows(ui, ROW, shown.len(), |ui, range| {
            for n in range {
                let e = shown[n];
                let marked = self.marking.contains(&(e.id, 0));
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(760.0), ROW), Sense::click());
                if n == self.cursor {
                    ui.painter().rect_filled(rect, 8, P.surface);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, 8, P.surface.gamma_multiply(0.5));
                }
                let tick = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(18.0));
                let box_response = ui.interact(tick.expand(4.0), ui.id().with(("mark-extra", e.id)), Sense::click());
                paint_box(ui.painter(), (e.id, 0u32), tick, marked, box_response.hovered());
                let thumb = egui::Rect::from_min_size(egui::pos2(rect.left() + 40.0, rect.top() + 6.0), Vec2::new(71.0, 40.0));
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
                } else if box_response.on_hover_text("Mark (x)").clicked() {
                    boxed = Some(n);
                } else if response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = Some(n);
                }
            }
        });
        self.view = (out.state.offset.y, out.inner_rect.height());
        if let Some(n) = boxed {
            self.marking.toggle(n, id_at);
            self.cursor = n;
        } else if let Some(n) = clicked {
            if modifiers.command || modifiers.shift {
                self.marking.click(n, self.cursor, modifiers, id_at);
            } else {
                // A plain click marks that row alone (as x would).
                self.marking.clear();
                self.marking.toggle(n, id_at);
            }
            self.cursor = n;
            self.said = None;
            // The name is next.
            ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        }
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
