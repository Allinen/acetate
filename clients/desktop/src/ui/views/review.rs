//! The Review page (6): the workbench for fixing the library, hands on the keyboard.
//!
//! Left, the tracks to go through: flagged "fix later" (by default), any open flag, or
//! every track with a problem. Each says what's wrong ("no artists · no album"),
//! worked out from the library kept here (see `catalog::problems`), so filtering and
//! counting are instant even for the whole library. Filter by problem or text, group by
//! folder, note, artist or problem.
//!
//! Right, the selected track's form, with the cursor already in the first field that
//! needs fixing, and the file's name, folder and tags to fix from.
//!
//! Keys: Ctrl+Enter saves, and when nothing's left to fix, marks the flag done and goes
//! to the next track. Ctrl+↓/↑ next/previous (anywhere); outside text fields also j/k
//! or ↓/↑, Space plays the track (or pauses it), u skips, d marks it done as it is.

use std::collections::HashMap;

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::catalog::problems::{self, Problem};
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::review::Flag;
use crate::ui::edit::track_form::TrackForm;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, widgets, Action, Shell};

const ROW: f32 = 46.0;
const LIST_WIDTH: f32 = 380.0;

/// Which tracks the list has.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Source {
    /// Flagged "fix later" at import.
    #[default]
    FixLater,
    /// Any open flag (duplicates, ambiguous artists…).
    Flags,
    /// Every track with a problem, flagged or not.
    Everything,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Group {
    #[default]
    Folder,
    Note,
    Artist,
    Problem,
    None,
}

impl Group {
    const ALL: [Group; 5] = [Group::Folder, Group::Note, Group::Artist, Group::Problem, Group::None];

    fn name(self) -> &'static str {
        match self {
            Group::Folder => "folder",
            Group::Note => "import note",
            Group::Artist => "artist",
            Group::Problem => "problem",
            Group::None => "nothing",
        }
    }
}

/// A track in the list: its place in the catalog, its flags' kinds as the list says
/// them (not "fix later": that's what the list is), what's wrong with it (counted checks
/// only), and the group it's in. (Copied, not positions in the flag list: that list
/// changes the moment a flag is marked done, before the rows are made again.)
struct Entry {
    track: u32,
    kinds: Vec<String>,
    problems: Vec<Problem>,
    group: String,
}

enum Row {
    Header(String, usize),
    Track(usize),
}

#[derive(Default)]
pub struct Review {
    source: Source,
    group: Group,
    only: Option<Problem>,
    filter: String,
    entries: Vec<Entry>,
    rows: Vec<Row>,
    /// How many tracks have each problem (before `only` narrows the list).
    counts: HashMap<Problem, usize>,
    /// What the list was built from: (catalog generation, flags version, filters).
    built_for: Option<(u64, u64, String)>,
    /// Every track's problems as bits (see `bits`), per catalog generation and flags
    /// version: worked out once, not per keystroke.
    bits: Option<(u64, u64, Vec<u8>)>,
    selected: Option<Uuid>,
    /// Where the selected track was in the list (to go on from there once it's left it).
    selected_at: usize,
    /// Scroll the list to the selected track.
    scroll_to_selected: bool,
    /// Put the cursor in the first field to fix, once the form is there.
    focus_first: bool,
    /// A Ctrl+Enter save is running for this track: when it's done and nothing's left to
    /// fix, the flag is marked done and the next track comes.
    advance_after_save: Option<Uuid>,
    /// Tracks fixed or marked done here, this session.
    done: usize,
    notice: Option<String>,
    asked_flags: bool,
}

impl Review {
    pub fn opened(&mut self) {
        self.asked_flags = false;
        self.focus_first = true;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &std::sync::Arc<Catalog>, player: &PlayerView) {
        if !shell.account.can("edit_metadata") {
            ui.label(RichText::new("Review is for fixing the library: it needs the edit_metadata permission.").color(P.muted));
            return;
        }
        for err in shell.flags.poll() {
            shell.controller.report(err);
        }
        if !self.asked_flags {
            self.asked_flags = true;
            shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
        }
        self.build(shell, catalog);
        self.keys(ui.ctx(), shell, catalog, player);
        // A key may have marked a flag done: the rows follow at once.
        self.build(shell, catalog);

        // ---- Top: title, what's listed, filters ----
        ui.horizontal(|ui| {
            ui.label(RichText::new("Review").font(font(28.0, bold())));
            ui.add_space(12.0);
            let fix_later = shell.flags.list.iter().filter(|f| f.kind == "fix_later").count();
            let all = shell.flags.list.len();
            for (source, name) in [
                (Source::FixLater, format!("Fix later ({})", crate::ui::thousands(fix_later))),
                (Source::Flags, format!("All flags ({})", crate::ui::thousands(all))),
                (Source::Everything, "Every track with a problem".to_string()),
            ] {
                if widgets::chip(ui, &name, self.source == source) {
                    self.source = source;
                    self.selected = None;
                    self.focus_first = true;
                }
            }
            if shell.flags.loading {
                ui.label(RichText::new("checking for new flags…").color(P.faint).small());
            } else if let Some(err) = &shell.flags.error {
                ui.label(RichText::new(format!("couldn't check for new flags: {err}")).color(P.danger).small());
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .id(egui::Id::new(super::FILTER_ID))
                    .hint_text("/  filter: title, artist, file, folder")
                    .desired_width(260.0),
            );
            egui::ComboBox::from_id_salt("review-group").selected_text(format!("group by {}", self.group.name())).show_ui(ui, |ui| {
                for group in Group::ALL {
                    ui.selectable_value(&mut self.group, group, group.name());
                }
            });
            ui.add_space(6.0);
            // A chip per counted problem: only tracks with it.
            for problem in shell.settings.review_checks.clone() {
                let count = self.counts.get(&problem).copied().unwrap_or(0);
                let text = format!("{} {}", problem.label(), crate::ui::thousands(count));
                if widgets::chip(ui, &text, self.only == Some(problem)) {
                    self.only = if self.only == Some(problem) { None } else { Some(problem) };
                }
            }
            ui.menu_button("Checks…", |ui| {
                ui.label(RichText::new("What counts as needing a fix:").color(P.muted));
                let mut changed = false;
                for problem in Problem::ALL {
                    let mut on = shell.settings.review_checks.contains(&problem);
                    if ui.checkbox(&mut on, problem.label()).changed() {
                        changed = true;
                        shell.settings.review_checks.retain(|p| *p != problem);
                        if on {
                            shell.settings.review_checks.push(problem);
                            shell.settings.review_checks.sort_by_key(|p| Problem::ALL.iter().position(|q| q == p));
                        }
                    }
                }
                if changed {
                    shell.settings.save(&shell.data_dir);
                    self.bits = None;
                }
            });
        });
        ui.add_space(4.0);
        let left = self.entries.len();
        let mut status = format!("{} to go", crate::ui::thousands(left));
        if self.done > 0 {
            status = format!("{} done this session · {status}", self.done);
        }
        ui.label(RichText::new(status).color(P.faint).small());
        ui.add_space(6.0);

        // ---- The list, and the selected track ----
        let rest = ui.available_rect_before_wrap();
        let list_rect = egui::Rect::from_min_size(rest.min, Vec2::new(LIST_WIDTH, rest.height()));
        let editor_rect = egui::Rect::from_min_max(egui::pos2(list_rect.right() + 24.0, rest.top()), rest.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        self.list(&mut list_ui, shell, catalog, player);
        let mut editor_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
        self.editor(&mut editor_ui, shell, catalog, player);
        ui.allocate_rect(rest, Sense::hover());
    }

    /// Rebuilds the list when the library, the flags or the filters changed.
    fn build(&mut self, shell: &Shell, catalog: &Catalog) {
        let flags = shell.flags.list.clone();
        let checks = shell.settings.review_checks.clone();
        let key = format!("{:?}|{:?}|{:?}|{}|{:?}", self.source, self.group, self.only, self.filter, checks);
        let built_for = (shell.generation, shell.flags.version, key);
        if self.built_for.as_ref() == Some(&built_for) {
            return;
        }
        let file_of: HashMap<Uuid, &str> = flags.iter().filter_map(|f| Some((f.track_id, f.imported_from.as_deref()?))).collect();
        if self.bits.as_ref().is_none_or(|b| (b.0, b.1) != (shell.generation, shell.flags.version)) {
            let bits = catalog
                .tracks
                .iter()
                .map(|t| bits(&problems::of(t, file_of.get(&t.id).map(|p| problems::file_name(p)))))
                .collect();
            self.bits = Some((shell.generation, shell.flags.version, bits));
        }
        let all_bits = &self.bits.as_ref().unwrap().2;
        let counted = bits(&checks);

        // The tracks, in flag order (newest first) or the library's.
        let mut flags_of: HashMap<Uuid, Vec<usize>> = HashMap::new();
        let mut order: Vec<Uuid> = Vec::new();
        for (i, f) in flags.iter().enumerate() {
            let listed = match self.source {
                Source::FixLater => f.kind == "fix_later",
                Source::Flags | Source::Everything => true,
            };
            if listed {
                let entry = flags_of.entry(f.track_id).or_default();
                if entry.is_empty() {
                    order.push(f.track_id);
                }
                entry.push(i);
            }
        }
        let candidates: Vec<u32> = match self.source {
            Source::FixLater | Source::Flags => order.iter().filter_map(|id| catalog.track_index(*id)).collect(),
            Source::Everything => (0..catalog.tracks.len() as u32).filter(|&i| all_bits[i as usize] & counted != 0).collect(),
        };

        let words: Vec<String> = self.filter.to_lowercase().split_whitespace().map(str::to_string).collect();
        self.counts.clear();
        let mut entries = Vec::new();
        for i in candidates {
            let track = &catalog.tracks[i as usize];
            let found: Vec<Problem> = unbits(all_bits[i as usize] & counted);
            let track_flags = flags_of.get(&track.id).cloned().unwrap_or_default();
            let path = file_of.get(&track.id).copied().unwrap_or("");
            if !words.is_empty() {
                let text = format!("{} {} {}", track.title, track.artist_display, path).to_lowercase();
                if !words.iter().all(|w| text.contains(w.as_str())) {
                    continue;
                }
            }
            for p in &found {
                *self.counts.entry(*p).or_default() += 1;
            }
            if self.only.is_some_and(|only| !found.contains(&only)) {
                continue;
            }
            let group = match self.group {
                Group::Folder if path.is_empty() => "(folder not known)".to_string(),
                Group::Folder => problems::folder(path).to_string(),
                Group::Note => track_flags.iter().map(|&f| flags[f].note()).find(|n| !n.is_empty()).unwrap_or("(no note)").to_string(),
                Group::Artist if track.artist_display.is_empty() => "(no artist)".to_string(),
                Group::Artist => track.artist_display.clone(),
                Group::Problem => found.first().map(|p| p.label().to_string()).unwrap_or_else(|| "Nothing found".into()),
                Group::None => String::new(),
            };
            let kinds = track_flags.iter().map(|&f| flags[f].kind_name()).filter(|k| *k != "Fix later").map(str::to_string).collect();
            entries.push(Entry { track: i, kinds, problems: found, group });
        }
        // Groups by name (unknown ones last), the order within kept.
        if self.group != Group::None {
            entries.sort_by(|a, b| (a.group.starts_with('('), &a.group).cmp(&(b.group.starts_with('('), &b.group)));
        }
        let mut rows = Vec::new();
        for (n, e) in entries.iter().enumerate() {
            if self.group != Group::None && (n == 0 || entries[n - 1].group != e.group) {
                let count = entries[n..].iter().take_while(|x| x.group == e.group).count();
                rows.push(Row::Header(e.group.clone(), count));
            }
            rows.push(Row::Track(n));
        }
        self.entries = entries;
        self.rows = rows;
        self.built_for = Some(built_for);

        // The selected track left the list (fixed, marked done): the one now in its place.
        let still_there = self.selected.is_some_and(|id| self.position(catalog, id).is_some());
        if !still_there && !self.entries.is_empty() {
            let at = self.selected_at.min(self.entries.len() - 1);
            self.select(catalog, at);
        }
    }

    fn position(&self, catalog: &Catalog, id: Uuid) -> Option<usize> {
        let index = catalog.track_index(id)?;
        self.entries.iter().position(|e| e.track == index)
    }

    fn select(&mut self, catalog: &Catalog, at: usize) {
        let Some(entry) = self.entries.get(at) else { return };
        self.selected = Some(catalog.tracks[entry.track as usize].id);
        self.selected_at = at;
        self.scroll_to_selected = true;
        self.focus_first = true;
        self.notice = None;
    }

    /// Moves by `by` in the list (and loads the one after, so moving on doesn't wait).
    fn step(&mut self, shell: &mut Shell, catalog: &Catalog, by: isize) {
        if self.entries.is_empty() {
            return;
        }
        let at = self.selected.and_then(|id| self.position(catalog, id)).unwrap_or(self.selected_at);
        let next = (at as isize + by).clamp(0, self.entries.len() as isize - 1) as usize;
        self.select(catalog, next);
        ctx_stop_text(&shell.ctx);
        if let Some(after) = self.entries.get(next + 1) {
            shell.track_details.get(catalog.tracks[after.track as usize].id);
        }
    }

    /// Marks the selected track's flags done (`kinds`: only these kinds; None: all of
    /// those listed), counts it, and goes on to the next.
    fn mark_done(&mut self, shell: &mut Shell, catalog: &Catalog, resolution: &str, only_fix_later: bool) {
        let Some(id) = self.selected else { return };
        let ids: Vec<Uuid> = shell
            .flags
            .list
            .iter()
            .filter(|f| f.track_id == id && (!only_fix_later || f.kind == "fix_later"))
            .filter(|f| self.source != Source::FixLater || f.kind == "fix_later")
            .map(|f| f.id)
            .collect();
        if !ids.is_empty() {
            shell.flags.resolve(&shell.api, shell.runtime.handle(), &ids, resolution);
            self.done += 1;
        }
        // On from here: the list is rebuilt without it, and the one in its place is next.
        if let Some(at) = self.position(catalog, id) {
            self.selected_at = at;
        }
        // Without a flag to take it off the list (or listed by its problems, which the
        // library shows fixed a moment later): on to the next now.
        if ids.is_empty() || self.source == Source::Everything {
            self.step(shell, catalog, 1);
        }
    }

    fn keys(&mut self, ctx: &egui::Context, shell: &mut Shell, catalog: &Catalog, player: &PlayerView) {
        if shell.palette_open {
            return;
        }
        let (save, next, previous) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
            )
        });
        if next {
            self.step(shell, catalog, 1);
        }
        if previous {
            self.step(shell, catalog, -1);
        }
        if save && let Some(id) = self.selected {
            // (Taken out of the shell to save it: saving needs the shell too.)
            match shell.track_forms.remove(&id) {
                Some(mut form) if form.dirty() => {
                    form.save(shell);
                    shell.track_forms.insert(id, form);
                    self.advance_after_save = Some(id);
                }
                // Nothing changed: it was checked and is fine as it is.
                form => {
                    if let Some(form) = form {
                        shell.track_forms.insert(id, form);
                    }
                    self.mark_done(shell, catalog, "checked: fine as it is", true);
                }
            }
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
        let mut acted = true;
        if key(Key::J) || key(Key::ArrowDown) {
            self.step(shell, catalog, 1);
        } else if key(Key::K) || key(Key::ArrowUp) {
            self.step(shell, catalog, -1);
        } else if key(Key::U) {
            self.step(shell, catalog, 1);
        } else if key(Key::D) {
            self.mark_done(shell, catalog, "done", false);
        } else if key(Key::Space)
            && let Some(id) = self.selected
        {
            if playing_track(player) == Some(id) {
                shell.act(Action::TogglePause);
            } else if let Some(index) = catalog.track_index(id) {
                shell.act(Action::Play { tracks: vec![index], start: 0 });
            }
        } else {
            acted = false;
        }
        // The key's character is still in this frame's input: taken out, or the next
        // track's first field (which gets the cursor right away) would type it ("d").
        if acted {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
        }
    }

    fn list(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, player: &PlayerView) {
        if self.rows.is_empty() {
            let text = match (self.source, shell.flags.list.is_empty() && shell.flags.loading) {
                (_, true) => "Loading the flags…",
                (Source::FixLater, _) => "Nothing flagged “fix later” (or nothing matches the filters).",
                _ => "Nothing to fix here (or nothing matches the filters).",
            };
            ui.label(RichText::new(text).color(P.muted));
            return;
        }
        let selected_row = self.selected.and_then(|id| self.position(catalog, id)).and_then(|e| self.rows.iter().position(|r| matches!(r, Row::Track(n) if *n == e)));
        let mut area = egui::ScrollArea::vertical().id_salt("review-list").auto_shrink([false, false]);
        if self.scroll_to_selected {
            self.scroll_to_selected = false;
            if let Some(row) = selected_row {
                area = area.vertical_scroll_offset((row as f32 * ROW - ui.available_height() / 2.0).max(0.0));
            }
        }
        let playing = playing_track(player);
        let mut clicked = None;
        area.show_rows(ui, ROW, self.rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for r in range {
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                let painter = ui.painter();
                match &self.rows[r] {
                    Row::Header(name, count) => {
                        let text = format!("{name}  ·  {count}");
                        widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 22.0), Vec2::new(rect.width() - 12.0, 16.0)), &text, font(11.5, semibold()), P.faint);
                    }
                    Row::Track(n) => {
                        let e = &self.entries[*n];
                        let t = &catalog.tracks[e.track as usize];
                        if Some(r) == selected_row {
                            painter.rect_filled(rect, 8, P.surface_hi);
                        } else if response.hovered() {
                            painter.rect_filled(rect, 8, P.surface);
                        }
                        let title = if t.title.trim().is_empty() { "(no title)" } else { &t.title };
                        let color = if playing == Some(t.id) { P.accent } else { P.text };
                        let unsaved = shell.track_forms.get(&t.id).is_some_and(|f| f.dirty());
                        let title = if unsaved { format!("● {title}") } else { title.to_string() };
                        widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(10.0, 6.0), Vec2::new(rect.width() - 20.0, 18.0)), &title, font(13.5, medium()), color);
                        let mut sub: Vec<String> = Vec::new();
                        if !t.artist_display.is_empty() {
                            sub.push(t.artist_display.clone());
                        }
                        if !e.problems.is_empty() {
                            sub.push(e.problems.iter().map(|p| p.label().to_lowercase()).collect::<Vec<_>>().join(", "));
                        }
                        if !e.kinds.is_empty() {
                            sub.push(e.kinds.join(", "));
                        }
                        widgets::text_in(painter, egui::Rect::from_min_size(rect.min + Vec2::new(10.0, 25.0), Vec2::new(rect.width() - 20.0, 16.0)), &sub.join("  ·  "), font(12.0, medium()), P.muted);
                        if response.clicked() {
                            clicked = Some(*n);
                        }
                    }
                }
            }
        });
        if let Some(n) = clicked {
            self.select(catalog, n);
            self.scroll_to_selected = false;
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &std::sync::Arc<Catalog>, player: &PlayerView) {
        let Some(id) = self.selected else { return };
        let Some(index) = catalog.track_index(id) else { return };
        let track = &catalog.tracks[index as usize];
        let flags: Vec<Flag> = shell.flags.list.iter().filter(|f| f.track_id == id).cloned().collect();
        let path = flags.iter().find_map(|f| f.imported_from.clone());
        let file_name = path.as_deref().map(problems::file_name);
        let cached = shell.track_details.get(id);
        let (detail, error) = (cached.value.clone(), cached.error.clone());

        egui::ScrollArea::vertical().id_salt(("review-editor", id)).auto_shrink([false, false]).show(ui, |ui| {
            // ---- What it is, and what's wrong ----
            ui.horizontal(|ui| {
                let size = Vec2::splat(64.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                shell.cover(ui, track.cover(), id, rect, 6, 0.2);
                ui.vertical(|ui| {
                    let title = if track.title.trim().is_empty() { "(no title)" } else { &track.title };
                    ui.label(RichText::new(title).font(font(22.0, bold())));
                    ui.label(RichText::new(if track.artist_display.is_empty() { "(no artists)" } else { &track.artist_display }).color(P.muted));
                    let playing = playing_track(player) == Some(id) && !player.paused;
                    let text = if playing { "Pause  (Space)" } else { "Play  (Space)" };
                    if ui.small_button(text).clicked() {
                        if playing_track(player) == Some(id) {
                            shell.act(Action::TogglePause);
                        } else {
                            shell.act(Action::Play { tracks: vec![index], start: 0 });
                        }
                    }
                });
            });
            ui.add_space(6.0);
            if let Some(path) = &path {
                ui.label(RichText::new(format!("File: {}", problems::file_name(path))).color(P.text));
                ui.label(RichText::new(format!("in {}", problems::folder(path))).color(P.faint).small());
            }
            for f in &flags {
                let mut line = f.kind_name().to_string();
                if !f.note().is_empty() {
                    line = format!("{line}: {}", f.note());
                }
                if let Some(reason) = f.related.get("reason").and_then(|r| r.as_str()) {
                    line = format!("{line} ({reason})");
                }
                ui.label(RichText::new(line).color(P.muted));
                related(ui, shell, f);
            }

            let Some(d) = detail else {
                let text = error.map(|e| format!("Couldn't load the track: {e}")).unwrap_or_else(|| "Loading…".into());
                ui.label(RichText::new(text).color(P.faint));
                return;
            };
            let Some(mut form) = shell.track_forms.remove(&id).or_else(|| TrackForm::from_detail(&d, Some(catalog))) else {
                ui.label(RichText::new("This track has no version to edit.").color(P.muted));
                return;
            };

            // What's still wrong, as the form has it now.
            let checks = shell.settings.review_checks.clone();
            let has_cover = track.cover_id.is_some();
            let left: Vec<Problem> = form.problems(has_cover, file_name).into_iter().filter(|p| checks.contains(p)).collect();
            ui.add_space(6.0);
            if left.is_empty() {
                ui.label(RichText::new("Nothing missing. Ctrl+Enter saves and goes on (or d: done as it is).").color(P.accent));
            } else {
                let names: Vec<&str> = left.iter().map(|p| p.name()).collect();
                ui.label(RichText::new(format!("Missing: {}", names.join(", "))).font(font(15.0, semibold())).color(P.danger));
            }
            if let Some(notice) = &self.notice {
                ui.label(RichText::new(notice).color(P.muted));
            }
            ui.label(RichText::new("Ctrl+Enter save and next · Ctrl+↓/↑ next/previous · Esc leaves a field; then j/k, Space play, u skip, d done").color(P.faint).small());
            ui.add_space(8.0);

            // The cursor in the first field to fix (once, when the track is shown).
            if self.focus_first {
                self.focus_first = false;
                if let Some(field) = left.iter().find_map(|p| TrackForm::field_for(*p)) {
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new(field)));
                }
            }

            form.show(ui, shell, Some(catalog), &d, track.cover());

            // A Ctrl+Enter save finished: on to the next when nothing's left.
            if form.take_saved() && self.advance_after_save == Some(id) {
                self.advance_after_save = None;
                let left: Vec<Problem> = form.problems(has_cover, file_name).into_iter().filter(|p| checks.contains(p)).collect();
                if left.is_empty() {
                    shell.track_forms.insert(id, form);
                    self.mark_done(shell, catalog, "fixed", true);
                    // Counted even when it had no flag (the "every track" list).
                    if !flags.iter().any(|f| f.kind == "fix_later") {
                        self.done += 1;
                        self.step(shell, catalog, 1);
                    }
                    return;
                }
                let names: Vec<&str> = left.iter().map(|p| p.name()).collect();
                self.notice = Some(format!("Saved. Still missing: {}.", names.join(", ")));
                self.focus_first = true;
            }

            // The file's own tags, to fix from: a click puts one in its field.
            let tags = d.file.embedded_tags.clone();
            if let Some(tags) = tags.as_object().filter(|t| !t.is_empty()) {
                ui.add_space(12.0);
                ui.label(RichText::new("Tags in the file").font(font(14.0, bold())));
                ui.label(RichText::new("Click one to use it: a title, an artist or an album goes in its field.").color(P.faint).small());
                egui::Grid::new(("review-tags", id)).num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    for (name, value) in tags {
                        let text = match value {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "),
                            other => other.to_string(),
                        };
                        ui.label(RichText::new(name).color(P.faint));
                        let field = tag_field(name);
                        let label = egui::Label::new(RichText::new(&text).color(if field.is_some() { P.text } else { P.muted })).sense(Sense::click()).truncate();
                        let response = ui.add(label);
                        if let Some(field) = field
                            && response.on_hover_text(format!("Use as the {field}")).clicked()
                        {
                            match field {
                                "title" => form.meta.title = text.clone(),
                                "release date" => form.meta.release_date = text.clone(),
                                "artist" => form.suggest_artist(&text, shell),
                                "album" => form.suggest_album(&text, shell),
                                _ => {}
                            }
                        }
                        ui.end_row();
                    }
                });
            }
            shell.track_forms.insert(id, form);
        });
    }
}

/// The field a tag fills, by its name (as the file has it, in any case).
fn tag_field(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "title" | "tit2" => Some("title"),
        "artist" | "tpe1" | "albumartist" | "album_artist" | "tpe2" => Some("artist"),
        "album" | "talb" => Some("album"),
        "date" | "year" | "tdrc" | "tyer" | "originaldate" => Some("release date"),
        _ => None,
    }
}

/// A flag's other tracks or artists, as links.
fn related(ui: &mut egui::Ui, shell: &mut Shell, flag: &Flag) {
    let tracks = flag.related_tracks.as_array().cloned().unwrap_or_default();
    let artists = flag.related_artists.as_array().cloned().unwrap_or_default();
    if tracks.is_empty() && artists.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for t in &tracks {
            let (Some(id), Some(title)) = (t.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok()), t.get("title").and_then(|v| v.as_str())) else {
                continue;
            };
            let artist = t.get("artist_display").and_then(|v| v.as_str()).unwrap_or("");
            if widgets::link(ui, &format!("{title} · {artist}"), font(13.0, medium()), P.text) {
                shell.act(Action::Go(crate::ui::Page::Track(id)));
            }
        }
        for a in &artists {
            let (Some(id), Some(name)) = (a.get("id").and_then(|v| v.as_str()).and_then(|v| v.parse::<Uuid>().ok()), a.get("name").and_then(|v| v.as_str())) else {
                continue;
            };
            let dis = a.get("disambiguation").and_then(|v| v.as_str()).filter(|d| !d.is_empty()).map(|d| format!(" ({d})")).unwrap_or_default();
            if widgets::link(ui, &format!("{name}{dis}"), font(13.0, medium()), P.text) {
                shell.act(Action::Go(crate::ui::Page::Artist(id)));
            }
        }
    });
}

/// Leaves the text field (moving to another track: the cursor goes to that one's first
/// field instead).
fn ctx_stop_text(ctx: &egui::Context) {
    ctx.memory_mut(|m| m.stop_text_input());
}

/// Problems as bits (1 << their place in `Problem::ALL`).
fn bits(problems: &[Problem]) -> u8 {
    problems.iter().fold(0, |b, p| b | 1 << Problem::ALL.iter().position(|q| q == p).unwrap_or(0))
}

fn unbits(bits: u8) -> Vec<Problem> {
    Problem::ALL.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, p)| *p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_as_bits_and_back() {
        let some = [Problem::NoTitle, Problem::NoCover];
        assert_eq!(unbits(bits(&some)), some);
        assert_eq!(unbits(0), []);
        assert_eq!(unbits(bits(&Problem::ALL)), Problem::ALL);
    }
}
