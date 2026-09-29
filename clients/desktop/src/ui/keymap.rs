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
        "Searching and filtering (everywhere)",
        &[
            ("words", "anywhere in title, artists, album"),
            ("artist:…  album:…  title:…", "only there (artist:lil quill, no quotes)"),
            ("year:2019  genre:…  status:demo", "released, genre, status"),
            ("format:flac  flag:cover  kind:ep", "file format, a flag's kind, album kind"),
            ("disc:2  track:5", "disc and track number"),
            ("keys shorten", "art: al: st: … (any start)"),
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
            ("Ctrl+E", "change this track (a quick fix, the popup below; any mode)"),
        ],
    ),
    (
        "Marking many (any track list, edit mode)",
        &[
            ("x, Ctrl+click  or  the box", "mark or unmark one"),
            ("Shift+↑/↓  or  Shift+click", "a range, as the row it starts from (marked or not)"),
            ("Ctrl+A", "mark everything shown"),
            ("Esc", "clear the marks"),
            ("Ctrl+E", "change the marked tracks (a popup; none marked: the selected one)"),
            ("Ctrl+M", "two marked: merge them (a popup, the merge keys below)"),
            ("status:demo date:2020 disc:2 …", "in the popup: set it at once (several too), Enter"),
            ("a word, then Enter", "in the popup: find what to change (spaces don't matter)"),
            ("/", "in the popup: back to “What to change”"),
            ("Ctrl+Enter", "in the popup: preview the change, then apply it"),
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
            ("d", "mark its flags done (the chip's kind, or all)"),
            ("Ctrl+M", "compare with its possible duplicate, to merge (again: close)"),
            ("a", "ambiguous artist: credit the next candidate"),
            ("Enter in Artists", "credit the match (Shift+Enter: as featured)"),
            ("Ctrl+V", "paste an image as the cover"),
        ],
    ),
    (
        "Merging (Ctrl+M on Review, or with two tracks marked)",
        &[
            ("↑ ↓  or  j k", "a row (rows that are the same are passed over)"),
            ("← →  or  h l", "take the left or the right one"),
            ("s", "swap sides (left is kept)"),
            ("Space / Shift+Space", "play the left / right file: from the middle, the other from the same moment"),
            ("a", "merge their singles along or not (each alone on one)"),
            ("Ctrl+Enter", "merge"),
            ("n", "not the same"),
            ("Esc", "close"),
        ],
    ),
    (
        "Merging (track, album or artist page, edit mode)",
        &[
            ("Ctrl+M", "choose the other one to merge with (again: close)"),
            ("s", "swap which one stays"),
            ("a", "merge along or not: singles with their track, tracks of the same tracklist"),
            ("Ctrl+Enter", "merge"),
            ("Esc", "close"),
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
    // Three columns on a wide window, else two; groups spread so the columns come out
    // about even; and it scrolls if it still doesn't fit.
    let width = (screen.width() - 80.0).clamp(560.0, 1500.0);
    let count = if width >= 1200.0 { 3 } else { 2 };
    let columns = spread(count);
    let max_height = (screen.height() - 80.0).max(200.0);
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
                    ui.set_max_height(max_height);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Keys").font(font(24.0, bold())));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("Esc to close").color(P.faint));
                        });
                    });
                    ui.add_space(12.0);
                    egui::ScrollArea::vertical().id_salt("keys-scroll").auto_shrink([false, true]).show(ui, |ui| {
                        ui.columns(count, |uis| {
                            for (column, groups) in columns.iter().enumerate() {
                                let ui = &mut uis[column];
                                for &n in groups {
                                    let (title, bindings) = GROUPS[n];
                                    ui.label(RichText::new(title).font(font(15.0, semibold())).color(P.accent));
                                    ui.add_space(4.0);
                                    egui::Grid::new(("keys", n)).num_columns(2).spacing([18.0, 5.0]).show(ui, |ui| {
                                        for (keys, what) in bindings {
                                            ui.label(RichText::new(*keys).font(font(13.0, semibold())).color(P.text));
                                            ui.add(egui::Label::new(RichText::new(*what).font(font(13.0, medium())).color(P.muted)).wrap());
                                            ui.end_row();
                                        }
                                    });
                                    ui.add_space(14.0);
                                }
                            }
                        });
                    });
                });
        });
}

/// The groups (their places in `GROUPS`) for each of `count` columns, in order, each
/// column taking the next groups until it has about its share of the rows.
fn spread(count: usize) -> Vec<Vec<usize>> {
    // A group's height in rows: its bindings, plus its title and the space after it.
    let rows = |n: usize| GROUPS[n].1.len() + 2;
    let total: usize = (0..GROUPS.len()).map(rows).sum();
    let share = total.div_ceil(count);
    let mut columns = vec![Vec::new(); count];
    let (mut column, mut filled) = (0, 0);
    for n in 0..GROUPS.len() {
        if filled >= share && column + 1 < count {
            column += 1;
            filled = 0;
        }
        columns[column].push(n);
        filled += rows(n);
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_once_in_order_about_even() {
        for count in [2, 3] {
            let columns = spread(count);
            let flat: Vec<usize> = columns.iter().flatten().copied().collect();
            assert_eq!(flat, (0..GROUPS.len()).collect::<Vec<_>>());
            assert!(columns.iter().all(|c| !c.is_empty()), "{columns:?}");
        }
    }
}
