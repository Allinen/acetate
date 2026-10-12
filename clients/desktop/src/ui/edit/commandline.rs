//! A commandline on a page: the field where commands are typed (the language is
//! `crate::commandline`'s), the list of choices under it, its keys, what Enter would do,
//! and the commands staged, as chips. The page says what its commands mean: the choices for
//! what's being typed, and what staging and applying do. The track editor has one, and so
//! does the artist page.
//!
//! Keys in the field: ↑/↓ choose in the list, Tab takes the choice and writes the next part
//! to type, Enter takes it and stages the command(s), Backspace on an empty field takes the
//! last chip back, Esc empties it.

use eframe::egui::{self, Key, Modifiers, RichText};

use crate::ui::theme::{font, medium, PALETTE as P};

/// Where a key an empty commandline passes on to the app waits (`ctx.data`).
pub const NAV: &str = "commandline-nav";

pub struct CommandLine {
    id: egui::Id,
    pub text: String,
    /// The chosen entry of the list under it.
    pub highlighted: usize,
    /// Put the cursor in it (until it's there).
    pub focus: bool,
    /// Only if no other field has it (it wasn't asked for).
    pub gently: bool,
    /// The cursor goes after the text (it was filled in).
    pub place_cursor: bool,
}

/// The keys pressed in the field this frame (taken: nothing else sees them).
#[derive(Debug, Default, Clone, Copy)]
pub struct Pressed {
    pub up: bool,
    pub down: bool,
    pub tab: bool,
    pub enter: bool,
    /// Backspace on an empty field.
    pub back: bool,
    /// Esc with something typed (it's emptied).
    pub escape: bool,
}

impl CommandLine {
    pub fn new(id: egui::Id) -> CommandLine {
        CommandLine { id, text: String::new(), highlighted: 0, focus: false, gently: false, place_cursor: false }
    }

    pub fn id(&self) -> egui::Id {
        self.id
    }

    pub fn has_focus(&self, ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(self.id))
    }

    /// The cursor after the text, and in the field.
    pub fn cursor_to_end(&mut self, ctx: &egui::Context) {
        let mut state = egui::text_edit::TextEditState::load(ctx, self.id).unwrap_or_default();
        let end = egui::text::CCursor::new(self.text.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, self.id);
        self.focus = true;
    }

    /// The cursor asked for, until it holds (a click that opened a popup ends outside the
    /// field, which lets go of it that same frame). Returns whether the field has it.
    pub fn hold_focus(&mut self, ctx: &egui::Context) -> bool {
        if std::mem::take(&mut self.place_cursor) {
            self.cursor_to_end(ctx);
        }
        if self.focus {
            // Asked gently and another field has it (a search being typed): left there.
            let elsewhere = ctx.memory(|m| m.focused().is_some_and(|f| f != self.id));
            let holds = self.has_focus(ctx) && !ctx.input(|i| i.pointer.any_released());
            if (self.gently && elsewhere) || holds {
                self.focus = false;
                self.gently = false;
            } else {
                ctx.memory_mut(|m| m.request_focus(self.id));
            }
        }
        self.has_focus(ctx)
    }

    /// The field's keys (when it has the cursor), taken before it sees them. ↑/↓ move
    /// through `choices`; with none, ↓ is taken only if `down_too` (the page has rows below).
    /// Backspace is taken (on an empty field) only if `can_take_back`.
    pub fn keys(&mut self, ctx: &egui::Context, choices: usize, down_too: bool, can_take_back: bool) -> Pressed {
        let empty = self.text.is_empty();
        // Nothing typed yet: a digit is a page key (or the volume's), Backspace goes back
        // (unless it takes a staged change back). Passed on to the app, and not typed.
        if empty {
            const DIGITS: [Key; 10] = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
            let nav = ctx.input_mut(|i| {
                let digit = i.events.iter().find_map(|e| match e {
                    egui::Event::Text(t) if t.len() == 1 && t.as_bytes()[0].is_ascii_digit() && !i.modifiers.any() => Some(DIGITS[(t.as_bytes()[0] - b'0') as usize]),
                    _ => None,
                });
                if digit.is_some() {
                    i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t.len() == 1 && t.as_bytes()[0].is_ascii_digit()));
                    return digit;
                }
                (!can_take_back && i.consume_key(Modifiers::NONE, Key::Backspace)).then_some(Key::Backspace)
            });
            if let Some(key) = nav {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new(NAV), Some(key)));
                ctx.request_repaint();
            }
        }
        let pressed = ctx.input_mut(|i| Pressed {
            up: choices > 0 && i.consume_key(Modifiers::NONE, Key::ArrowUp),
            down: (choices > 0 || (empty && down_too)) && i.consume_key(Modifiers::NONE, Key::ArrowDown),
            tab: i.consume_key(Modifiers::NONE, Key::Tab),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            back: empty && can_take_back && i.consume_key(Modifiers::NONE, Key::Backspace),
            escape: !empty && i.consume_key(Modifiers::NONE, Key::Escape),
        });
        if pressed.up {
            self.highlighted = self.highlighted.saturating_sub(1);
        }
        if pressed.down && choices > 0 {
            self.highlighted = (self.highlighted + 1).min(choices - 1);
        }
        if pressed.escape {
            self.text.clear();
        }
        pressed
    }

    /// The field. A change starts the list from its top.
    pub fn field(&mut self, ui: &mut egui::Ui, enabled: bool, hint: &str) -> egui::Response {
        let response = ui.add_enabled(enabled, egui::TextEdit::singleline(&mut self.text).id(self.id).hint_text(hint).desired_width(f32::INFINITY));
        if response.changed() {
            self.highlighted = 0;
        }
        response
    }

    /// A choice taken for the value being typed (`value_start..end` in the text): `text`
    /// in its place (None: what's typed stays as typed), then `next`, the next part to type.
    /// The cursor goes after it.
    pub fn take(&mut self, ctx: &egui::Context, value_start: usize, end: usize, text: Option<&str>, next: Option<&str>) {
        if let Some(text) = text {
            let end = end.min(self.text.len());
            let start = value_start.min(end);
            self.text.replace_range(start..end, text);
        }
        if let Some(after) = next {
            self.text.push(' ');
            self.text.push_str(after);
        }
        self.highlighted = 0;
        self.cursor_to_end(ctx);
    }
}

/// The list under a commandline: (label, what tells it apart), the chosen one lit. A click
/// sets `clicked`. `help` says what the keys do.
pub fn list(ui: &mut egui::Ui, rows: Vec<(String, String)>, highlighted: usize, clicked: &mut Option<usize>, help: &str) {
    egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (n, (label, sub)) in rows.iter().enumerate() {
            let text = if sub.is_empty() { label.clone() } else { format!("{label}   {sub}") };
            if ui.selectable_label(n == highlighted, RichText::new(text).font(font(13.5, medium()))).clicked() {
                *clicked = Some(n);
            }
        }
        ui.label(RichText::new(help).color(P.faint).small());
    });
}

/// What Enter would do, each command: what it does (and what's worth a look), or why it
/// can't be. `nothing`: said when there are none.
pub fn would_do(ui: &mut egui::Ui, units: &[Result<(String, Option<String>), String>], nothing: &str) {
    egui::Frame::NONE.fill(P.bg).corner_radius(8).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        for u in units {
            match u {
                Ok((label, warning)) => {
                    ui.label(RichText::new(label).color(P.accent));
                    if let Some(w) = warning {
                        ui.label(RichText::new(format!("  ⚠ {w}")).color(P.text).small());
                    }
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(P.danger));
                }
            }
        }
        if units.is_empty() {
            ui.label(RichText::new(nothing).color(P.faint).small());
        }
    });
}

/// A staged command, as a chip: what was typed, what it does (its tip), and what's worth
/// a look (drawn plainer).
pub struct Chip<'a> {
    pub text: &'a str,
    pub label: &'a str,
    pub warning: Option<&'a str>,
}

/// The staged commands, each with × (if `removable`): returns the one whose × was clicked.
pub fn chips(ui: &mut egui::Ui, chips: &[Chip], removable: bool) -> Option<usize> {
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (n, c) in chips.iter().enumerate() {
            let color = if c.warning.is_some() { P.text } else { P.accent };
            egui::Frame::NONE.fill(P.surface_hi).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tip = match c.warning {
                        Some(w) => format!("{}\n⚠ {w}", c.label),
                        None => c.label.to_string(),
                    };
                    ui.label(RichText::new(c.text).font(font(13.0, medium())).color(color)).on_hover_text(tip);
                    if removable && ui.small_button("×").on_hover_text("Take it back").clicked() {
                        remove = Some(n);
                    }
                });
            });
        }
    });
    remove
}
