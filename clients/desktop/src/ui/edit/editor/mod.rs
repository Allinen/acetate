//! The track editor: one or many tracks changed by typing what to change, the way a filter
//! is typed: `artist:Ann role:featured`, `album:Some EP #1+`, `status:demo`, `date:2020`
//! (see `parse` for the line, `resolve` for what it means). The one editor for tracks: the
//! track page in edit mode, Review, and the tables' Ctrl+E (one track or the marked ones).
//!
//! * **The line**: Enter turns it into staged changes (chips), all of it or, if a part
//!   can't be (no artist like that), none of it, saying why. While a part is typed, its
//!   choices are listed under the line (artists, albums, tracks, roles, statuses…):
//!   ↑/↓ choose, **Tab** takes the choice and writes the next part to type (after an
//!   artist its `role:`, then `as:`, then its place `#`), Enter takes it and stages the
//!   line. Plain words find what can be changed by name. Backspace on an empty line takes
//!   the last chip back to edit; `/` comes back to the line.
//! * **State**: with one track, its fields as rows, each with the flags about it (now, and
//!   after, from the server's preview). ↓ from an empty line goes into the rows (↑/↓ or
//!   j/k, Esc back): Enter edits a row through the line (`title:` and its value, ready to
//!   change); on an artist `r` its role, `a` the name it's credited under, Alt+↑/↓ its
//!   place, Delete takes it off (and an album, a related track, the date). With many
//!   tracks: each field as they have it (how many differ), and as they will.
//! * **Preview**: each track's changes in the history's words, the albums that change
//!   along (`along:off` to leave them), and its review flags after (fixed, still there,
//!   new), from the server's own checks. Ctrl+Enter applies what it shows; with many
//!   tracks only once it's up to date, with one as soon as it is.

mod engine;
pub mod model;
mod panel;
mod parse;
mod resolve;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};
use uuid::Uuid;

use engine::{Change, Engine, Outcome, Phase, PlanState};
use model::Op;
use panel::{Flag, Row, RowKind};
use parse::K;
use resolve::{Candidate, Ctx, Names, Pick, PinKind, Unit};
use crate::account::Vocab;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, Action, Shell};

pub use panel::line_for;
pub use parse::quoted;

/// A staged change: what it was typed as, what it does.
#[derive(Debug, Clone)]
struct Staged {
    text: String,
    label: String,
    ops: Vec<Op>,
    warning: Option<String>,
    /// Its text can be typed back (Backspace puts it in the line). Not for changes made
    /// for you whose names alone wouldn't say which artist (ambiguous names).
    editable: bool,
}

/// What resolving a line needs, apart from the editor itself.
struct Here<'a> {
    catalog: &'a Catalog,
    vocab: &'a Vocab,
    targets: usize,
    album: Option<Uuid>,
    staged_album: Option<Uuid>,
    may_relate: bool,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

pub struct TrackEditor {
    /// Its own (its line's id: several editors may exist, one per track being edited).
    id: u64,
    line: String,
    /// The chosen entry of the list under the line.
    highlighted: usize,
    staged: Vec<Staged>,
    names: Names,
    lookup: Lookup,
    engine: Engine,
    /// Put the cursor in the line (until it's there).
    focus_line: bool,
    /// Esc was pressed with changes staged: again discards them.
    confirm_discard: bool,
    notice: Option<String>,
    /// The line resolved: (what for, the units).
    resolved: Option<(u64, Vec<Unit>)>,
    /// The version edited (one track; None: the shown one).
    variant: Option<Uuid>,
    /// The row the keys are on (one track); None: the line.
    row: Option<usize>,
    /// A one-track edit was just applied: its flags after (taken by `take_saved`).
    saved: Option<Vec<String>>,
    /// Ctrl+Enter with one track before the preview was there: apply once it is.
    apply_when_ready: bool,
    /// The line was filled in for you: the cursor goes after it.
    place_cursor: bool,
}

impl Default for TrackEditor {
    fn default() -> Self {
        TrackEditor {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            line: String::new(),
            highlighted: 0,
            staged: Vec::new(),
            names: Names::default(),
            lookup: Lookup::default(),
            engine: Engine::default(),
            focus_line: false,
            confirm_discard: false,
            notice: None,
            resolved: None,
            variant: None,
            row: None,
            saved: None,
            apply_when_ready: false,
            place_cursor: false,
        }
    }
}

impl TrackEditor {
    fn line_id(&self) -> egui::Id {
        egui::Id::new(("track-editor-line", self.id))
    }

    /// The cursor goes to the line.
    pub fn opened(&mut self) {
        self.focus_line = true;
        self.row = None;
    }

    /// Applying, or applied and its outcome not closed yet.
    pub fn applying_or_applied(&self) -> bool {
        self.engine.applied.lock().unwrap().phase != Phase::Editing
    }

    /// Changes are staged (Esc is the editor's then: it asks before discarding them).
    pub fn has_changes(&self) -> bool {
        !self.staged.is_empty()
    }

    /// The keys are on its rows (a page's own keys wait).
    pub fn takes_keys(&self) -> bool {
        self.row.is_some()
    }

    /// Nothing staged and nothing typed.
    pub fn is_blank(&self) -> bool {
        self.staged.is_empty() && self.line.trim().is_empty()
    }

    /// Types `text` in the line (replacing what's there), with the cursor after it.
    pub fn prefill(&mut self, text: &str) {
        self.line = text.to_string();
        self.highlighted = 0;
        self.row = None;
        self.resolved = None;
        self.focus_line = true;
        self.place_cursor = true;
    }

    /// The version edited (one track), if another than the shown one was chosen.
    pub fn variant(&self) -> Option<Uuid> {
        self.variant
    }

    /// Edits `variant` (one track). Only without staged changes: they're for the version
    /// they were typed for.
    pub fn set_variant(&mut self, variant: Uuid) {
        if self.staged.is_empty() {
            self.variant = Some(variant);
        }
    }

    /// The flags after a one-track edit just applied (once).
    pub fn take_saved(&mut self) -> Option<Vec<String>> {
        self.saved.take()
    }

    /// One track's title as it is (loaded by the editor), once it's there.
    pub fn title(&self, track: Uuid) -> Option<String> {
        self.engine.ready(track).map(|r| r.base.meta.title.clone())
    }

    /// One track's credits as the staged changes leave them.
    pub fn credits_after(&self, track: Uuid) -> Option<Vec<model::Credit>> {
        self.engine.ready(track).map(|r| r.meta.credits.clone())
    }

    /// Stages crediting `to` wherever `from` is credited now (same role, same place): an
    /// ambiguous name settled on another of its artists.
    pub fn replace_artist(&mut self, catalog: &Catalog, track: Uuid, from: Uuid, to: Uuid) {
        let Some(credits) = self.credits_after(track) else { return };
        let mut ops = Vec::new();
        for (at, c) in credits.iter().enumerate().filter(|(_, c)| c.artist_id == from) {
            ops.push(Op::Uncredit { artist: from, role: Some(c.role.clone()) });
            ops.push(Op::Credit { artist: to, role: Some(c.role.clone()), alias: Some(None), at: Some(at) });
        }
        if ops.is_empty() {
            return;
        }
        let label = |id: Uuid| catalog.artist_label(id).unwrap_or_else(|| "(an artist)".into());
        let text = format!("artist: {} → {}", label(from), label(to));
        self.staged.push(Staged { label: text.clone(), text, ops, warning: None, editable: false });
    }

    fn ops(&self) -> Vec<Op> {
        self.staged.iter().flat_map(|s| s.ops.clone()).collect()
    }

    /// The editor for `targets` (in the order shown); `album`: the list is this album's.
    /// With one track it's laid out top to bottom (the page around it scrolls); with many,
    /// in two columns filling the room given. Returns true when it's done and asks to close.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, targets: &[Uuid], album: Option<Uuid>) -> bool {
        let ctx = ui.ctx().clone();
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let single = targets.len() == 1;
        let variant = if single { self.variant } else { None };
        let change = Change::of(self.ops(), &self.names);
        self.engine.update(shell, catalog, targets, &change, &self.names, variant);
        self.finished(shell, targets);
        let phase = self.engine.applied.lock().unwrap().phase;
        let line_id = self.line_id();
        if !self.staged.is_empty() {
            shell.form_dirty = true;
        }

        // Ctrl+Enter with one track came before its preview: applied once it's there.
        if self.apply_when_ready && phase == Phase::Editing && self.engine.preview_current() && !self.loading() {
            self.apply_when_ready = false;
            if self.refused() > 0 {
                self.notice = Some("Not applied: the preview says it can't be (in red).".into());
            } else if self.changing() > 0 {
                self.engine.apply(shell, &change, variant);
            }
        }

        // The cursor to the line: asked until it holds (a click that opened a popup ends
        // outside the field, which lets go of it that same frame).
        if self.place_cursor {
            self.place_cursor = false;
            self.cursor_to_end(&ctx);
        }
        if self.focus_line {
            if ctx.memory(|m| m.has_focus(line_id)) && !ctx.input(|i| i.pointer.any_released()) {
                self.focus_line = false;
            } else {
                ctx.memory_mut(|m| m.request_focus(line_id));
            }
        }
        let focused = ctx.memory(|m| m.has_focus(line_id));
        if focused {
            self.row = None;
        }
        let typing = ctx.egui_wants_keyboard_input();
        // `/` outside a field: back to the line.
        if !typing && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "/"))) {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/")));
            self.focus_line = true;
            self.row = None;
        }

        // ---- What the line means, and its list ----
        let staged_album = change.ops.iter().rev().find_map(|op| if let Op::Album { album, .. } = op { Some(*album) } else { None });
        let may_relate = shell.account.can("edit_relations");
        let here = Here { catalog, vocab: &vocab, targets: targets.len(), album, staged_album, may_relate };
        let parsed = parse::parse(&self.line);
        let last = parsed.parts.last().cloned();
        let owner = last.as_ref().and_then(|l| {
            matches!(l.key, K::Role | K::As | K::Kind).then(|| parsed.parts.iter().rev().find(|p| !matches!(p.key, K::Role | K::As | K::Kind)).cloned()).flatten()
        });
        // Tab or Enter takes what's listed: the matches must be for exactly what's typed
        // (found in the background, they can still be the last keystroke's), so then
        // they're found now.
        let taking = focused && ctx.input(|i| i.key_pressed(Key::Tab) || i.key_pressed(Key::Enter));
        let hits = match last.as_ref().and_then(|l| resolve::search_for(l).map(|sp| (sp, l.value.clone()))) {
            Some((sp, value)) if taking => self.lookup.now(catalog, sp, &value),
            Some((sp, value)) => self.lookup.hits(shell, sp, &value),
            None => Vec::new(),
        };
        let words = parsed.parts.is_empty() && !parsed.words.is_empty();
        let (candidates, actions, units) = {
            let mut aliases = |id: Uuid| shell.artist_details.get(id).value.as_ref().map(|a| a.aliases.clone());
            let rctx = here.ctx(&self.names);
            let candidates: Vec<Candidate> = match &last {
                Some(l) if !words => resolve::candidates(l, owner.as_ref(), &rctx, &hits, &mut aliases),
                _ => Vec::new(),
            };
            let actions = if words { resolve::actions(&parsed.words.join(" "), &rctx) } else { Vec::new() };
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (&self.line, self.names.version, staged_album, shell.generation, targets.len()).hash(&mut h);
                h.finish()
            };
            let units = match &self.resolved {
                Some((k, units)) if *k == key => units.clone(),
                _ => {
                    let (units, waiting) = resolve::resolve(&self.line, &rctx, &mut aliases);
                    self.resolved = if waiting { None } else { Some((key, units.clone())) };
                    units
                }
            };
            (candidates, actions, units)
        };
        let choices = if words { actions.len() } else { candidates.len() };
        self.highlighted = self.highlighted.min(choices.saturating_sub(1));

        // One track's rows (and the flags on them).
        let rows: Vec<Row> = match (single, targets.first().and_then(|t| self.engine.ready(*t))) {
            (true, Some(r)) => {
                let track = targets[0];
                let done = &shell.flags.done;
                let now: Vec<String> = catalog.issues.get(&track).map(|f| f.iter().filter(|(id, _)| !done.contains(id)).map(|(_, k)| k.clone()).collect()).unwrap_or_default();
                let after = self.after_flags(track);
                // The names credits are under: the track's own, and those of artists chosen here.
                let mut aliases: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
                if let Some(d) = self.engine.detail(track) {
                    aliases.extend(d.credits.iter().filter_map(|c| Some((c.alias_id?, c.alias.clone()?))));
                    aliases.extend(d.alias_names.iter().map(|(k, v)| (*k, v.clone())));
                }
                let unnamed: Vec<Uuid> = r.meta.credits.iter().filter(|c| c.alias_id.is_some_and(|a| !aliases.contains_key(&a))).map(|c| c.artist_id).collect();
                for artist in unnamed {
                    if let Some(a) = shell.artist_details.get(artist).value.as_ref() {
                        aliases.extend(a.aliases.iter().map(|x| (x.id, x.name.clone())));
                    }
                }
                let ambiguous: Vec<(Uuid, Vec<Uuid>)> = shell
                    .flags
                    .list
                    .iter()
                    .filter(|f| f.track_id == track && f.kind == "ambiguous_artist" && !done.contains(&f.id))
                    .map(|f| (f.id, f.candidates().into_iter().map(|c| c.0).collect()))
                    .collect();
                let names = self.names.stand_ins();
                let know = panel::Know { catalog, names: &names, aliases: &aliases, ambiguous: &ambiguous };
                panel::rows(r, &know, &now, after.as_deref())
            }
            _ => Vec::new(),
        };
        if let Some(row) = self.row {
            self.row = (!rows.is_empty()).then(|| row.min(rows.len() - 1));
        }
        // On the rows, the keys are theirs (Esc too: back to the line, not off the page).
        if self.row.is_some() {
            shell.page_takes_keys = true;
            shell.page_takes_back = true;
        }

        // ---- Keys in the line ----
        let mut take: Option<(usize, bool)> = None;
        let mut stage = false;
        if focused {
            let (up, down, tab, enter, back, escape) = ctx.input_mut(|i| {
                (
                    choices > 0 && i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    (choices > 0 || (self.line.is_empty() && !rows.is_empty())) && i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::Tab),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    self.line.is_empty() && !self.staged.is_empty() && i.consume_key(Modifiers::NONE, Key::Backspace),
                    !self.line.is_empty() && i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if up {
                self.highlighted = self.highlighted.saturating_sub(1);
            }
            if down {
                if choices > 0 {
                    self.highlighted = (self.highlighted + 1).min(choices - 1);
                } else {
                    // Into the rows.
                    self.row = Some(0);
                    ctx.memory_mut(|m| m.surrender_focus(line_id));
                }
            }
            if tab && choices > 0 {
                take = Some((self.highlighted, true));
            }
            if enter {
                if choices > 0 {
                    take = Some((self.highlighted, false));
                }
                stage = !words;
            }
            if back
                && let Some(last) = self.staged.pop()
            {
                self.apply_when_ready = false;
                if last.editable {
                    self.line = last.text;
                    self.cursor_to_end(&ctx);
                }
            }
            if escape {
                self.line.clear();
            }
        }
        // ---- Keys on a row ----
        if let Some(n) = self.row.filter(|_| !typing && !shell.palette_open && !shell.modal_was_open)
            && let Some(row) = rows.get(n)
            && let Some(r) = targets.first().and_then(|t| self.engine.ready(*t)).cloned()
        {
            let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
            // Alt first: a key asked for without Alt matches with it too.
            let earlier = pressed(Key::ArrowUp, Modifiers::ALT) || pressed(Key::K, Modifiers::ALT);
            let later = !earlier && (pressed(Key::ArrowDown, Modifiers::ALT) || pressed(Key::J, Modifiers::ALT));
            if earlier || later {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                self.row_key(row.kind, if earlier { RowKey::Earlier } else { RowKey::Later }, &r, shell, &here);
                // The cursor goes with the artist moved (credits are rows next to each other).
                if let RowKind::Credit(at) = row.kind {
                    if earlier && at > 0 {
                        self.row = Some(n - 1);
                    } else if later && at + 1 < r.meta.credits.len() {
                        self.row = Some(n + 1);
                    }
                }
            } else if pressed(Key::ArrowDown, Modifiers::NONE) || pressed(Key::J, Modifiers::NONE) {
                self.row = Some((n + 1).min(rows.len() - 1));
            } else if pressed(Key::ArrowUp, Modifiers::NONE) || pressed(Key::K, Modifiers::NONE) {
                if n == 0 {
                    self.row = None;
                    self.focus_line = true;
                } else {
                    self.row = Some(n - 1);
                }
            } else if pressed(Key::Escape, Modifiers::NONE) {
                self.row = None;
                self.focus_line = true;
            } else {
                let action = if pressed(Key::Enter, Modifiers::NONE) {
                    Some(RowKey::Edit)
                } else if pressed(Key::R, Modifiers::NONE) {
                    Some(RowKey::Role)
                } else if pressed(Key::A, Modifiers::NONE) {
                    Some(RowKey::Alias)
                } else if pressed(Key::Delete, Modifiers::NONE) {
                    Some(RowKey::Remove)
                } else if pressed(Key::C, Modifiers::NONE) {
                    Some(RowKey::Next)
                } else {
                    None
                };
                if let Some(action) = action {
                    // The letter typed is still in this frame's input: not into the line.
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                    if action == RowKey::Next {
                        // An ambiguous name: its next candidate credited instead.
                        if let (Some((_, candidates)), Some(from)) = (&row.choose, row.artist)
                            && let Some(at) = candidates.iter().position(|c| *c == from)
                        {
                            let to = candidates[(at + 1) % candidates.len()];
                            self.replace_artist(catalog, targets[0], from, to);
                        }
                    } else {
                        self.row_key(row.kind, action, &r, shell, &here);
                    }
                }
            }
        }
        // Ctrl+Enter: what's typed is staged first; then the preview is applied.
        let ctrl_enter = !shell.palette_open && !shell.modal_was_open && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
        if ctrl_enter && !self.line.trim().is_empty() {
            stage = true;
        }
        // Esc outside the line, with changes staged: asks, and again discards them.
        if !typing && self.row.is_none() && !self.staged.is_empty() && phase == Phase::Editing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.confirm_discard {
                self.staged.clear();
                self.confirm_discard = false;
            } else {
                self.confirm_discard = true;
            }
        }

        // ---- Header ----
        if !single {
            ui.horizontal(|ui| {
                let loaded = self.engine.loaded_count(targets);
                let mut parts = vec![format!("{} tracks", thousands(targets.len()))];
                if loaded < targets.len() {
                    parts.push(format!("loading {} of {}", thousands(loaded), thousands(targets.len())));
                } else if !self.staged.is_empty() {
                    parts.push(format!("{} change", thousands(self.changing())));
                }
                ui.label(RichText::new(parts.join(" · ")).color(P.faint).small());
            });
        }

        // ---- The line ----
        let editing = phase == Phase::Editing;
        let hint = "What to change: artist:…  album:…  status:demo  date:2020  genre:…  (Tab: the next part · Enter: stage it)";
        let response = ui.add_enabled(editing, egui::TextEdit::singleline(&mut self.line).id(line_id).hint_text(hint).desired_width(f32::INFINITY));
        if response.changed() {
            self.highlighted = 0;
            self.notice = None;
            self.confirm_discard = false;
        }

        // A choice taken (Tab, Enter or a click).
        let mut clicked = None;
        if words && !actions.is_empty() {
            list(ui, actions.iter().map(|(label, insert)| (label.clone(), insert.clone())).collect(), self.highlighted, &mut clicked);
        } else if !candidates.is_empty() {
            list(ui, candidates.iter().map(|c| (c.label.clone(), c.sub.clone())).collect(), self.highlighted, &mut clicked);
        }
        if let Some(n) = clicked {
            take = Some((n, true));
        }
        if let Some((n, next)) = take {
            if words {
                if let Some((_, insert)) = actions.get(n) {
                    self.line = insert.clone();
                    self.cursor_to_end(&ctx);
                    stage = false;
                }
            } else if let (Some(c), Some(l)) = (candidates.get(n), &last) {
                self.take(c, l, next, &ctx);
                if next {
                    stage = false;
                }
            }
            self.highlighted = 0;
            self.focus_line = true;
        }
        if stage {
            let line = std::mem::take(&mut self.line);
            if !self.stage(&line, shell, &here) {
                self.line = line;
            }
        }

        // What Enter would do (or why it can't), when no list is in the way.
        if candidates.is_empty() && actions.is_empty() && !self.line.trim().is_empty() {
            egui::Frame::NONE.fill(P.bg).corner_radius(8).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                for u in &units {
                    match &u.result {
                        Ok(r) => {
                            ui.label(RichText::new(&r.label).color(P.accent));
                            if let Some(w) = &r.warning {
                                ui.label(RichText::new(format!("  ⚠ {w}")).color(P.text).small());
                            }
                        }
                        Err(e) => {
                            ui.label(RichText::new(e).color(P.danger));
                        }
                    }
                }
                if units.is_empty() {
                    ui.label(RichText::new("Nothing to change by that name").color(P.faint).small());
                }
            });
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted).small());
        }

        // ---- The staged changes ----
        if self.staged.is_empty() {
            if self.line.is_empty() {
                let more = if single { " · ↓ goes to the fields below" } else { "" };
                ui.label(
                    RichText::new(format!("key:value sets it (several at once too) · -key:value takes one off, -key clears · a word finds what to change · Ctrl+Enter applies{more}"))
                        .color(P.faint)
                        .small(),
                );
            }
        } else {
            ui.add_space(4.0);
            let mut remove = None;
            ui.horizontal_wrapped(|ui| {
                for (n, s) in self.staged.iter().enumerate() {
                    let color = if s.warning.is_some() { P.text } else { P.accent };
                    egui::Frame::NONE.fill(P.surface_hi).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let tip = match &s.warning {
                                Some(w) => format!("{}\n⚠ {w}", s.label),
                                None => s.label.clone(),
                            };
                            ui.label(RichText::new(&s.text).font(font(13.0, medium())).color(color)).on_hover_text(tip);
                            if editing && ui.small_button("×").on_hover_text("Take it back").clicked() {
                                remove = Some(n);
                            }
                        });
                    });
                }
            });
            if let Some(n) = remove {
                self.staged.remove(n);
                self.apply_when_ready = false;
            }
            if self.confirm_discard {
                ui.label(RichText::new("Discard these changes? Esc again to discard them.").color(P.danger).small());
            }
        }
        ui.add_space(8.0);

        // ---- State and preview ----
        let mut closed = false;
        if single {
            self.rows_view(ui, shell, catalog, targets[0], &rows, &vocab);
            ui.add_space(10.0);
            closed = self.preview(ui, shell, catalog, &change, ctrl_enter, variant, true);
        } else {
            let (height, full) = (ui.available_height(), ui.available_width());
            let left = (full * 0.38).floor();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(egui::vec2(left, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_height(height);
                    egui::ScrollArea::vertical().id_salt(("track-editor-state", self.id)).auto_shrink([false, false]).show(ui, |ui| {
                        state(ui, &self.engine, catalog, &self.names);
                    });
                });
                ui.add_space(16.0);
                ui.allocate_ui_with_layout(egui::vec2(full - left - 16.0, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_height(height);
                    closed = self.preview(ui, shell, catalog, &change, ctrl_enter, variant, false);
                });
            });
        }
        closed
    }

    fn loading(&self) -> bool {
        self.engine.plans.iter().any(|p| matches!(p.state, PlanState::Loading))
    }

    /// Tracks the staged changes change.
    fn changing(&self) -> usize {
        self.engine.plans.iter().filter(|p| matches!(&p.state, PlanState::Ready(r) if r.changes())).count()
    }

    /// Tracks the preview says can't be written.
    fn refused(&self) -> usize {
        self.engine.preview.lock().unwrap().answers.values().filter(|a| a.error.is_some()).count()
    }

    /// A track's flags after the edit, as the preview (or the edit just applied) says.
    fn after_flags(&self, track: Uuid) -> Option<Vec<String>> {
        let applied = self.engine.applied.lock().unwrap();
        if let Some(a) = applied.answers.get(&track) {
            return Some(a.flags.clone());
        }
        drop(applied);
        let preview = self.engine.preview.lock().unwrap();
        (preview.key == self.engine.plan_key && !preview.running && !self.staged.is_empty())
            .then(|| preview.answers.get(&track).filter(|a| a.error.is_none()).map(|a| a.flags.clone()))
            .flatten()
    }

    /// Applying just finished: the library, the flags and the tracks' pages catch up. One
    /// track written: the changes are done with (its flags kept for `take_saved`).
    fn finished(&mut self, shell: &mut Shell, targets: &[Uuid]) {
        let (seen, outcomes, answers) = {
            let mut a = self.engine.applied.lock().unwrap();
            (std::mem::take(&mut a.finished_unseen), a.outcomes.clone(), a.answers.clone())
        };
        if !seen {
            return;
        }
        shell.act(Action::RefreshLibrary);
        shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
        if targets.len() <= 100 {
            for t in targets {
                shell.track_details.refresh(*t);
            }
        }
        let all_done = !outcomes.is_empty() && outcomes.values().all(|o| matches!(o, Outcome::Done));
        if targets.len() == 1 && all_done {
            self.saved = Some(answers.get(&targets[0]).map(|a| a.flags.clone()).unwrap_or_default());
            self.staged.clear();
            self.names = Names::default();
            self.engine.forget();
            self.notice = Some("Saved.".into());
        }
    }

    /// Puts the cursor at the end of the line (after the line was changed for you).
    fn cursor_to_end(&mut self, ctx: &egui::Context) {
        let id = self.line_id();
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        let end = egui::text::CCursor::new(self.line.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, id);
        self.focus_line = true;
    }

    /// A choice from the list: its text in the line for the value being typed, and what it
    /// stands for from now on; `next`: the next part to type after it.
    fn take(&mut self, c: &Candidate, part: &parse::Part, next: bool, ctx: &egui::Context) {
        let text = parse::quoted(&c.text);
        let end = part.end.min(self.line.len());
        let start = part.value_start.min(end);
        self.line.replace_range(start..end, &text);
        match &c.pick {
            Some(Pick::Pin(kind, id)) => self.names.pin(*kind, &c.text, *id),
            Some(Pick::NewArtist) => {
                let id = self.names.new_artist(&c.text);
                self.names.pin(PinKind::Artist, &c.text, id);
            }
            Some(Pick::NewAlbum) => {
                let id = self.names.new_album(&c.text);
                self.names.pin(PinKind::Album, &c.text, id);
            }
            None => {}
        }
        if next && let Some(after) = c.next {
            self.line.push(' ');
            self.line.push_str(after);
        }
        self.resolved = None;
        self.cursor_to_end(ctx);
    }

    /// `text` staged: all of it, or (something in it can't be) none of it, saying why.
    fn stage(&mut self, text: &str, shell: &mut Shell, here: &Here) -> bool {
        if text.trim().is_empty() {
            return true;
        }
        let (units, _) = {
            let mut aliases = |id: Uuid| shell.artist_details.get(id).value.as_ref().map(|a| a.aliases.clone());
            resolve::resolve(text, &here.ctx(&self.names), &mut aliases)
        };
        if units.is_empty() {
            self.notice = Some("Nothing to change there: key:value (artist:…, status:…), or a word to find what to change.".into());
            return false;
        }
        if let Some(err) = units.iter().find_map(|u| u.result.as_ref().err()) {
            self.notice = Some(format!("Not staged: {err}"));
            return false;
        }
        for u in units {
            let Ok(r) = u.result else { continue };
            if let Some((id, kind)) = &r.album_kind {
                self.names.set_album_kind(*id, kind);
            }
            self.staged.push(Staged { text: u.text, label: r.label, ops: r.ops, warning: r.warning, editable: true });
        }
        // A Ctrl+Enter waiting was for what was staged before.
        self.apply_when_ready = false;
        self.resolved = None;
        self.notice = None;
        self.confirm_discard = false;
        true
    }

    /// A key on one track's row: an edit through the line (filled in, the cursor there), or
    /// a change staged at once (taking something off, moving an artist).
    fn row_key(&mut self, kind: RowKind, key: RowKey, r: &engine::Ready, shell: &mut Shell, here: &Here) {
        let q = parse::quoted;
        let catalog = here.catalog;
        // A name in the line that stands for exactly this one.
        let artist = |names: &mut Names, id: Uuid| -> String {
            let name = catalog.artist(id).map(|a| a.name.clone()).or_else(|| names.stand_ins().get(&id).cloned()).unwrap_or_default();
            names.pin(PinKind::Artist, &name, id);
            q(&name)
        };
        let album = |names: &mut Names, id: Uuid| -> String {
            let title = catalog.album(id).map(|a| a.title.clone()).or_else(|| names.stand_ins().get(&id).cloned()).unwrap_or_default();
            names.pin(PinKind::Album, &title, id);
            q(&title)
        };
        let track = |names: &mut Names, id: Uuid| -> String {
            let title = catalog.track_index(id).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_default();
            names.pin(PinKind::Track, &title, id);
            q(&title)
        };
        let fill = |this: &mut Self, text: String| this.prefill(&text);
        match (kind, key) {
            (RowKind::Title, RowKey::Edit) => fill(self, format!("title:{}", q(&r.meta.title))),
            (RowKind::Date, RowKey::Edit) => fill(self, format!("date:{}", r.meta.release_date.clone().unwrap_or_default())),
            (RowKind::Date, RowKey::Remove) => {
                self.stage("date:none", shell, here);
            }
            (RowKind::Comment, RowKey::Edit) => fill(self, format!("comment:{}", q(r.meta.comment.as_deref().unwrap_or_default()))),
            (RowKind::Comment, RowKey::Remove) => {
                self.stage("comment:none", shell, here);
            }
            (RowKind::Status, RowKey::Edit) => fill(self, "status:".into()),
            (RowKind::Genres, RowKey::Edit) => fill(self, "genre:".into()),
            (RowKind::Genres, RowKey::Remove) => {
                self.stage("-genre", shell, here);
            }
            (RowKind::AddArtist, RowKey::Edit) => fill(self, "artist:".into()),
            (RowKind::AddAlbum, RowKey::Edit) => fill(self, "album:".into()),
            (RowKind::AddRelated, RowKey::Edit) => fill(self, "related:".into()),
            (RowKind::Credit(n), key) => {
                let Some(c) = r.meta.credits.get(n).cloned() else { return };
                let name = artist(&mut self.names, c.artist_id);
                match key {
                    // Another role: off in this one, on in the one chosen, same place.
                    RowKey::Edit | RowKey::Role => fill(self, format!("-artist:{name} role:{} artist:{name} #{} role:", c.role, n + 1)),
                    RowKey::Alias => fill(self, format!("artist:{name} role:{} as:", c.role)),
                    RowKey::Remove => {
                        self.stage(&format!("-artist:{name} role:{}", c.role), shell, here);
                    }
                    RowKey::Earlier if n > 0 => {
                        self.stage(&format!("artist:{name} role:{} #{n}", c.role), shell, here);
                    }
                    RowKey::Later if n + 1 < r.meta.credits.len() => {
                        self.stage(&format!("artist:{name} role:{} #{}", c.role, n + 2), shell, here);
                    }
                    _ => {}
                }
            }
            (RowKind::Album(n), key) => {
                let Some(m) = r.facts.albums.get(n).cloned() else { return };
                let title = album(&mut self.names, m.album_id);
                match key {
                    RowKey::Edit => {
                        let numbers = match (m.disc_number, m.track_number) {
                            (Some(d), Some(t)) => format!(" #{d}-{t}"),
                            (_, Some(t)) => format!(" #{t}"),
                            _ => " #".into(),
                        };
                        fill(self, format!("album:{title}{numbers}"));
                    }
                    RowKey::Remove => {
                        self.stage(&format!("-album:{title}"), shell, here);
                    }
                    _ => {}
                }
            }
            (RowKind::Related(n), key) => {
                let Some(rel) = r.facts.relations.get(n).cloned() else { return };
                let title = track(&mut self.names, rel.track_id);
                match key {
                    RowKey::Edit => fill(self, format!("-related:{title} related:{title} kind:")),
                    RowKey::Remove => {
                        self.stage(&format!("-related:{title}"), shell, here);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// One track's rows: each field (the one the keys are on lit), its value as it will be
    /// (what it was, struck through), and the flags about it.
    fn rows_view(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, track: Uuid, rows: &[Row], vocab: &Vocab) {
        if rows.is_empty() {
            let text = match self.engine.plans.first().map(|p| &p.state) {
                Some(PlanState::Failed(err)) => err.clone(),
                _ => "Loading the track…".into(),
            };
            ui.label(RichText::new(text).color(P.faint));
            return;
        }
        let mut clicked = None;
        // An ambiguous name's candidate clicked: (the flag, the one credited, the one clicked).
        let mut chosen: Option<(Uuid, Uuid, Uuid)> = None;
        // The value gets the room the key and the flags leave (a grid's cells are only as
        // wide as they're told: text in them wraps after a word or two otherwise).
        let value_width = (ui.available_width() - 90.0 - 220.0).max(240.0);
        egui::Grid::new(("track-editor-rows", self.id)).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            for (n, row) in rows.iter().enumerate() {
                let lit = self.row == Some(n);
                let key = RichText::new(format!("{}:", row.key)).color(if lit { P.accent } else { P.faint });
                if ui.add(egui::Label::new(key).sense(egui::Sense::click())).clicked() {
                    clicked = Some(n);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.set_min_width(value_width);
                    ui.set_max_width(value_width);
                    let empty = matches!(row.kind, RowKind::AddArtist | RowKind::AddAlbum | RowKind::AddRelated);
                    let mut value = if row.kind == RowKind::Removed {
                        // Taken off by the staged changes.
                        RichText::new(row.was.clone().unwrap_or_default()).color(P.faint).strikethrough()
                    } else {
                        let text = if row.value.is_empty() { "—".to_string() } else { row.value.clone() };
                        let color = if row.was.is_some() { P.accent } else if empty || row.kind == RowKind::Other { P.faint } else { P.text };
                        RichText::new(text).color(color)
                    };
                    if lit {
                        value = value.background_color(P.surface_hi);
                    }
                    if ui.add(egui::Label::new(value).sense(egui::Sense::click())).clicked() {
                        clicked = Some(n);
                    }
                    if row.kind == RowKind::Removed {
                        ui.label(RichText::new("taken off").color(P.faint).small());
                    } else if let Some(was) = row.was.as_ref().filter(|w| *w != "new") {
                        let was = if was.is_empty() { "—" } else { was.as_str() };
                        ui.label(RichText::new(was).color(P.faint).strikethrough());
                    }
                    // An ambiguous name: which of its artists it is (the credited one lit).
                    if let (Some((flag, candidates)), Some(credited)) = (&row.choose, row.artist) {
                        ui.label(RichText::new("which:").color(P.faint).small());
                        for c in candidates {
                            let label = catalog.artist_label(*c).unwrap_or_else(|| "(an artist)".into());
                            let size = catalog.artist(*c).map(|a| format!(" · {} tracks", a.track_count)).unwrap_or_default();
                            if ui.selectable_label(*c == credited, RichText::new(format!("{label}{size}")).small()).clicked() {
                                chosen = Some((*flag, credited, *c));
                            }
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.set_min_width(200.0);
                    for (kind, state) in &row.flags {
                        let name = vocab.review_kind_name(kind).to_lowercase();
                        let (text, color) = match state {
                            Flag::Open => (format!("⚠ {name}"), P.danger),
                            Flag::Still => (format!("⚠ {name} (still)"), P.danger),
                            Flag::Fixed => (format!("✓ {name}"), P.accent),
                            Flag::New => (format!("⚠ {name} (new)"), P.danger),
                        };
                        ui.label(RichText::new(text).color(color).small());
                    }
                });
                ui.end_row();
            }
        });
        if let Some(n) = clicked {
            self.row = Some(n);
            ui.ctx().memory_mut(|m| m.surrender_focus(self.line_id()));
        }
        match chosen {
            // The one credited, as it is now: it was right.
            Some((flag, credited, c)) if c == credited && !self.has_changes() => {
                shell.flags.resolve(&shell.api, shell.runtime.handle(), &[flag], "the credited artist was right");
            }
            Some((_, credited, c)) if c != credited => self.replace_artist(catalog, track, credited, c),
            _ => {}
        }
        if self.row.is_some() {
            ui.label(
                RichText::new("Enter edits it · on an artist: r role, a credited as, Alt+↑/↓ (or Alt+k/j) its place, c the next of an ambiguous name's artists · Delete takes it off · Esc back to the line")
                    .color(P.faint)
                    .small(),
            );
        } else if rows.iter().any(|r| r.choose.is_some()) {
            ui.label(RichText::new("An ambiguous name: click which artist it is (the one credited, as it is: it was right).").color(P.faint).small());
        }
    }

    /// The preview (and applying it, and what came of it). True: closed. `single`: one
    /// track (the rows say its flags; applying waits for the preview if it must).
    #[allow(clippy::too_many_arguments)]
    fn preview(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, change: &Change, ctrl_enter: bool, variant: Option<Uuid>, single: bool) -> bool {
        let (phase, done, total, outcomes, applied_answers) = {
            let a = self.engine.applied.lock().unwrap();
            (a.phase, a.done, a.total, a.outcomes.clone(), a.answers.clone())
        };
        let (answers, preview_error, running) = {
            let p = self.engine.preview.lock().unwrap();
            (p.answers.clone(), p.error.clone(), p.running)
        };
        let changing = self.changing();
        let unchanged = self.engine.plans.iter().filter(|p| matches!(&p.state, PlanState::Ready(r) if !r.changes())).count();
        let refused = answers.values().filter(|a| a.error.is_some()).count();
        let current = self.engine.preview_current();
        let loading = self.loading();
        let mut closed = false;

        match phase {
            Phase::Editing => {
                let can = current && changing > refused && !loading && self.line.trim().is_empty();
                let text = if single { "Apply  (Ctrl+Enter)".to_string() } else { format!("Apply to {} tracks  (Ctrl+Enter)", thousands(changing - refused.min(changing))) };
                let mut apply = false;
                ui.horizontal(|ui| {
                    if changing > 0 {
                        apply = ui.add_enabled(can, egui::Button::new(text)).clicked();
                    }
                    if changing == 0 {
                        if !self.staged.is_empty() {
                            ui.label(RichText::new("It changes nothing.").color(P.faint));
                        }
                    } else if loading || running || !current {
                        ui.add(egui::Spinner::new());
                        ui.label(RichText::new("Checking it with the server…").color(P.faint));
                    }
                });
                if ctrl_enter {
                    if can {
                        apply = true;
                    } else if single && !self.staged.is_empty() && self.line.trim().is_empty() {
                        // (What was typed may have been staged just now: its preview comes
                        // next.) Applied once the preview's there, if it can be.
                        self.apply_when_ready = true;
                    } else if changing > 0 && (running || !current || loading) {
                        self.notice = Some("The preview isn't up to date yet: Ctrl+Enter again once it is.".into());
                    }
                }
                if apply {
                    self.engine.apply(shell, change, variant);
                }
                // The new things it makes, named.
                let mut new = Vec::new();
                for (id, name) in &self.names.new_artists {
                    if change.ops.iter().any(|op| matches!(op, Op::Credit { artist, .. } if artist == id)) {
                        new.push(format!("artist “{name}”"));
                    }
                }
                for (id, title, kind) in &self.names.new_albums {
                    if change.ops.iter().any(|op| matches!(op, Op::Album { album, .. } if album == id)) {
                        new.push(format!("{kind} “{title}” (credited to the primary artists its tracks share)"));
                    }
                }
                if !new.is_empty() {
                    ui.label(RichText::new(format!("Makes: {}", new.join(", "))).color(P.muted).small());
                }
                if !change.along {
                    ui.label(RichText::new("Albums the tracks are alone on are left as they are (along:off).").color(P.muted).small());
                }
                if let Some(err) = preview_error {
                    ui.label(RichText::new(format!("The server couldn't check it: {err}")).color(P.danger));
                }
            }
            Phase::Applying => {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(format!("Changing… {done} of {total}")));
            }
            Phase::Finished => {
                let failed = outcomes.values().filter(|o| matches!(o, Outcome::Failed(_))).count();
                let ok = outcomes.len() - failed;
                let text = if failed == 0 { format!("Done: {ok} tracks changed.") } else { format!("Done: {ok} changed, {failed} not (below).") };
                ui.label(RichText::new(text).color(if failed == 0 { P.accent } else { P.danger }));
                let (mut close, mut keep) = (false, false);
                ui.horizontal(|ui| {
                    close = ui.button("Close").on_hover_text("Done: the changes are cleared").clicked();
                    keep = ui.button("Keep editing").on_hover_text("The same changes, on the tracks as they are now").clicked();
                });
                if close {
                    self.staged.clear();
                    self.names = Names::default();
                    self.engine.forget();
                    closed = true;
                } else if keep {
                    self.engine.forget();
                }
            }
        }
        ui.add_space(6.0);

        // ---- Each track ----
        let issues_before = |track: Uuid| -> Vec<String> {
            let done = &shell.flags.done;
            catalog.issues.get(&track).map(|f| f.iter().filter(|(id, _)| !done.contains(id)).map(|(_, k)| k.clone()).collect()).unwrap_or_default()
        };
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let each = |ui: &mut egui::Ui| {
            for plan in &self.engine.plans {
                let (mark, color, lines, err): (&str, egui::Color32, Vec<String>, Option<String>) = match &plan.state {
                    PlanState::Loading => continue,
                    PlanState::Failed(err) => ("✗", P.danger, Vec::new(), Some(err.clone())),
                    PlanState::Ready(r) if !r.changes() => continue,
                    PlanState::Ready(r) => {
                        let answer = applied_answers.get(&plan.track).or_else(|| answers.get(&plan.track));
                        let mut lines = r.lines.clone();
                        if let Some(a) = answer {
                            for along in &a.along {
                                lines.extend(engine::along_lines(along, catalog));
                            }
                        }
                        let err = match outcomes.get(&plan.track) {
                            Some(Outcome::Failed(why)) => Some(why.clone()),
                            _ => answer.and_then(|a| a.error.as_ref()).filter(|e| e.code != "stale").map(|e| e.message.clone()),
                        };
                        let mark = match outcomes.get(&plan.track) {
                            Some(Outcome::Done) => "✓",
                            Some(Outcome::Failed(_)) => "✗",
                            None if err.is_some() => "✗",
                            None => "",
                        };
                        let color = if err.is_some() { P.danger } else if mark == "✓" { P.accent } else { P.text };
                        (mark, color, lines, err)
                    }
                };
                if !single {
                    ui.label(RichText::new(format!("{mark} {}", plan.title).trim().to_string()).font(font(13.5, semibold())).color(color));
                }
                let indent = if single { "" } else { "    " };
                for line in &lines {
                    ui.label(RichText::new(format!("{indent}{line}")).color(P.muted));
                }
                if let Some(err) = &err {
                    ui.label(RichText::new(format!("{indent}{err}")).color(P.danger));
                }
                // Made to match, and nothing to change: said so (once the server has).
                let answered = applied_answers.contains_key(&plan.track) || answers.contains_key(&plan.track);
                if change.sync && lines.is_empty() && err.is_none() && answered {
                    ui.label(RichText::new(format!("{indent}its albums match it already (or it's alone on none)")).color(P.faint));
                }
                // Its flags (with one track the rows say them).
                let answer = applied_answers.get(&plan.track).or_else(|| answers.get(&plan.track));
                if let Some(a) = answer.filter(|a| a.error.is_none() && !single) {
                    let before = issues_before(plan.track);
                    let name = |k: &String| vocab.review_kind_name(k).to_lowercase();
                    let fixed: Vec<String> = before.iter().filter(|k| !a.flags.contains(k)).map(name).collect();
                    let still: Vec<String> = a.flags.iter().filter(|k| before.contains(k)).map(name).collect();
                    let new: Vec<String> = a.flags.iter().filter(|k| !before.contains(k)).map(name).collect();
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(24.0);
                        if !fixed.is_empty() {
                            ui.label(RichText::new(format!("fixes: {}", fixed.join(", "))).color(P.accent).small());
                        }
                        if !new.is_empty() {
                            ui.label(RichText::new(format!("new: {}", new.join(", "))).color(P.danger).small());
                        }
                        if !still.is_empty() {
                            ui.label(RichText::new(format!("still: {}", still.join(", "))).color(P.faint).small());
                        }
                    });
                }
                ui.add_space(4.0);
            }
            if unchanged > 0 && !self.staged.is_empty() && !single {
                ui.label(RichText::new(format!("{} unchanged", thousands(unchanged))).color(P.faint));
            }
        };
        if single {
            each(ui);
        } else {
            egui::ScrollArea::vertical().id_salt(("track-editor-preview", self.id)).auto_shrink([false, false]).show(ui, each);
        }
        closed
    }
}

impl Here<'_> {
    fn ctx<'b>(&'b self, names: &'b Names) -> Ctx<'b> {
        Ctx {
            catalog: self.catalog,
            vocab: self.vocab,
            targets: self.targets,
            context_album: self.album,
            staged_album: self.staged_album,
            names,
            may_relate: self.may_relate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowKey {
    Edit,
    Role,
    Alias,
    Remove,
    Earlier,
    Later,
    /// An ambiguous name's next candidate.
    Next,
}

/// A list under the line: (label, what tells it apart), the chosen one lit. A click sets
/// `clicked`.
fn list(ui: &mut egui::Ui, rows: Vec<(String, String)>, highlighted: usize, clicked: &mut Option<usize>) {
    egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (n, (label, sub)) in rows.iter().enumerate() {
            let text = if sub.is_empty() { label.clone() } else { format!("{label}   {sub}") };
            if ui.selectable_label(n == highlighted, RichText::new(text).font(font(13.5, medium()))).clicked() {
                *clicked = Some(n);
            }
        }
        ui.label(RichText::new("↑/↓ choose · Tab: take it and type the next part · Enter: take it and stage the line").color(P.faint).small());
    });
}

/// Each field as the tracks have it, and as they will (with many tracks: how many differ).
fn state(ui: &mut egui::Ui, engine: &Engine, catalog: &Catalog, names: &Names) {
    let ready: Vec<&engine::Ready> = engine.plans.iter().filter_map(|p| if let PlanState::Ready(r) = &p.state { Some(&**r) } else { None }).collect();
    if ready.is_empty() {
        ui.label(RichText::new("Loading the tracks…").color(P.faint));
        return;
    }
    let new_names = names.stand_ins();
    let artist = |id: Uuid| catalog.artist(id).map(|a| a.name.clone()).or_else(|| new_names.get(&id).cloned()).unwrap_or_else(|| "(an artist)".into());
    let album = |id: Uuid| catalog.album(id).map(|a| a.title.clone()).or_else(|| new_names.get(&id).cloned()).unwrap_or_else(|| "(an album)".into());
    let track = |id: Uuid| catalog.track_index(id).map(|i| catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| "(a track)".into());
    type Field<'a> = (&'static str, Box<dyn Fn(&model::Meta, &model::Facts) -> String + 'a>);
    let fields: Vec<Field> = vec![
        ("title", Box::new(|m, _| m.title.clone())),
        (
            "artist",
            Box::new(|m, _| {
                m.credits.iter().map(|c| if c.role == "primary" { artist(c.artist_id) } else { format!("{} ({})", artist(c.artist_id), c.role) }).collect::<Vec<_>>().join(", ")
            }),
        ),
        ("date", Box::new(|m, _| m.release_date.clone().unwrap_or_default())),
        ("status", Box::new(|_, f| f.status.clone())),
        ("genre", Box::new(|m, _| m.genres.join(", "))),
        (
            "album",
            Box::new(|_, f| {
                f.albums
                    .iter()
                    .map(|a| match (a.disc_number, a.track_number) {
                        (Some(d), Some(n)) if d > 1 => format!("{} #{d}-{n}", album(a.album_id)),
                        (_, Some(n)) => format!("{} #{n}", album(a.album_id)),
                        _ => album(a.album_id),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        ),
        ("related", Box::new(|_, f| f.relations.iter().map(|r| format!("{} “{}”", r.kind.replace('_', " "), track(r.track_id))).collect::<Vec<_>>().join(", "))),
        ("comment", Box::new(|m, _| m.comment.clone().unwrap_or_default())),
    ];
    let value_width = (ui.available_width() - 90.0).max(200.0);
    egui::Grid::new("track-editor-state-grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        for (key, value) in &fields {
            let before = summary(ready.iter().map(|r| value(&r.base.meta, &r.base.facts)));
            let after = summary(ready.iter().map(|r| value(&r.meta, &r.facts)));
            ui.label(RichText::new(format!("{key}:")).color(P.faint));
            ui.vertical(|ui| {
                ui.set_min_width(value_width);
                ui.set_max_width(value_width);
                if before == after {
                    ui.label(RichText::new(&before).color(P.text));
                } else {
                    ui.label(RichText::new(&before).color(P.faint).strikethrough());
                    ui.label(RichText::new(&after).color(P.accent));
                }
            });
            ui.end_row();
        }
    });
}

/// The values many tracks have for a field, in a few words.
fn summary(values: impl Iterator<Item = String>) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut total = 0;
    for v in values {
        total += 1;
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v, 1)),
        }
    }
    let shown = |v: &str| if v.is_empty() { "—".to_string() } else { v.to_string() };
    match counts.len() {
        0 => "—".into(),
        1 => shown(&counts[0].0),
        2 | 3 => {
            counts.sort_by_key(|c| std::cmp::Reverse(c.1));
            counts.iter().map(|(v, n)| format!("{} ({n})", shown(v))).collect::<Vec<_>>().join(" · ")
        }
        n => format!("{n} different (of {total})"),
    }
}

/// The library's matches for what's typed (artists, albums, tracks), found in the
/// background like the palette's: typing never waits.
#[derive(Default)]
struct Lookup {
    want: Option<(SearchPart, String)>,
    have: Option<(SearchPart, String)>,
    hits: Vec<u32>,
    answer: Arc<Mutex<Option<((SearchPart, String), Vec<u32>)>>>,
}

impl Lookup {
    fn hits(&mut self, shell: &Shell, part: SearchPart, query: &str) -> Vec<u32> {
        if let Some((key, hits)) = self.answer.lock().unwrap().take()
            && Some(&key) == self.want.as_ref()
        {
            self.have = Some(key);
            self.hits = hits;
        }
        let key = (part, query.trim().to_lowercase());
        if key.1.is_empty() {
            return Vec::new();
        }
        if self.want.as_ref() != Some(&key) {
            self.want = Some(key.clone());
            if let Some(catalog) = shell.catalog.value.clone() {
                let (answer, ctx) = (self.answer.clone(), shell.ctx.clone());
                shell.runtime.spawn_blocking(move || {
                    let hits = Search::default().run_part(&catalog, &key.1, key.0, 8).into_iter().map(|h| h.0).collect();
                    *answer.lock().unwrap() = Some((key, hits));
                    ctx.request_repaint();
                });
            }
        }
        // Until the new answer: the last one, if it was for the same kind of thing.
        match &self.have {
            Some((p, _)) if *p == part => self.hits.clone(),
            _ => Vec::new(),
        }
    }

    /// The matches for exactly `query`, found now if they aren't yet.
    fn now(&mut self, catalog: &Catalog, part: SearchPart, query: &str) -> Vec<u32> {
        let key = (part, query.trim().to_lowercase());
        if key.1.is_empty() {
            return Vec::new();
        }
        if self.have.as_ref() != Some(&key) {
            self.hits = Search::default().run_part(catalog, &key.1, part, 8).into_iter().map(|h| h.0).collect();
            self.have = Some(key.clone());
            self.want = Some(key);
        }
        self.hits.clone()
    }
}
