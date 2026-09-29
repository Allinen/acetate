//! Timing lyrics while listening (the lyrics editor's "Sync…", in a popup): the text as
//! lines, each with its time. Play the track; Enter gives the chosen line the time playing
//! now and moves to the next one, so tapping along as it's sung times the whole song. ←/→
//! nudge a line's time by a tenth of a second, Delete takes it away, `g` plays from just
//! before it, ↑/↓ (or j/k) choose a line, a click on a time plays from there, Space pauses,
//! Shift+←/→ seek. The line being sung is lit, to check the timing.
//!
//! Parts of the song: `h` adds a heading before the chosen line (`Verse 1: Wrenfield`,
//! written `[Verse 1: Wrenfield]`, see `crate::lyrics`); it needs no time (Enter's tapping
//! along passes over headings) and lasts until the next one.
//!
//! The words can be changed here too, without going back to the text: F2 (or a double
//! click) edits a line's (a heading's label), `o` adds a line after it, Shift+Delete removes it (the marked
//! ones, if any). Ctrl+Z takes back the last change made here.
//!
//! Languages per line (opt in: "A language per line", on by itself when a line has one):
//! each line shows its language, the entry's unless it says another; `l` (or a click on
//! it) changes the chosen line's, empty for the entry's again. Several at once: mark them
//! (`x`, Ctrl+click; Shift+↑/↓ or Shift+click for a range), then `l`. Written as our own
//! tag, `[lang:ja]` after the line's times (see `crate::lyrics`).
//!
//! What it writes is LRC, the text the lyrics are kept as ("[01:23.45]a line"). A line
//! stays exactly as it was written until it's changed (tags such as `[ar:…]`, a chorus's
//! second time, times written another way). An `[offset:…]` tag is taken into account: a
//! line gets the time that makes it show when it's heard.

use std::time::Duration;

use eframe::egui::{self, Key, Modifiers, RichText, Sense};

use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::lyrics;
use crate::playback::PlayerView;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, Action, Shell};

/// `g` plays from this long before the line, to hear it coming.
const LEAD_IN: Duration = Duration::from_secs(2);
/// ←/→ move a line's time by this much.
const NUDGE_MS: i64 = 100;
/// Shift+←/→ seek by this much.
const SEEK_S: f32 = 5.0;
pub const LANGUAGE_ID: &str = "sync-line-language";
pub const TEXT_ID: &str = "sync-line-text";
/// Changes Ctrl+Z can take back.
const UNDO_DEPTH: usize = 200;

#[derive(Debug, Clone, PartialEq)]
struct Line {
    /// Its (first) time as written (before the offset), in milliseconds.
    at: Option<i64>,
    /// More times, as written (a chorus sung again): kept as they are.
    more: Vec<String>,
    /// Its own language (`[lang:…]`), if it says one.
    language: Option<String>,
    /// The words.
    text: String,
    /// A tag line (`[ar:…]`, `[offset:…]`): kept, not timed.
    tag: bool,
    /// A heading (`[Verse 1: Wrenfield]`): `text` is what's in its brackets.
    heading: bool,
    /// The line as it was written, until it's changed.
    original: Option<String>,
}

impl Line {
    fn read(raw: &str) -> Line {
        let mut line = Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: false, original: Some(raw.to_string()) };
        let mut rest = raw.trim_start();
        while let Some(end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let tag = &rest[1..=end];
            if let Some(at) = lyrics::timestamp(tag) {
                match line.at {
                    None => line.at = Some(at),
                    Some(_) => line.more.push(rest[..end + 2].to_string()),
                }
            } else if let Some(language) = tag.strip_prefix("lang:") {
                line.language = Some(language.trim().to_string()).filter(|l| !l.is_empty());
            } else if rest[end + 2..].trim().is_empty() && line.language.is_none() && line.more.is_empty() && lyrics::section(tag).is_some() {
                // A heading, alone on its line (a time before it allowed).
                line.heading = true;
                line.text = tag.to_string();
                return line;
            } else {
                // Another tag: a whole line of it is metadata; else it's part of the words.
                line.tag = line.at.is_none() && rest[end + 2..].trim().is_empty() && tag.contains(':');
                break;
            }
            rest = &rest[end + 2..];
        }
        line.text = rest.to_string();
        line
    }

    fn write(&self) -> String {
        if let Some(original) = &self.original {
            return original.clone();
        }
        let mut out = self.at.map(stamp).unwrap_or_default();
        out.extend(self.more.iter().cloned());
        if self.heading {
            out.push_str(&format!("[{}]", self.text.trim()));
            return out;
        }
        if let Some(language) = &self.language {
            out.push_str(&format!("[lang:{language}]"));
        }
        out.push_str(&self.text);
        out
    }
}

pub struct Sync {
    lines: Vec<Line>,
    /// A trailing newline, kept.
    newline_at_end: bool,
    /// `[offset:…]`, in milliseconds (a positive one shows lines sooner).
    offset_ms: i64,
    /// The line Enter times next.
    cursor: usize,
    scroll_to_cursor: bool,
    /// Languages per line shown (opt in).
    languages: bool,
    /// The line whose language is being typed (for it, or for the marked lines), and
    /// what's typed.
    editing_language: Option<(usize, String)>,
    /// The line whose words are being typed, and what's typed.
    editing_text: Option<(usize, String)>,
    /// Marked lines (for their language, or removing them), and where a range starts.
    marked: std::collections::BTreeSet<usize>,
    anchor: Option<usize>,
    /// The lines before each change, newest last (Ctrl+Z).
    undo: Vec<Vec<Line>>,
}

impl Sync {
    pub fn new(body: &str) -> Sync {
        let lines: Vec<Line> = body.lines().map(Line::read).collect();
        let offset_ms = lines
            .iter()
            .filter(|l| l.tag)
            .find_map(|l| l.text.trim().strip_prefix("[offset:").and_then(|r| r.strip_suffix(']')).and_then(|v| v.trim().parse().ok()))
            .unwrap_or(0);
        let cursor = lines.iter().position(|l| !l.tag && l.at.is_none()).or_else(|| lines.iter().position(|l| !l.tag)).unwrap_or(0);
        let languages = lines.iter().any(|l| l.language.is_some());
        Sync {
            lines,
            newline_at_end: body.ends_with('\n'),
            offset_ms,
            cursor,
            scroll_to_cursor: true,
            languages,
            editing_language: None,
            editing_text: None,
            marked: Default::default(),
            anchor: None,
            undo: Vec::new(),
        }
    }

    /// The lyrics as text again.
    pub fn text(&self) -> String {
        let mut text = self.lines.iter().map(Line::write).collect::<Vec<_>>().join("\n");
        if self.newline_at_end {
            text.push('\n');
        }
        text
    }

    /// When a line shows (its time with the offset), in milliseconds.
    fn shows_at(&self, line: &Line) -> Option<i64> {
        line.at.map(|at| (at - self.offset_ms).max(0))
    }

    /// Keeps the lines as they are, for Ctrl+Z (call before changing them).
    fn remember(&mut self) {
        if self.undo.len() == UNDO_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(self.lines.clone());
    }

    /// Takes back the last change. False if there's none.
    fn undo(&mut self) -> bool {
        let Some(lines) = self.undo.pop() else { return false };
        self.lines = lines;
        self.cursor = self.cursor.min(self.lines.len().saturating_sub(1));
        self.marked.clear();
        self.anchor = None;
        true
    }

    fn set(&mut self, n: usize, at: Option<i64>) {
        self.remember();
        if let Some(line) = self.lines.get_mut(n) {
            line.at = at.map(|at| at.max(0));
            line.original = None;
        }
    }

    /// Sets the language of lines `lines` (empty: the lyrics' own). True if one changed.
    fn set_language(&mut self, lines: &[usize], language: &str) -> bool {
        let language = Some(language.trim().to_string()).filter(|l| !l.is_empty());
        let takes = |l: &Line| !l.tag && !l.heading && l.language != language;
        if !lines.iter().any(|&n| self.lines.get(n).is_some_and(takes)) {
            return false;
        }
        self.remember();
        for &n in lines {
            if let Some(line) = self.lines.get_mut(n).filter(|l| takes(l)) {
                line.language.clone_from(&language);
                line.original = None;
            }
        }
        true
    }

    /// Sets a line's words. True if they changed.
    fn set_text(&mut self, n: usize, text: &str) -> bool {
        if self.lines.get(n).is_none_or(|l| l.text == text) {
            return false;
        }
        self.remember();
        let line = &mut self.lines[n];
        line.text = text.to_string();
        line.original = None;
        true
    }

    /// Adds an empty line after line `n` (at the start, with none); it becomes the chosen one.
    fn insert_after(&mut self, n: Option<usize>) {
        self.remember();
        let at = n.map_or(0, |n| n + 1).min(self.lines.len());
        self.lines.insert(at, Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: false, original: None });
        self.cursor = at;
        self.marked.clear();
        self.anchor = None;
    }

    /// Adds an empty heading before line `n`; it becomes the chosen one.
    fn insert_heading_before(&mut self, n: usize) {
        self.remember();
        let at = n.min(self.lines.len());
        self.lines.insert(at, Line { at: None, more: Vec::new(), language: None, text: String::new(), tag: false, heading: true, original: None });
        self.cursor = at;
        self.marked.clear();
        self.anchor = None;
    }

    /// The next line after `n` that Enter's tapping along times (not a heading).
    fn next_sung(&self, n: usize) -> Option<usize> {
        (n + 1..self.lines.len()).find(|&i| !self.lines[i].tag && !self.lines[i].heading)
    }

    /// Removes the marked lines, or else the chosen one.
    fn remove(&mut self) {
        let doomed: Vec<usize> = if self.marked.is_empty() { vec![self.cursor] } else { self.marked.iter().copied().collect() };
        if doomed.iter().all(|&n| n >= self.lines.len()) {
            return;
        }
        self.remember();
        for &n in doomed.iter().rev() {
            if n < self.lines.len() {
                self.lines.remove(n);
            }
        }
        self.cursor = doomed[0].min(self.lines.len().saturating_sub(1));
        self.marked.clear();
        self.anchor = None;
    }

    /// Marks the lines from the anchor (or `from`) to `to` (a range).
    fn mark_range(&mut self, from: usize, to: usize) {
        let start = *self.anchor.get_or_insert(from);
        for n in start.min(to)..=start.max(to) {
            if self.lines.get(n).is_some_and(|l| !l.tag) {
                self.marked.insert(n);
            }
        }
    }

    /// The lines a language is for: the marked ones, or else line `n`.
    fn language_targets(&self, n: usize) -> Vec<usize> {
        if self.marked.is_empty() { vec![n] } else { self.marked.iter().copied().collect() }
    }

    fn start_text(&mut self, ctx: &egui::Context, n: usize) {
        if let Some(line) = self.lines.get(n).filter(|l| !l.tag) {
            self.editing_text = Some((n, line.text.clone()));
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(TEXT_ID)));
        }
    }

    /// The next line after `n` that can be timed.
    fn next(&self, n: usize) -> Option<usize> {
        (n + 1..self.lines.len()).find(|&i| !self.lines[i].tag)
    }

    fn previous(&self, n: usize) -> Option<usize> {
        (0..n).rev().find(|&i| !self.lines[i].tag)
    }

    /// The editor (in the popup). `language`: the entry's. True when something changed
    /// (the text is then `text()`).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, d: &TrackDetail, player: &PlayerView, language: &str) -> bool {
        let playing_this = playing_track(player) == Some(d.id);
        let position = playing_this.then(|| player.position_now());
        let mut changed = false;

        // ---- Keys (outside text fields) ----
        let ctx = ui.ctx().clone();
        if !ctx.egui_wants_keyboard_input() {
            let pressed = |modifiers: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(modifiers, k));
            // (Shift first: a key asked for without modifiers matches it with Shift too.)
            if pressed(Modifiers::SHIFT, Key::ArrowLeft) {
                shell.act(Action::SeekBy(-SEEK_S));
            }
            if pressed(Modifiers::SHIFT, Key::ArrowRight) {
                shell.act(Action::SeekBy(SEEK_S));
            }
            if pressed(Modifiers::COMMAND, Key::Z) && self.undo() {
                changed = true;
            }
            if pressed(Modifiers::SHIFT, Key::Delete) {
                self.remove();
                changed = true;
            }
            if pressed(Modifiers::SHIFT, Key::ArrowDown) {
                let from = self.cursor;
                self.cursor = self.next(self.cursor).unwrap_or(self.cursor);
                self.mark_range(from, self.cursor);
                self.scroll_to_cursor = true;
            }
            if pressed(Modifiers::SHIFT, Key::ArrowUp) {
                let from = self.cursor;
                self.cursor = self.previous(self.cursor).unwrap_or(self.cursor);
                self.mark_range(from, self.cursor);
                self.scroll_to_cursor = true;
            }
            if pressed(Modifiers::NONE, Key::X) {
                if !self.marked.remove(&self.cursor) {
                    self.marked.insert(self.cursor);
                }
                self.anchor = Some(self.cursor);
            }
            if pressed(Modifiers::NONE, Key::F2) {
                self.start_text(&ctx, self.cursor);
            }
            if pressed(Modifiers::NONE, Key::H) {
                self.insert_heading_before(self.cursor);
                self.start_text(&ctx, self.cursor);
                changed = true;
            }
            if pressed(Modifiers::NONE, Key::O) {
                self.insert_after((!self.lines.is_empty()).then_some(self.cursor));
                self.start_text(&ctx, self.cursor);
                changed = true;
            }
            if pressed(Modifiers::NONE, Key::Space) {
                if playing_this {
                    shell.act(Action::TogglePause);
                } else {
                    play_from(shell, catalog, d, false, Duration::ZERO);
                }
            }
            if pressed(Modifiers::NONE, Key::ArrowDown) || pressed(Modifiers::NONE, Key::J) {
                self.cursor = self.next(self.cursor).unwrap_or(self.cursor);
                self.scroll_to_cursor = true;
            }
            if pressed(Modifiers::NONE, Key::ArrowUp) || pressed(Modifiers::NONE, Key::K) {
                self.cursor = self.previous(self.cursor).unwrap_or(self.cursor);
                self.scroll_to_cursor = true;
            }
            if pressed(Modifiers::NONE, Key::Enter) {
                // On a heading: on to its first line (a heading needs no time).
                if self.lines.get(self.cursor).is_some_and(|l| l.heading) {
                    self.cursor = self.next_sung(self.cursor).unwrap_or(self.cursor);
                    self.scroll_to_cursor = true;
                } else if let Some(position) = position {
                    self.set(self.cursor, Some(position.as_millis() as i64 + self.offset_ms));
                    changed = true;
                    self.cursor = self.next_sung(self.cursor).unwrap_or(self.cursor);
                    self.scroll_to_cursor = true;
                }
            }
            if let Some(at) = self.lines.get(self.cursor).and_then(|l| l.at) {
                if pressed(Modifiers::NONE, Key::ArrowLeft) {
                    self.set(self.cursor, Some(at - NUDGE_MS));
                    changed = true;
                }
                if pressed(Modifiers::NONE, Key::ArrowRight) {
                    self.set(self.cursor, Some(at + NUDGE_MS));
                    changed = true;
                }
                if pressed(Modifiers::NONE, Key::Delete) {
                    self.set(self.cursor, None);
                    changed = true;
                }
            }
            if pressed(Modifiers::NONE, Key::G)
                && let Some(shows) = self.lines.get(self.cursor).and_then(|l| self.shows_at(l))
            {
                play_from(shell, catalog, d, playing_this, Duration::from_millis(shows as u64).saturating_sub(LEAD_IN));
            }
            if self.languages && pressed(Modifiers::NONE, Key::L) {
                let typed = self.lines.get(self.cursor).and_then(|l| l.language.clone()).unwrap_or_default();
                self.editing_language = Some((self.cursor, typed));
                ctx.memory_mut(|m| m.request_focus(egui::Id::new(LANGUAGE_ID)));
            }
            // The key typed (`l`, `o`…) isn't the first letter of what's typed next.
            if self.editing_language.is_some() || self.editing_text.is_some() {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
            }
        }

        // ---- What plays ----
        ui.horizontal_wrapped(|ui| {
            match position {
                Some(position) => {
                    let state = if player.paused { "paused" } else { "playing" };
                    ui.label(RichText::new(clock(position.as_millis() as i64)).font(font(22.0, semibold())).color(P.accent));
                    ui.label(RichText::new(state).color(P.muted));
                    if player.playing && !player.paused {
                        ui.ctx().request_repaint_after(Duration::from_millis(40));
                    }
                }
                None => {
                    if ui.button("▶ Play this track, to time it  (Space)").clicked() {
                        play_from(shell, catalog, d, false, Duration::ZERO);
                    }
                }
            }
            if self.offset_ms != 0 {
                ui.label(RichText::new(format!("· offset {:+} ms (taken into account)", self.offset_ms)).color(P.faint));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let lines_with = self.lines.iter().any(|l| l.language.is_some());
                ui.add_enabled_ui(!lines_with, |ui| {
                    ui.checkbox(&mut self.languages, "A language per line")
                        .on_hover_text(format!("Lines in another language than the lyrics' ({language}) say which"))
                        .on_disabled_hover_text("Some lines say their language");
                });
            });
        });
        let mut keys = String::from(
            "Enter: this line starts now (then the next) · ←/→ 0.1 s earlier/later · Delete: no time · g: from just before it · ↑/↓ or j/k: another line · Space: pause · Shift+←/→: 5 s back/on · F2 or double click: its words · o: a line after it · h: a heading before it (Verse 1: Wrenfield) · Shift+Delete: remove it · x, Shift+↑/↓: mark · Ctrl+Z: undo",
        );
        if self.languages {
            keys.push_str(" · l: its language (the marked lines')");
        }
        keys.push_str(" · Ctrl+Enter: save · Esc: back to the text");
        ui.label(RichText::new(keys).color(P.faint).small());
        if !self.marked.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} marked", self.marked.len())).color(P.accent));
                if ui.small_button("Clear the marks").clicked() {
                    self.marked.clear();
                    self.anchor = None;
                }
            });
        }
        ui.add_space(6.0);

        // ---- The lines ----
        // The line sung now: the last one started (as the lyrics view lights it).
        let sung = position.and_then(|p| {
            let p = p.as_millis() as i64;
            self.lines.iter().enumerate().filter(|(_, l)| !l.heading).filter_map(|(n, l)| self.shows_at(l).filter(|at| *at <= p).map(|at| (at, n))).max().map(|(_, n)| n)
        });
        let mut clicked_line = None;
        let mut double_clicked = None;
        let mut play_at = None;
        let mut language_clicked = None;
        let mut language_done = None;
        let mut text_done = None;
        let scroll_to_cursor = std::mem::take(&mut self.scroll_to_cursor);
        let editing_language = &mut self.editing_language;
        let editing_text = &mut self.editing_text;
        let marked = &self.marked;
        egui::ScrollArea::vertical().id_salt(("lyrics-sync", d.id)).auto_shrink([false, false]).show(ui, |ui| {
            let mut last = None;
            for (n, line) in self.lines.iter().enumerate() {
                let shows = line.at.map(|at| (at - self.offset_ms).max(0));
                // A time earlier than the line before it: probably a slip.
                let out_of_order = matches!((shows, last), (Some(s), Some(l)) if s < l);
                if shows.is_some() {
                    last = shows;
                }
                let current = n == self.cursor;
                let is_marked = marked.contains(&n);
                let frame = egui::Frame::NONE
                    .fill(if current { P.accent_soft } else { egui::Color32::TRANSPARENT })
                    .stroke(egui::Stroke::new(1.0, if is_marked { P.accent } else { egui::Color32::TRANSPARENT }))
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 4));
                let response = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if line.tag {
                                ui.label(RichText::new(&line.text).monospace().color(P.faint));
                                return;
                            }
                            if line.heading {
                                if let Some((editing, typed)) = editing_text.as_mut()
                                    && *editing == n
                                {
                                    let field = egui::TextEdit::singleline(typed)
                                        .id(egui::Id::new(TEXT_ID))
                                        .hint_text("Verse 1: Wrenfield  (a part, then who sings it)")
                                        .font(font(15.0, semibold()))
                                        .desired_width(f32::INFINITY);
                                    if ui.add(field).lost_focus() {
                                        let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                                        text_done = Some((n, (!cancelled).then(|| typed.clone())));
                                    }
                                    return;
                                }
                                let time = line.at.map(clock).unwrap_or_default();
                                ui.label(RichText::new(format!("{time:<8}")).font(font(15.0, medium())).monospace().color(P.faint));
                                let (label, artists) = match lyrics::section(&line.text) {
                                    Some(section) => (section.label.to_uppercase(), section.artists.join(", ")),
                                    None => ("(an empty heading: F2 names it)".to_string(), String::new()),
                                };
                                ui.label(RichText::new(label).font(font(15.0, semibold())).color(P.accent));
                                if !artists.is_empty() {
                                    ui.label(RichText::new(format!("·  {artists}")).font(font(15.0, medium())).color(P.muted));
                                }
                                return;
                            }
                            let time = match line.at {
                                Some(at) => RichText::new(clock(at)).font(font(15.0, medium())).monospace().color(if out_of_order { P.danger } else { P.muted }),
                                None => RichText::new("   —    ").font(font(15.0, medium())).monospace().color(P.faint),
                            };
                            let time = ui.add(egui::Label::new(time).sense(Sense::click()));
                            if let Some(shows) = shows
                                && time.on_hover_text(if out_of_order { "Earlier than the line before it. Click: play from here" } else { "Play from here" }).clicked()
                            {
                                play_at = Some(Duration::from_millis(shows as u64));
                            }
                            if self.languages {
                                match editing_language {
                                    Some((editing, typed)) if *editing == n => {
                                        let field = ui.add(egui::TextEdit::singleline(typed).id(egui::Id::new(LANGUAGE_ID)).hint_text(language).desired_width(56.0));
                                        // Enter (or a click elsewhere) takes it; Esc doesn't.
                                        if field.lost_focus() {
                                            let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                                            language_done = Some((n, (!cancelled).then(|| typed.clone())));
                                        }
                                    }
                                    _ => {
                                        let (text, color) = match &line.language {
                                            Some(own) => (own.as_str(), P.accent),
                                            None => (language, P.faint),
                                        };
                                        let chip = ui.add(egui::Label::new(RichText::new(format!("{text:<5}")).monospace().color(color)).sense(Sense::click()));
                                        if chip.on_hover_text("Its language (click, or l): empty for the lyrics' own").clicked() {
                                            language_clicked = Some(n);
                                        }
                                    }
                                }
                            }
                            if let Some((editing, typed)) = editing_text.as_mut()
                                && *editing == n
                            {
                                let field = egui::TextEdit::singleline(typed).id(egui::Id::new(TEXT_ID)).font(font(18.0, semibold())).desired_width(f32::INFINITY);
                                // Enter (or a click elsewhere) takes it; Esc doesn't.
                                if ui.add(field).lost_focus() {
                                    let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                                    text_done = Some((n, (!cancelled).then(|| typed.clone())));
                                }
                                return;
                            }
                            let words = if line.text.trim().is_empty() { "(a pause)" } else { line.text.as_str() };
                            let color = if Some(n) == sung { P.accent } else if line.text.trim().is_empty() { P.faint } else { P.text };
                            let size = if Some(n) == sung { 20.0 } else { 18.0 };
                            ui.add(egui::Label::new(RichText::new(words).font(font(size, semibold())).color(color)).wrap());
                        });
                    })
                    .response;
                let click = response.interact(Sense::click());
                if !line.tag && click.double_clicked() {
                    double_clicked = Some(n);
                } else if !line.tag && click.clicked() {
                    clicked_line = Some((n, ui.input(|i| i.modifiers)));
                }
                if current && scroll_to_cursor {
                    response.scroll_to_me(Some(egui::Align::Center));
                }
            }
        });
        if let Some((n, modifiers)) = clicked_line {
            // Ctrl+click: marked or not; Shift+click: a range; a click: just chosen.
            if modifiers.command {
                if !self.marked.remove(&n) {
                    self.marked.insert(n);
                }
                self.anchor = Some(n);
            } else if modifiers.shift {
                self.mark_range(self.cursor, n);
            }
            self.cursor = n;
        }
        if let Some(n) = double_clicked {
            self.cursor = n;
            self.start_text(&ctx, n);
        }
        if let Some(n) = language_clicked {
            self.cursor = n;
            self.editing_language = Some((n, self.lines[n].language.clone().unwrap_or_default()));
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(LANGUAGE_ID)));
        }
        if let Some((n, typed)) = language_done {
            self.editing_language = None;
            if let Some(typed) = typed {
                let targets = self.language_targets(n);
                changed |= self.set_language(&targets, &typed);
            }
        }
        if let Some((n, typed)) = text_done {
            self.editing_text = None;
            if let Some(typed) = typed {
                changed |= self.set_text(n, &typed);
            }
        }
        if let Some(at) = play_at {
            play_from(shell, catalog, d, playing_this, at);
        }
        changed
    }
}

/// Plays the track from `at`: a seek if it's playing already, else it starts playing
/// (from the start: a seek can't wait for it yet).
fn play_from(shell: &mut Shell, catalog: &Catalog, d: &TrackDetail, playing_this: bool, at: Duration) {
    if playing_this {
        shell.act(Action::SeekTo(at));
    } else if let Some(index) = catalog.track_index(d.id) {
        shell.act(Action::Play { tracks: vec![index], start: 0 });
    }
}

/// `[mm:ss.xx]`, as LRC writes a time.
fn stamp(ms: i64) -> String {
    format!("[{}]", clock(ms))
}

/// `mm:ss.xx`.
fn clock(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{:02}:{:02}.{:02}", ms / 60_000, ms / 1000 % 60, ms % 1000 / 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_lines_come_back_as_they_were() {
        let body = "[ar:Someone]\n[00:01.5]one\n[00:03.00][00:10.00]chorus\n[lang:ja][00:12.00]x\nnot timed\n\n";
        assert_eq!(Sync::new(body).text(), body);
        assert_eq!(Sync::new("plain\nlyrics").text(), "plain\nlyrics");
    }

    #[test]
    fn timing_lines_writes_lrc_with_the_offset() {
        let mut sync = Sync::new("[offset:+500]\none\ntwo\n[00:09.00]three\n[00:03.00][00:10.00]chorus");
        assert_eq!((sync.offset_ms, sync.cursor), (500, 1), "the first line without a time");
        // Heard at 1.2 s: written so that, with the offset, it shows at 1.2 s.
        sync.set(1, Some(1200 + sync.offset_ms));
        sync.set(2, Some(61_234));
        sync.set(3, None);
        sync.set(4, Some(3500));
        assert_eq!(sync.text(), "[offset:+500]\n[00:01.70]one\n[01:01.23]two\nthree\n[00:03.50][00:10.00]chorus");
        assert_eq!(lyrics::parse(&sync.text())[0].at, Duration::from_millis(1200));
        assert_eq!((sync.next(0), sync.previous(1)), (Some(1), None), "tag lines are passed over");
    }

    #[test]
    fn lines_edited_added_removed_marked_and_taken_back() {
        let mut sync = Sync::new("[ar:Someone]\n[00:01.00]one\ntwo\nthree");
        assert!(sync.set_text(2, "two, fixed"));
        sync.cursor = 2;
        sync.insert_after(Some(2));
        sync.set_text(3, "new");
        assert_eq!(sync.text(), "[ar:Someone]\n[00:01.00]one\ntwo, fixed\nnew\nthree");

        // Marked (a range, tag lines left out): all get the language, then go.
        sync.cursor = 0;
        sync.mark_range(0, 3);
        assert_eq!(sync.marked.iter().copied().collect::<Vec<_>>(), [1, 2, 3]);
        assert!(sync.set_language(&sync.language_targets(0), "ja"));
        assert_eq!(sync.text(), "[ar:Someone]\n[00:01.00][lang:ja]one\n[lang:ja]two, fixed\n[lang:ja]new\nthree");
        sync.remove();
        assert_eq!(sync.text(), "[ar:Someone]\nthree");

        // Each change taken back, newest first.
        assert!(sync.undo());
        assert_eq!(sync.text(), "[ar:Someone]\n[00:01.00][lang:ja]one\n[lang:ja]two, fixed\n[lang:ja]new\nthree");
        while sync.undo() {}
        assert_eq!(sync.text(), "[ar:Someone]\n[00:01.00]one\ntwo\nthree", "back to as it was written");
    }

    #[test]
    fn headings_are_kept_added_and_passed_over() {
        let body = "[Verse 1: Wrenfield]\n[00:01.00]one\n[00:30.00][Chorus]\ntwo";
        let mut sync = Sync::new(body);
        assert_eq!(sync.text(), body);
        assert!(sync.lines[0].heading && sync.lines[2].heading && sync.lines[2].at == Some(30_000));
        assert_eq!(sync.next_sung(1), Some(3), "tapping along passes over the heading");
        assert!(!sync.set_language(&[0], "ja"), "a heading has no language");

        sync.insert_heading_before(3);
        sync.set_text(3, "Bridge: A & B");
        assert_eq!(sync.text(), "[Verse 1: Wrenfield]\n[00:01.00]one\n[00:30.00][Chorus]\n[Bridge: A & B]\ntwo");
    }

        #[test]
    fn a_line_gets_its_own_language_and_back() {
        let mut sync = Sync::new("[00:01.00]hello\nこんにちは");
        assert!(!sync.languages, "opt in");
        sync.set_language(&[1], "ja");
        sync.set_language(&[0], " ");
        assert_eq!(sync.text(), "[00:01.00]hello\n[lang:ja]こんにちは");
        let mut again = Sync::new(&sync.text());
        assert!(again.languages, "on by itself when a line has one");
        again.set(1, Some(2000));
        assert_eq!(again.text(), "[00:01.00]hello\n[00:02.00][lang:ja]こんにちは");
        again.set_language(&[1], "");
        assert_eq!(again.text(), "[00:01.00]hello\n[00:02.00]こんにちは");
    }
}
