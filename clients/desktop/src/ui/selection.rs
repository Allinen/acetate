//! Marks in a list, the same in every list that has them (the track tables, Review's
//! lists, Unlinked extras, Didn't import, the trash).
//!
//! A list gives its rows as positions shown (0..len) and `id_at`, the row's key at one.
//! Marks are kept by key, so sorting and filtering don't lose them. The box in front of a
//! row, `x`, or Ctrl+click marks or unmarks one; Shift+click or Shift+↑/↓ a range from
//! where it starts: the row the cursor was on (moved there or clicked), which marks, or a
//! row just (un)marked by itself, whose state the range takes (so ranges unmark too).
//! Ctrl+A marks everything shown; Esc clears.

use std::collections::HashSet;
use std::hash::Hash;

use eframe::egui;

use crate::ui::widgets;

pub struct Selection<K> {
    marked: HashSet<K>,
    /// Where a range starts (a row shown), and whether the range marks (or unmarks).
    anchor: Option<(usize, bool)>,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Selection { marked: HashSet::new(), anchor: None }
    }
}

/// What the keys did.
#[derive(Debug, Default, Clone, Copy)]
pub struct Keys {
    /// Esc cleared the marks.
    pub cleared: bool,
}

impl<K: Copy + Eq + Hash> Selection<K> {
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    pub fn len(&self) -> usize {
        self.marked.len()
    }

    pub fn contains(&self, key: &K) -> bool {
        self.marked.contains(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &K> {
        self.marked.iter()
    }

    /// The marked rows shown, as positions.
    pub fn rows(&self, len: usize, id_at: impl Fn(usize) -> Option<K>) -> Vec<usize> {
        (0..len).filter(|&n| id_at(n).is_some_and(|key| self.marked.contains(&key))).collect()
    }

    /// The marked keys in the order shown (marked ones not shown last, in no order).
    pub fn ordered(&self, len: usize, id_at: impl Fn(usize) -> Option<K>) -> Vec<K> {
        let mut seen = HashSet::new();
        let shown = (0..len).filter_map(id_at).filter(|key| self.marked.contains(key));
        shown.chain(self.marked.iter().copied()).filter(|key| seen.insert(*key)).collect()
    }

    /// Marks and nothing else: these keys.
    pub fn set(&mut self, keys: impl IntoIterator<Item = K>) {
        self.marked = keys.into_iter().collect();
        self.anchor = None;
    }

    /// Keeps only the marks `keep` says yes to.
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        self.marked.retain(|key| keep(key));
    }

    /// Marks everything shown.
    pub fn all(&mut self, len: usize, id_at: impl Fn(usize) -> Option<K>) {
        self.marked.extend((0..len).filter_map(id_at));
    }

    /// Marks or unmarks one shown row; it becomes where ranges start, and they do as it
    /// did (mark, or unmark).
    pub fn toggle(&mut self, n: usize, id_at: impl Fn(usize) -> Option<K>) {
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
    pub fn range_to(&mut self, from: usize, to: usize, id_at: impl Fn(usize) -> Option<K>) {
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
    pub fn click(&mut self, n: usize, selected: usize, modifiers: egui::Modifiers, id_at: impl Fn(usize) -> Option<K>) {
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

    /// The keys, for a list with the keyboard (no text field has it): `x` marks the
    /// `cursor` row, Ctrl+A everything shown, Esc clears (`may_clear`: the list isn't
    /// holding Esc for something else).
    pub fn keys(&mut self, ctx: &egui::Context, cursor: usize, len: usize, id_at: impl Fn(usize) -> Option<K>, may_clear: bool) -> Keys {
        let (toggle, all, clear) = ctx.input_mut(|i| {
            (
                i.key_pressed(egui::Key::X) && i.modifiers.is_none(),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::A),
                may_clear && !self.marked.is_empty() && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if toggle {
            self.toggle(cursor, &id_at);
        }
        if all {
            self.all(len, &id_at);
        }
        if clear {
            self.clear();
        }
        Keys { cleared: clear }
    }

    /// The cursor moves with the arrow keys, hjkl, Page Up/Down, Home and End (`page`
    /// rows a page); with Shift the marks follow it as a range, else the range starts
    /// where it lands. Moved: the list scrolls to it.
    pub fn move_cursor(&mut self, ctx: &egui::Context, cursor: &mut usize, len: usize, page: usize, id_at: impl Fn(usize) -> Option<K>) -> widgets::Moved {
        let before = *cursor;
        let moved = widgets::move_selection(ctx, cursor, len, 1, page);
        if moved.moved {
            if ctx.input(|i| i.modifiers.shift) {
                self.range_to(before, *cursor, id_at);
            } else {
                self.cursor_at(*cursor);
            }
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(n: usize) -> Option<u32> {
        (n < 10).then_some(n as u32)
    }

    #[test]
    fn ranges_mark_and_unmark_from_where_they_start() {
        let mut s: Selection<u32> = Selection::default();
        s.cursor_at(2);
        s.range_to(2, 5, at);
        assert_eq!(s.len(), 4);
        // A row unmarked by itself: the range after it unmarks.
        s.toggle(4, at);
        assert!(!s.contains(&4));
        s.range_to(4, 2, at);
        assert_eq!(s.len(), 1);
        assert!(s.contains(&5));
        s.all(10, at);
        assert_eq!(s.len(), 10);
        s.retain(|k| k % 2 == 0);
        assert_eq!(s.rows(10, at), [0, 2, 4, 6, 8]);
        s.clear();
        assert!(s.is_empty());
    }
}
