//! Every key, in one table: what the `?` overlay shows. (Each key is handled where it
//! acts: `ui::App::keys` for the ones that work everywhere, `widgets::move_selection`
//! for lists, the pages for their own. A key added there belongs here too.)

use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, RichText, Sense, Shadow};

use super::theme::{bold, font, medium, semibold, PALETTE as P};

/// (keys, what they do)
type Binding = (&'static str, &'static str);

pub const GROUPS: &[(&str, &[Binding])] = &[
    (
        "Anywhere",
        &[
            ("Ctrl+K", "command palette: find anything, run a command"),
            ("Ctrl+1 … 6", "albums, artists, tracks, now playing, queue, review"),
            ("1 … 6", "the same, outside a text field"),
            ("Ctrl+,", "settings"),
            ("?  or  F1", "these keys"),
            ("Alt+← / Alt+→", "back / forward"),
            ("Esc  or  Backspace", "back (a text field: leave it)"),
            ("/", "filter this list"),
            ("F5", "check the library for changes"),
            ("e", "edit mode on / off"),
        ],
    ),
    (
        "Playback",
        &[
            ("Space", "play / pause"),
            ("Ctrl+← / Ctrl+→", "previous / next track"),
            ("Shift+← / Shift+→", "back / ahead 10 seconds"),
            ("9 / 0", "volume down / up"),
            ("m", "mute"),
            ("r", "loop this track"),
            ("z", "shuffle"),
            ("Shift+S", "star what plays"),
        ],
    ),
    (
        "Lists and grids",
        &[
            ("↑ ↓ ← →  or  h j k l", "move"),
            ("Page Up / Down, Home / End", "move further"),
            ("Enter", "open, or play from here"),
            ("Shift+Enter", "play"),
            ("i", "the track's page"),
            ("s", "star"),
            ("+ / −", "rate half a star up / down"),
            ("p", "add to a playlist"),
            ("q", "queue it (at the end of the queue)"),
            ("Shift+Q", "queue it first"),
        ],
    ),
    (
        "Your playlists and the queue",
        &[
            ("Delete", "take the track out"),
            ("Ctrl+↑ / Ctrl+↓", "move it up / down"),
            ("F2", "rename the playlist"),
        ],
    ),
    (
        "Edit mode (e)",
        &[
            ("Ctrl+Enter", "save the changes (one revision each for metadata and facts)"),
            ("Esc", "discard the changes (asks first: Esc again)"),
        ],
    ),
    (
        "Review (6)",
        &[
            ("Ctrl+Enter", "save; nothing left to fix: mark it done, go to the next"),
            ("Ctrl+↓ / Ctrl+↑", "next / previous track (in a field too)"),
            ("j / k  or  ↓ ↑", "next / previous track"),
            ("Space", "play the track (or pause it)"),
            ("u", "skip it"),
            ("d", "mark it done as it is"),
            ("Enter in Artists", "credit the match (Shift+Enter: as featured)"),
            ("Ctrl+V", "paste an image as the cover"),
        ],
    ),
    (
        "Command palette",
        &[
            ("↑ ↓  or  Tab", "choose"),
            ("Enter", "open (a track: play it)"),
            ("Shift+Enter", "play (a track: open its album)"),
            (">", "only commands"),
            ("Esc", "close"),
        ],
    ),
];

/// Draws the overlay while `open`; Esc, `?`, F1 or a click outside closes it.
pub fn overlay(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    let close = ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::Escape)
            || i.consume_key(Modifiers::NONE, Key::F1)
            // "?" as typed (it's on different keys on different layouts).
            || i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"))
    });
    if close {
        *open = false;
        return;
    }
    let screen = ctx.content_rect();
    let veil = egui::Area::new(egui::Id::new("keys-veil")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(rect, 0, Color32::from_black_alpha(140));
        response.clicked()
    });
    if veil.inner {
        *open = false;
        return;
    }
    let width = (screen.width() - 120.0).clamp(560.0, 1040.0);
    egui::Area::new(egui::Id::new("keys"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(Pos2::new(screen.center().x, screen.center().y))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(P.surface)
                .corner_radius(14)
                .stroke(egui::Stroke::new(1.0, P.line))
                .shadow(Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(170) })
                .inner_margin(egui::Margin::same(24))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Keys").font(font(24.0, bold())));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("Esc to close").color(P.faint));
                        });
                    });
                    ui.add_space(12.0);
                    // Two columns of groups.
                    ui.columns(2, |columns| {
                        for (n, (title, bindings)) in GROUPS.iter().enumerate() {
                            let ui = &mut columns[if n < 2 { 0 } else { 1 }];
                            ui.label(RichText::new(*title).font(font(15.0, semibold())).color(P.accent));
                            ui.add_space(4.0);
                            egui::Grid::new(("keys", n)).num_columns(2).spacing([18.0, 5.0]).show(ui, |ui| {
                                for (keys, what) in *bindings {
                                    ui.label(RichText::new(*keys).font(font(13.0, semibold())).color(P.text));
                                    ui.label(RichText::new(*what).font(font(13.0, medium())).color(P.muted));
                                    ui.end_row();
                                }
                            });
                            ui.add_space(14.0);
                        }
                    });
                });
        });
}
