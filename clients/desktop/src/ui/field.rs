//! The keys every search field shares: the page commandlines, the list filters, the
//! searches in forms and the palette. Each draws its own field and its own list; the keys
//! that work the same in all of them are here, taken before the field sees them.
//!
//! - Esc leaves the field; what's typed stays. (Outside a field, Esc goes back, or closes
//!   what's open: the app's keys.)
//! - Backspace on an empty field goes back, or does what the field says "back" is
//!   (a commandline: the last staged change back to edit). Only a fresh press on a field
//!   that was empty already: holding Backspace to delete what's typed stops at empty.
//! - ↑/↓, Tab and Enter, for a field that asks for them (a list under it).

use eframe::egui::{self, Key, Modifiers};

/// Where a key an empty field passes on to the app waits (`ctx.data`): the app's keys take
/// it next as if pressed outside a field.
pub const NAV: &str = "field-nav";

/// Which keys the field takes, besides Esc and Backspace.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rules {
    pub up: bool,
    pub down: bool,
    pub tab: bool,
    /// Enter and Shift+Enter.
    pub enter: bool,
    /// Backspace on an empty field is the field's own (`Keys::back`), not the app's "back".
    pub own_back: bool,
    /// On an empty field, a digit is a page key (or the volume's), passed on, not typed.
    pub page_digits: bool,
}

/// The keys pressed in the field this frame (taken: nothing else sees them).
#[derive(Debug, Default, Clone, Copy)]
pub struct Keys {
    pub up: bool,
    pub down: bool,
    pub tab: bool,
    pub enter: bool,
    pub shift_enter: bool,
    /// Backspace on an empty field, when the field's own (`Rules::own_back`).
    pub back: bool,
    /// Esc: the field was let go of.
    pub escape: bool,
}

/// The field `id`'s keys, by `rules`, while it has the cursor; `text` is what's in it (as
/// it was before this frame's keys).
pub fn keys(ctx: &egui::Context, id: egui::Id, text: &str, rules: Rules) -> Keys {
    if !ctx.memory(|m| m.has_focus(id)) {
        return Keys::default();
    }
    let empty = text.is_empty();
    let mut nav = None;
    let keys = ctx.input_mut(|i| {
        let mut keys = Keys::default();
        if empty {
            if rules.page_digits {
                const DIGITS: [Key; 10] = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
                let digit = |e: &egui::Event| match e {
                    egui::Event::Text(t) if t.len() == 1 && t.as_bytes()[0].is_ascii_digit() => Some(DIGITS[(t.as_bytes()[0] - b'0') as usize]),
                    _ => None,
                };
                if !i.modifiers.any()
                    && let Some(key) = i.events.iter().find_map(digit)
                {
                    i.events.retain(|e| digit(e).is_none());
                    nav = Some(key);
                }
            }
            // A fresh press only (not one repeating from being held), and all of them taken:
            // on an empty field they'd do nothing anyway.
            let fresh = i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Backspace, pressed: true, repeat: false, modifiers, .. } if !modifiers.any()));
            if i.consume_key(Modifiers::NONE, Key::Backspace) && fresh {
                if rules.own_back {
                    keys.back = true;
                } else {
                    nav = nav.or(Some(Key::Backspace));
                }
            }
        }
        keys.escape = i.consume_key(Modifiers::NONE, Key::Escape);
        keys.up = rules.up && i.consume_key(Modifiers::NONE, Key::ArrowUp);
        keys.down = rules.down && i.consume_key(Modifiers::NONE, Key::ArrowDown);
        keys.tab = rules.tab && i.consume_key(Modifiers::NONE, Key::Tab);
        // Shift+Enter first: a plain Enter asked for matches with Shift too.
        keys.shift_enter = rules.enter && i.consume_key(Modifiers::SHIFT, Key::Enter);
        keys.enter = rules.enter && i.consume_key(Modifiers::NONE, Key::Enter);
        keys
    });
    if keys.escape {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
    if let Some(key) = nav {
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique(NAV), Some(key)));
        ctx.request_repaint();
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, repeat: bool) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat, modifiers: Modifiers::NONE }
    }

    /// Runs one frame with `events`, the field `id` holding `text` and the cursor; what
    /// `keys` gave, and what it passed on to the app.
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, text: &str, rules: Rules) -> (Keys, Option<Key>) {
        let id = egui::Id::unique("field");
        let mut got = Keys::default();
        let input = egui::RawInput { events, ..Default::default() };
        let mut output = ctx.run_ui(input, |ui| {
            ui.memory_mut(|m| m.request_focus(id));
            got = keys(ui.ctx(), id, text, rules);
        });
        output.textures_delta.clear();
        let nav = ctx.data_mut(|d| d.remove_temp::<Option<Key>>(egui::Id::unique(NAV))).flatten();
        (got, nav)
    }

    fn focused(rules: Rules) -> egui::Context {
        let ctx = egui::Context::default();
        frame(&ctx, Vec::new(), "", rules);
        ctx
    }

    #[test]
    fn backspace_on_an_empty_field_goes_back() {
        let ctx = focused(Rules::default());
        assert_eq!(frame(&ctx, vec![key(Key::Backspace, false)], "", Rules::default()).1, Some(Key::Backspace));
    }

    /// Holding Backspace to delete what's typed doesn't go on to go back. (egui tells a
    /// repeat from the key being held down already, whatever the event says.)
    #[test]
    fn a_held_backspace_stops_at_empty() {
        let ctx = focused(Rules::default());
        // The press deletes the last letter (the field's, not taken)…
        assert_eq!(frame(&ctx, vec![key(Key::Backspace, false)], "a", Rules::default()).1, None);
        // …and the key, still held, repeats on the empty field.
        let (keys, nav) = frame(&ctx, vec![key(Key::Backspace, false), key(Key::Backspace, false)], "", Rules::default());
        assert!(!keys.back);
        assert_eq!(nav, None);
    }

    #[test]
    fn backspace_with_text_is_the_fields() {
        let ctx = focused(Rules::default());
        assert_eq!(frame(&ctx, vec![key(Key::Backspace, false)], "abc", Rules::default()).1, None);
    }

    #[test]
    fn a_field_can_take_back_itself() {
        let rules = Rules { own_back: true, ..Default::default() };
        let ctx = focused(rules);
        let (keys, nav) = frame(&ctx, vec![key(Key::Backspace, false)], "", rules);
        assert!(keys.back);
        assert_eq!(nav, None);
    }

    /// Esc lets go of the field (the text is the owner's: it stays).
    #[test]
    fn esc_leaves_the_field() {
        let ctx = focused(Rules::default());
        let (keys, _) = frame(&ctx, vec![key(Key::Escape, false)], "abc", Rules::default());
        assert!(keys.escape);
        assert!(!ctx.memory(|m| m.has_focus(egui::Id::unique("field"))));
    }

    #[test]
    fn digits_are_page_keys_only_where_asked() {
        let digit = || vec![egui::Event::Text("3".into())];
        let ctx = focused(Rules::default());
        assert_eq!(frame(&ctx, digit(), "", Rules::default()).1, None);
        let rules = Rules { page_digits: true, ..Default::default() };
        assert_eq!(frame(&ctx, digit(), "", rules).1, Some(Key::Num3));
        assert_eq!(frame(&ctx, digit(), "a", rules).1, None);
    }
}
