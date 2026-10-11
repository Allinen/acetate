//! A search field in a form, with its matches listed under it as you type (artists or
//! albums from the catalog, fuzzy as in the palette), plus extra choices the form adds
//! ("New artist “…”"). ↑/↓ choose, Enter takes the chosen one (Shift+Enter too: the form
//! decides what Shift means), Esc empties the field. After a choice the field is empty
//! again and keeps the cursor, so the next one can be typed right away.
//!
//! The search runs in the background, like the palette's: typing never waits.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::catalog::search::{Part, Search};
use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::Shell;

/// What was chosen: a match (its position in the catalog) or one of the extras.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Hit(u32),
    Extra(usize),
}

pub struct InlineSearch {
    part: Part,
    pub query: String,
    asked: u64,
    answer: Arc<Mutex<Option<(u64, Vec<u32>)>>>,
    hits: Vec<u32>,
    selected: usize,
    /// Extras (by id) that are never offered: the one the form is for, those chosen already.
    pub skip: Vec<uuid::Uuid>,
}

impl InlineSearch {
    pub fn new(part: Part) -> InlineSearch {
        InlineSearch { part, query: String::new(), asked: 0, answer: Arc::default(), hits: Vec::new(), selected: 0, skip: Vec::new() }
    }

    /// The field (`id`, so it can be focused from outside) and, while it has text, its
    /// matches and `extras`. Returns the choice, and whether Shift was held.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &Shell, catalog: Option<&Catalog>, id: egui::Id, hint: &str, extras: &[String]) -> Option<(Choice, bool)> {
        if let Some((number, hits)) = self.answer.lock().unwrap().take()
            && number == self.asked
        {
            self.hits = hits;
            self.selected = 0;
        }
        let focused = ui.ctx().memory(|m| m.has_focus(id));
        // Keys for the list, taken before the field sees them.
        let (up, down, enter, shift_enter, escape) = if focused {
            ui.ctx().input_mut(|i| {
                let shift_enter = i.consume_key(Modifiers::SHIFT, Key::Enter);
                let enter = i.consume_key(Modifiers::NONE, Key::Enter);
                let up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
                let down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
                let escape = !self.query.is_empty() && i.consume_key(Modifiers::NONE, Key::Escape);
                (up, down, enter, shift_enter, escape)
            })
        } else {
            (false, false, false, false, false)
        };
        let response = ui.add(egui::TextEdit::singleline(&mut self.query).id(id).hint_text(hint).desired_width(420.0));
        if escape {
            self.query.clear();
            self.hits.clear();
        }
        if response.changed() {
            self.ask(shell, catalog);
        }

        let query = self.query.trim();
        let count = if query.is_empty() { 0 } else { self.hits.len() + extras.len() };
        if count == 0 {
            return None;
        }
        if down {
            self.selected = (self.selected + 1).min(count - 1);
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(count - 1);
        let entry = |n: usize| if n < self.hits.len() { Choice::Hit(self.hits[n]) } else { Choice::Extra(n - self.hits.len()) };
        let mut chosen = (enter || shift_enter).then(|| (entry(self.selected), shift_enter));

        let frame = egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(4));
        frame.show(ui, |ui| {
            ui.set_width(420.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            for n in 0..count {
                let (title, sub) = match (entry(n), catalog) {
                    (Choice::Hit(i), Some(c)) => describe(c, self.part, i),
                    (Choice::Extra(e), _) => (extras[e].clone(), String::new()),
                    _ => continue,
                };
                let text = if sub.is_empty() {
                    RichText::new(title).font(font(13.5, medium()))
                } else {
                    RichText::new(format!("{title}   {sub}")).font(font(13.5, medium()))
                };
                let row = ui.selectable_label(n == self.selected, text);
                if row.clicked() {
                    chosen = Some((entry(n), ui.input(|i| i.modifiers.shift)));
                }
            }
        });
        if chosen.is_some() {
            self.query.clear();
            self.hits.clear();
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        chosen
    }

    /// Types `text` in (from elsewhere: a tag in the file), searching for it.
    pub fn set_query(&mut self, text: &str, shell: &Shell) {
        self.query = text.to_string();
        let catalog = shell.catalog.value.clone();
        self.ask(shell, catalog.as_deref());
    }

    fn ask(&mut self, shell: &Shell, catalog: Option<&Catalog>) {
        self.asked += 1;
        let query = self.query.trim().to_string();
        let Some(catalog) = shell.catalog.value.clone().filter(|_| catalog.is_some() && !query.is_empty()) else {
            self.hits.clear();
            return;
        };
        let (number, answer, ctx, part, skip) = (self.asked, self.answer.clone(), shell.ctx.clone(), self.part, self.skip.clone());
        shell.runtime.spawn_blocking(move || {
            let mut hits: Vec<u32> = Search::default().run_part(&catalog, &query, part, 8 + skip.len()).into_iter().map(|h| h.0).collect();
            if part == Part::Extras {
                hits.retain(|&h| catalog.extras.get(h as usize).is_none_or(|e| !skip.contains(&e.id)));
            }
            hits.truncate(8);
            *answer.lock().unwrap() = Some((number, hits));
            ctx.request_repaint();
        });
    }
}

pub use crate::catalog::search::describe;
