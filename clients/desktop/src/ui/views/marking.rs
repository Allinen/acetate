//! Marking many tracks in a list and doing something to them all at once: the marks,
//! their keys, the bar of tools for the marked tracks, and those tools' popups. Every list
//! that marks tracks uses this (the track tables, Review's list), so they work alike.
//!
//! A list gives its rows as positions shown (0..len) and `id_at`, the track at one. The
//! box in front of each row (`paint_box`), `x`, or Ctrl+click marks or unmarks one;
//! Shift+click or Shift+↑/↓ a range from where it starts: the row the cursor was on (moved
//! there or clicked), which marks, or a row just (un)marked by itself, whose state the
//! range takes (so ranges unmark too). Ctrl+A marks everything shown; Esc clears (all of
//! them, shown or not; after a change was applied to them too). Marks are kept by track,
//! so sorting and filtering don't lose them.
//!
//! In any mode: star them all (`star`), add them to a playlist, queue them, and Ctrl+E (or
//! the button) opens the change for all of them, in a popup (see `edit::editor`); or, on a
//! page that shows it in place (`change_in_place`: Review, instead of its one track's
//! form), Ctrl+E puts the cursor in it. In edit mode (`tools`) the editing tools too:
//! Ctrl+M with two marked compares them side by side, to merge (see `merge`), in a popup;
//! groupings, credited as, original files.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Vec2};
use uuid::Uuid;

use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Shell};

/// Starring or rating is one request a track: up to this many at once.
const MANY: usize = 1000;

#[derive(Default)]
pub struct Marking {
    /// Marked tracks (by id: sorting and filtering keep them).
    marked: HashSet<Uuid>,
    /// Where a range starts (a row shown), and whether the range marks (or unmarks):
    /// Shift+click and Shift+↑/↓ go from here.
    anchor: Option<(usize, bool)>,
    /// Changing the marked tracks, and whether its popup is open.
    bulk: crate::ui::edit::editor::TrackEditor,
    bulk_open: bool,
    /// The change is shown in the page (`change`), not in a popup.
    in_place: bool,
    /// The popup is for this one track instead (a quick fix: a table's selected track,
    /// nothing marked).
    quick: Option<Uuid>,
    /// The two marked tracks side by side, to merge (Ctrl+M), in a popup.
    merge: Option<super::merge::MergeView>,
    /// The marked tracks' "credited as" (an artist's name on them and their albums).
    credited_as: Option<super::credited_as::CreditedAs>,
    /// The marked tracks' files marked as original files (`edit_files`).
    mark_og: Option<super::mark_og::MarkOg>,
    /// Marked tracks being put in a grouping: the server's answer (how many, or why not).
    grouping_answer: Arc<Mutex<Option<Result<String, String>>>>,
}

impl Marking {
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub fn contains(&self, id: &Uuid) -> bool {
        self.marked.contains(id)
    }

    /// Keeps only the marks `keep` says yes to. Not while a change to them is applying or
    /// its outcome is shown: that's about these tracks (and Close clears them anyway).
    pub fn retain(&mut self, keep: impl Fn(&Uuid) -> bool) {
        if self.bulk.applying_or_applied() {
            return;
        }
        self.marked.retain(|id| keep(id));
    }

    /// One of the tools' popups is open (it takes the keys).
    pub fn popup_open(&self) -> bool {
        self.bulk_open || self.merge.is_some() || self.credited_as.is_some() || self.mark_og.is_some()
    }

    /// Ctrl+M is the marks' (merging the two marked tracks): with tracks marked, or the
    /// merge popup open.
    pub fn takes_ctrl_m(&self) -> bool {
        !self.marked.is_empty() || self.merge.is_some()
    }

    /// The editing tools are offered (merging, groupings, credited as…): in edit mode, and
    /// on Review (all of it is editing).
    fn tools(&self, shell: &Shell) -> bool {
        shell.edit_mode || self.in_place
    }

    /// The marked tracks, in the order shown (marked ones not shown last).
    pub fn ordered(&self, len: usize, id_at: impl Fn(usize) -> Option<Uuid>) -> Vec<Uuid> {
        let shown: Vec<Uuid> = (0..len).filter_map(id_at).filter(|id| self.marked.contains(id)).collect();
        let seen: HashSet<&Uuid> = shown.iter().collect();
        let hidden: Vec<Uuid> = self.marked.iter().filter(|id| !seen.contains(id)).copied().collect();
        shown.into_iter().chain(hidden).collect()
    }

    /// Marks or unmarks one shown row; it becomes where ranges start, and they do as it
    /// did (mark, or unmark).
    pub fn toggle(&mut self, n: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        let mut on = false;
        if let Some(id) = id_at(n)
            && !self.marked.remove(&id)
        {
            self.marked.insert(id);
            on = true;
        }
        self.anchor = Some((n, on));
    }

    /// The cursor is on row `n` (moved there, or clicked): ranges start here, and mark.
    pub fn cursor_at(&mut self, n: usize) {
        self.anchor = Some((n, true));
    }

    /// The rows from the anchor (or `from`) to `to` are marked, or (the anchor was a row
    /// just unmarked) unmarked.
    pub fn range_to(&mut self, from: usize, to: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        let (start, on) = self.anchor.unwrap_or((from, true));
        for n in start.min(to)..=start.max(to) {
            if let Some(id) = id_at(n) {
                if on {
                    self.marked.insert(id);
                } else {
                    self.marked.remove(&id);
                }
            }
        }
        self.anchor = Some((start, on));
    }

    /// A click on row `n` (`selected` before it): Ctrl+click marks or unmarks it,
    /// Shift+click marks the range from where it starts (see `range_to`), a plain click
    /// is where ranges start.
    pub fn click(&mut self, n: usize, selected: usize, modifiers: egui::Modifiers, id_at: impl Fn(usize) -> Option<Uuid>) {
        if modifiers.command {
            self.toggle(n, id_at);
        } else if modifiers.shift {
            self.range_to(selected, n, id_at);
        } else {
            self.cursor_at(n);
        }
    }

    pub fn clear(&mut self) {
        self.marked.clear();
        self.anchor = None;
    }

    /// The editing tools' popups close (leaving edit mode); the change (Ctrl+E) stays.
    pub fn close_popups(&mut self) {
        self.merge = None;
        self.credited_as = None;
        self.mark_og = None;
    }

    /// The change popup for this one track alone (a quick fix).
    pub fn open_quick(&mut self, id: Uuid) {
        self.quick = Some(id);
        self.open_bulk();
    }

    /// The change popup for these tracks: one alone is a quick fix (the marks stay as they
    /// are); several are marked (only these) and changed together.
    pub fn edit(&mut self, ids: Vec<Uuid>) {
        match ids.as_slice() {
            [] => {}
            [one] => self.open_quick(*one),
            _ => {
                self.marked = ids.into_iter().collect();
                self.anchor = None;
                self.open_bulk();
            }
        }
    }

    /// The change for the marked tracks is shown in the page (`change`) rather than in a
    /// popup.
    pub fn change_in_place(&mut self) {
        self.in_place = true;
    }

    fn open_bulk(&mut self) {
        // (In place it's there already: only the cursor goes to it.)
        self.bulk_open = !self.in_place;
        self.bulk.opened();
    }

    /// The change for the marked tracks, in the page (`change_in_place`). Done with it
    /// ("Close" once applied): the marks are cleared, and the page is as before.
    pub fn change(&mut self, ui: &mut egui::Ui, shell: &mut Shell, len: usize, id_at: impl Fn(usize) -> Option<Uuid>, album: Option<Uuid>) {
        let Some(catalog) = shell.catalog.value.clone() else { return };
        let marked = self.ordered(len, id_at);
        ui.label(RichText::new(format!("Change {} tracks", thousands(marked.len()))).font(font(22.0, bold())));
        ui.label(RichText::new("Ctrl+E: the commandline · Ctrl+Enter: apply the preview · Esc (outside the commandline): discard the changes, then unmark them all (once applied: close, unmarking them)").color(P.faint).small());
        ui.add_space(8.0);
        let verbs = self.verbs(shell);
        self.bulk.offer(&verbs);
        if show_editor(&mut self.bulk, ui, shell, &catalog, &marked, album) {
            self.clear();
        }
        if let Some(verb) = self.bulk.take_verb() {
            self.open_verb(shell, verb, marked);
        }
    }

    /// The marked tracks' tools, as the editor's line offers them (verbs).
    fn verbs(&self, shell: &Shell) -> Vec<crate::ui::edit::editor::Verb> {
        use crate::ui::edit::editor::Verb;
        if self.quick.is_some() || !self.tools(shell) {
            return Vec::new();
        }
        let mut verbs = vec![Verb::CreditedAs];
        if self.marked.len() == 2 {
            verbs.insert(0, Verb::MergeTwo);
        }
        if shell.account.can("edit_files") {
            verbs.push(Verb::MarkOriginal);
        }
        verbs
    }

    /// A tool the editor's line asked for, for the marked tracks (in the order shown).
    fn open_verb(&mut self, shell: &Shell, verb: crate::ui::edit::editor::Verb, marked: Vec<Uuid>) {
        use crate::ui::edit::editor::Verb;
        match verb {
            Verb::MergeTwo if marked.len() == 2 => self.merge = Some(super::merge::MergeView::new(marked[0], marked[1]).in_popup()),
            Verb::CreditedAs => self.credited_as = Some(super::credited_as::CreditedAs::open(marked)),
            Verb::MarkOriginal => self.mark_og = Some(super::mark_og::MarkOg::open(shell, marked)),
            _ => {}
        }
    }

    /// Each frame, before the list: the answers come in, and the popups are drawn (first:
    /// they take Esc and Ctrl+Enter before the list sees them). `album`: the list is an
    /// album's (numbering on it).
    pub fn popups(&mut self, ctx: &egui::Context, shell: &mut Shell, player: &PlayerView, len: usize, id_at: impl Fn(usize) -> Option<Uuid> + Copy, album: Option<Uuid>) {
        // Marked tracks put in a grouping: said so, and the lists catch up.
        if let Some(result) = self.grouping_answer.lock().unwrap().take() {
            let text = match result {
                Ok(done) => {
                    shell.act(Action::RefreshLibrary);
                    done
                }
                Err(err) => err,
            };
            shell.playlists.notice = Some((text, std::time::Instant::now()));
        }
        if self.merge.is_some() {
            self.merge_popup(ctx, shell, player);
        }
        if let Some(popup) = &mut self.credited_as
            && popup.show(shell)
        {
            self.credited_as = None;
        }
        if let Some(popup) = &mut self.mark_og
            && popup.show(shell)
        {
            self.mark_og = None;
        }
        // The change for all of them.
        if self.bulk_open {
            shell.modal_open = true;
            let Some(catalog) = shell.catalog.value.clone() else { return };
            let (marked, title) = match self.quick {
                Some(id) => {
                    let title = catalog.track_index(id).map(|i| catalog.track_at(i).title().to_string()).unwrap_or_default();
                    (vec![id], format!("Change “{title}”"))
                }
                None => {
                    let marked = self.ordered(len, id_at);
                    let title = format!("Change {} tracks", thousands(marked.len()));
                    (marked, title)
                }
            };
            let mut done = false;
            let verbs = self.verbs(shell);
            self.bulk.offer(&verbs);
            let closed = crate::ui::modal::show(ctx, "bulk-edit", &title, Vec2::new(1180.0, 760.0), |ui| {
                done = show_editor(&mut self.bulk, ui, shell, &catalog, &marked, album);
            });
            // A tool asked for in the line: its own popup instead (the changes stay: Ctrl+E).
            if let Some(verb) = self.bulk.take_verb() {
                self.bulk_open = false;
                self.open_verb(shell, verb, marked);
                return;
            }
            if closed || done {
                // Closed after applying (Esc, ×): done with it, as Close is.
                if self.bulk.applied() {
                    self.bulk.close();
                }
                self.bulk_open = false;
                self.quick = None;
            }
        }
    }

    /// The keys, for a list that has them (and no popup or text field does): `x` marks
    /// the `selected` row, Ctrl+A everything shown, Esc clears, Ctrl+E changes the marked
    /// tracks, Ctrl+M merges the two marked (edit mode; in a text field too, as on the
    /// track page). The list's own keys for a track (s, p, q…) go to `star`, `add`… while
    /// tracks are marked.
    pub fn keys(&mut self, ctx: &egui::Context, shell: &mut Shell, selected: usize, len: usize, id_at: impl Fn(usize) -> Option<Uuid> + Copy) {
        if shell.palette_open {
            return;
        }
        // Ctrl+M: the two marked tracks side by side, to merge (again: close).
        if !self.bulk_open && self.tools(shell) && self.takes_ctrl_m() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if self.merge.is_some() {
                self.merge = None;
            } else {
                match self.ordered(len, id_at)[..] {
                    // The first one shown is kept (s swaps).
                    [left, right] => self.merge = Some(super::merge::MergeView::new(left, right).in_popup()),
                    ref marked => {
                        let text = format!("Merging is for two marked tracks ({} marked).", thousands(marked.len()));
                        shell.playlists.notice = Some((text, std::time::Instant::now()));
                    }
                }
            }
        }
        if self.popup_open() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (toggle, all, clear, open) = ctx.input_mut(|i| {
            (
                i.key_pressed(egui::Key::X) && i.modifiers.is_none(),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                !self.marked.is_empty() && (!self.bulk.has_changes() || self.bulk.applied()) && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                !self.marked.is_empty() && shell.account.can("edit_metadata") && i.consume_key(egui::Modifiers::COMMAND, egui::Key::E),
            )
        });
        if toggle {
            self.toggle(selected, id_at);
        }
        if all {
            self.marked.extend((0..len).filter_map(id_at));
        }
        if clear {
            if self.bulk.applied() {
                self.bulk.close();
            }
            self.clear();
        }
        if open {
            self.open_bulk();
        }
    }

    /// Stars the marked tracks; all of them starred already: unstars them.
    pub fn star(&self, shell: &mut Shell) {
        // One request a track: not for the whole library at once.
        if self.marked.len() > MANY {
            let text = format!("Starring is for up to {} tracks at once ({} marked).", thousands(MANY), thousands(self.marked.len()));
            shell.playlists.notice = Some((text, std::time::Instant::now()));
            return;
        }
        let on = !self.marked.iter().all(|id| shell.mark(*id).starred);
        for &id in &self.marked {
            if shell.mark(id).starred != on {
                shell.act(Action::ToggleStar(Kind::Track, id));
            }
        }
    }

    /// The marked tracks rated half a star up (or down, `by` -1).
    pub fn rate_by(&self, shell: &mut Shell, by: i16) {
        if self.marked.len() > MANY {
            let text = format!("Rating is for up to {} tracks at once ({} marked).", thousands(MANY), thousands(self.marked.len()));
            shell.playlists.notice = Some((text, std::time::Instant::now()));
            return;
        }
        for &id in &self.marked {
            shell.act(Action::RateBy(Kind::Track, id, by));
        }
    }

    /// The marked tracks added to a playlist (chosen in the palette), in the order shown.
    pub fn add(&self, shell: &mut Shell, len: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        shell.act(Action::AddToPlaylist(self.ordered(len, id_at)));
    }

    /// The marked tracks queued, in the order shown: at the end, or (`next`) first.
    pub fn queue(&self, shell: &mut Shell, catalog: &Catalog, next: bool, len: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        let tracks = self.ordered(len, id_at).into_iter().filter_map(|id| catalog.track_index(id)).collect();
        shell.act(Action::Enqueue { tracks, next });
    }

    /// While tracks are marked: how many, and the tools for them (a row). `salt`: the
    /// list's own (pages with several lists).
    pub fn bar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, len: usize, id_at: impl Fn(usize) -> Option<Uuid> + Copy, salt: &str) {
        if self.marked.is_empty() {
            return;
        }
        // Esc clears the marks (`keys`), not leaving the page.
        shell.form_dirty = true;
        let tools = self.tools(shell);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} marked", thousands(self.marked.len()))).font(font(13.0, semibold())).color(P.accent));
            // In any mode.
            let starred = self.marked.iter().all(|id| shell.mark(*id).starred);
            if ui.button(if starred { "Unstar  (s)" } else { "Star  (s)" }).clicked() {
                self.star(shell);
            }
            if ui.button("Add to a playlist…  (p)").clicked() {
                self.add(shell, len, id_at);
            }
            if ui.button("Queue  (q)").on_hover_text("At the end of the queue (Shift+Q: first)").clicked() {
                self.queue(shell, catalog, false, len, id_at);
            }
            if !self.in_place && shell.account.can("edit_metadata") && ui.button(format!("Change {} tracks…  (Ctrl+E)", thousands(self.marked.len()))).clicked() {
                self.open_bulk();
            }
            if !tools {
                ui.label(RichText::new("the box, x or Ctrl+click (un)marks · Shift+↑/↓ or Shift+click a range · Ctrl+A all shown · Esc clear").color(P.faint).small());
                return;
            }
            // Put them all in a grouping (a collective, an era…), in one go.
            if !catalog.groupings.is_empty() {
                let mut chosen = None;
                egui::ComboBox::from_id_salt(("put-in-grouping", salt)).selected_text("Put in a grouping…").show_ui(ui, |ui| {
                    for g in &catalog.groupings {
                        if ui.selectable_label(false, format!("{} · {}", g.name, g.kind)).clicked() {
                            chosen = Some((g.id, g.name.clone()));
                        }
                    }
                });
                if let Some((grouping, name)) = chosen {
                    let tracks = self.ordered(len, id_at);
                    let body = serde_json::json!({ "track_ids": tracks });
                    let (api, answer, ctx) = (shell.api.clone(), self.grouping_answer.clone(), shell.ctx.clone());
                    shell.runtime.spawn(async move {
                        let result = api
                            .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/groupings/{grouping}/add"), Some(body))
                            .await
                            .map(|v| {
                                let (tracks, albums) = (v["tracks"].as_u64().unwrap_or(0), v["albums"].as_u64().unwrap_or(0));
                                let singles = if albums > 0 { format!(" and {albums} single(s) they're alone on") } else { String::new() };
                                format!("Put {tracks} track(s){singles} in “{name}” (the others were in it already).")
                            })
                            .map_err(|e| format!("Couldn't put them in “{name}”: {}", e.message));
                        *answer.lock().unwrap() = Some(result);
                        ctx.request_repaint();
                    });
                }
            }
            if ui.button("Credited as…").on_hover_text("An artist's name on these tracks and their albums (an alias, or its own name)").clicked() {
                self.credited_as = Some(super::credited_as::CreditedAs::open(self.ordered(len, id_at)));
            }
            // Only for someone allowed to correct files (admins): a very specific fix.
            if shell.account.can("edit_files") && ui.button("Mark as original files…").on_hover_text("Their files are original files (og files) whose tags didn't say so").clicked() {
                self.mark_og = Some(super::mark_og::MarkOg::open(shell, self.ordered(len, id_at)));
            }
            if self.marked.len() == 2 && ui.button("Merge the two…  (Ctrl+M)").clicked() {
                let marked = self.ordered(len, id_at);
                self.merge = Some(super::merge::MergeView::new(marked[0], marked[1]).in_popup());
            }
            ui.label(RichText::new("the box, x or Ctrl+click (un)marks · Shift+↑/↓ or Shift+click a range · Ctrl+A all shown · Esc clear").color(P.faint).small());
        });
        ui.add_space(4.0);
    }

    /// The merge popup, while it's open; closes it when the merge view is done. A merged
    /// track leaves the marks (it's gone), and the lists catch up.
    fn merge_popup(&mut self, ctx: &egui::Context, shell: &mut Shell, player: &PlayerView) {
        use super::merge::Outcome;
        let Some(catalog) = shell.catalog.value.clone() else { return };
        let Some(view) = self.merge.as_mut() else { return };
        shell.modal_open = true;
        let pair = (view.left, view.right);
        let mut outcome = None;
        let closed = crate::ui::modal::show(ctx, "merge-tracks", "Merge two tracks", Vec2::new(1180.0, 760.0), |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| outcome = view.show(ui, shell, &catalog, player));
        });
        match outcome {
            Some(Outcome::Merged { kept, gone }) => {
                self.clear();
                shell.track_details.refresh(kept);
                shell.track_editors.remove(&gone);
                shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.waker);
                shell.act(Action::RefreshLibrary);
                shell.playlists.notice = Some(("Merged.".into(), std::time::Instant::now()));
            }
            Some(Outcome::Shared) => {
                shell.track_details.refresh(pair.0);
                shell.track_details.refresh(pair.1);
                shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.waker);
                shell.act(Action::RefreshLibrary);
            }
            Some(Outcome::NotSame | Outcome::Closed) | None => {}
        }
        if outcome.is_some() || closed {
            self.merge = None;
        }
    }
}

/// The editor for `tracks`. One track's is laid out top to bottom, for a page that
/// scrolls: here it scrolls on its own.
fn show_editor(editor: &mut crate::ui::edit::editor::TrackEditor, ui: &mut egui::Ui, shell: &mut Shell, catalog: &std::sync::Arc<Catalog>, tracks: &[Uuid], album: Option<Uuid>) -> bool {
    if tracks.len() == 1 {
        egui::ScrollArea::vertical().id_salt("marked-editor").auto_shrink([false, false]).show(ui, |ui| editor.show(ui, shell, catalog, tracks, album)).inner
    } else {
        editor.show(ui, shell, catalog, tracks, album)
    }
}

/// The mark's box in front of a row (18×18): ticked when marked.
pub fn paint_box(painter: &egui::Painter, id: impl std::hash::Hash + std::fmt::Debug, rect: egui::Rect, marked: bool, hovered: bool) {
    let ctx = painter.ctx();
    let id = egui::Id::new(("mark-box", id));
    let on = ctx.animate_bool_with_time(id, marked, 0.12);
    let hover = ctx.animate_bool_with_time(id.with("hover"), hovered, 0.1);
    let stroke = widgets::lerp(P.line, P.muted, hover);
    painter.rect_stroke(rect, 4, egui::Stroke::new(1.5, stroke.gamma_multiply(1.0 - on)), egui::StrokeKind::Inside);
    if on > 0.0 {
        // Grows into the box from its middle.
        painter.rect_filled(rect.shrink((1.0 - on) * rect.width() * 0.3), 4, P.accent.gamma_multiply(on));
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "✓", font(13.0, semibold()), P.bg.gamma_multiply(on));
    }
}
