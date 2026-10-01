//! Marking many tracks in a list and changing them all at once: the marks, their keys,
//! the bar of tools for the marked tracks, and those tools' popups. Every list that marks
//! tracks uses this (the track tables in edit mode, Review's list), so they work alike.
//!
//! A list gives its rows as positions shown (0..len) and `id_at`, the track at one. The
//! box in front of each row (`paint_box`), `x`, or Ctrl+click marks or unmarks one;
//! Shift+click or Shift+↑/↓ a range, which takes the state of the row it starts from (so
//! ranges unmark too); Ctrl+A marks everything shown; Esc clears. Marks are kept by
//! track, so sorting and filtering don't lose them. Ctrl+E (or the button) opens the
//! change for all of them, in a popup (see `edit::bulk`); or, on a page that shows it
//! in place (`change_in_place`: Review, instead of its one track's form), Ctrl+E puts
//! the cursor in it. Ctrl+M with two marked compares them side by side, to merge (see
//! `merge`), in a popup.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Vec2};
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{thousands, Action, Shell};

#[derive(Default)]
pub struct Marking {
    /// Marked tracks (by id: sorting and filtering keep them).
    marked: HashSet<Uuid>,
    /// Where a range starts (a row shown): Shift+click and Shift+↑/↓ give the range
    /// this row's state.
    anchor: Option<usize>,
    /// Changing the marked tracks, and whether its popup is open.
    bulk: crate::ui::edit::bulk::BulkEditor,
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

    /// The marked tracks, in the order shown (marked ones not shown last).
    pub fn ordered(&self, len: usize, id_at: impl Fn(usize) -> Option<Uuid>) -> Vec<Uuid> {
        let shown: Vec<Uuid> = (0..len).filter_map(id_at).filter(|id| self.marked.contains(id)).collect();
        let seen: HashSet<&Uuid> = shown.iter().collect();
        let hidden: Vec<Uuid> = self.marked.iter().filter(|id| !seen.contains(id)).copied().collect();
        shown.into_iter().chain(hidden).collect()
    }

    /// Marks or unmarks one shown row; it becomes where ranges start.
    pub fn toggle(&mut self, n: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        if let Some(id) = id_at(n)
            && !self.marked.remove(&id)
        {
            self.marked.insert(id);
        }
        self.anchor = Some(n);
    }

    /// The rows from the anchor (or `from`) to `to` get the anchor row's state: marked,
    /// or (starting from an unmarked one after a click on it) unmarked.
    pub fn range_to(&mut self, from: usize, to: usize, id_at: impl Fn(usize) -> Option<Uuid>) {
        let start = self.anchor.unwrap_or(from);
        let on = self.anchor.is_none() || id_at(start).is_some_and(|id| self.marked.contains(&id));
        for n in start.min(to)..=start.max(to) {
            if let Some(id) = id_at(n) {
                if on {
                    self.marked.insert(id);
                } else {
                    self.marked.remove(&id);
                }
            }
        }
        self.anchor = Some(start);
    }

    /// A click on row `n` (`selected` before it): Ctrl+click marks or unmarks it,
    /// Shift+click gives the range from where it started that row's state, a plain click
    /// is where ranges start.
    pub fn click(&mut self, n: usize, selected: usize, modifiers: egui::Modifiers, id_at: impl Fn(usize) -> Option<Uuid>) {
        if modifiers.command {
            self.toggle(n, id_at);
        } else if modifiers.shift {
            self.range_to(selected, n, id_at);
        } else {
            self.anchor = Some(n);
        }
    }

    pub fn clear(&mut self) {
        self.marked.clear();
        self.anchor = None;
    }

    /// The tools' popups close (leaving edit mode); a quick fix of one track stays.
    pub fn close_popups(&mut self) {
        if self.quick.is_none() {
            self.bulk_open = false;
        }
        self.merge = None;
        self.credited_as = None;
        self.mark_og = None;
    }

    /// The change popup for this one track alone (a quick fix).
    pub fn open_quick(&mut self, id: Uuid) {
        self.quick = Some(id);
        self.open_bulk();
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
        ui.label(RichText::new("Ctrl+E: what to change · Ctrl+Enter: preview, then apply · Esc (outside a field): unmark them all").color(P.faint).small());
        ui.add_space(8.0);
        if self.bulk.show(ui, shell, &catalog, &marked, album) {
            self.clear();
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
        // A bulk change just finished (the popup open or not): the lists catch up.
        if self.bulk.take_finished() {
            shell.act(Action::RefreshLibrary);
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
                    let title = catalog.track_index(id).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_default();
                    (vec![id], format!("Change “{title}”"))
                }
                None => {
                    let marked = self.ordered(len, id_at);
                    let title = format!("Change {} tracks", thousands(marked.len()));
                    (marked, title)
                }
            };
            let mut done = false;
            let closed = crate::ui::modal::show(ctx, "bulk-edit", &title, Vec2::new(1180.0, 760.0), |ui| {
                done = self.bulk.show(ui, shell, &catalog, &marked, album);
            });
            if closed || done {
                self.bulk_open = false;
                self.quick = None;
            }
        }
    }

    /// The keys, for a list that has them (and no popup or text field does): `x` marks
    /// the `selected` row, Ctrl+A everything shown, Esc clears, Ctrl+E changes the marked
    /// tracks, Ctrl+M merges the two marked (in a text field too, as on the track page).
    pub fn keys(&mut self, ctx: &egui::Context, shell: &mut Shell, selected: usize, len: usize, id_at: impl Fn(usize) -> Option<Uuid> + Copy) {
        if shell.palette_open {
            return;
        }
        // Ctrl+M: the two marked tracks side by side, to merge (again: close).
        if !self.bulk_open && self.takes_ctrl_m() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
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
                !self.marked.is_empty() && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                !self.marked.is_empty() && i.consume_key(egui::Modifiers::COMMAND, egui::Key::E),
            )
        });
        if toggle {
            self.toggle(selected, id_at);
        }
        if all {
            self.marked.extend((0..len).filter_map(id_at));
        }
        if clear {
            self.clear();
        }
        if open {
            self.open_bulk();
        }
    }

    /// While tracks are marked: how many, and the tools for them (a row). `salt`: the
    /// list's own (pages with several lists).
    pub fn bar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, len: usize, id_at: impl Fn(usize) -> Option<Uuid> + Copy, salt: &str) {
        if self.marked.is_empty() {
            return;
        }
        // Esc clears the marks (`keys`), not leaving the page.
        shell.form_dirty = true;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} marked", thousands(self.marked.len()))).font(font(13.0, semibold())).color(P.accent));
            if !self.in_place && ui.button(format!("Change {} tracks…  (Ctrl+E)", thousands(self.marked.len()))).clicked() {
                self.open_bulk();
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
            ui.label(RichText::new("the box or x (un)marks · Shift+↑/↓ or Shift+click a range · Ctrl+A all shown · Esc clear").color(P.faint).small());
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
                shell.track_forms.remove(&gone);
                shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
                shell.act(Action::RefreshLibrary);
                shell.playlists.notice = Some(("Merged.".into(), std::time::Instant::now()));
            }
            Some(Outcome::Shared) => {
                shell.track_details.refresh(pair.0);
                shell.track_details.refresh(pair.1);
                shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
                shell.act(Action::RefreshLibrary);
            }
            Some(Outcome::NotSame | Outcome::Closed) | None => {}
        }
        if outcome.is_some() || closed {
            self.merge = None;
        }
    }
}

/// The mark's box in front of a row (18×18): ticked when marked.
pub fn paint_box(painter: &egui::Painter, rect: egui::Rect, marked: bool, hovered: bool) {
    if marked {
        painter.rect_filled(rect, 4, P.accent);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "✓", font(13.0, semibold()), P.bg);
    } else {
        let stroke = if hovered { P.muted } else { P.line };
        painter.rect_stroke(rect, 4, egui::Stroke::new(1.5, stroke), egui::StrokeKind::Inside);
    }
}
