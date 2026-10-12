//! Every key, in one table: what the `?` overlay shows. (Each key is handled where it
//! acts: `ui::App::keys` for the ones that work everywhere, `widgets::move_selection`
//! for lists, the pages for their own. A key added there belongs here too.) The
//! commandlines' commands aren't here: the overlay's Commands tab lists them from their
//! declarations (`ui::commands`).

use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, RichText, Sense, Shadow};

use super::theme::{bold, font, medium, semibold, PALETTE as P};

/// (keys, what they do)
type Binding = (&'static str, &'static str);

pub const GROUPS: &[(&str, &[Binding])] = &[
    (
        "Anywhere",
        &[
            ("Ctrl+K", "command palette: find anything, run a command"),
            ("Ctrl+1 … 7", "albums, artists, tracks, now playing, queue, review, liked songs"),
            ("digit  /  Backspace", "first in an empty commandline: the same as outside it (pages, volume)  /  back to the page before"),
            ("1 … 7", "the same, outside a text field"),
            ("Ctrl+,", "settings (h / l or the arrows: next tab)"),
            ("Ctrl+L", "copy a link to the track, album or artist shown (for someone with an account)"),
            ("Ctrl+Shift+L", "share link popup for the track or album shown (anyone can open it)"),
            ("right-click a track or album", "Share (Shift+click: share link popup)"),
            ("?  or  F1", "these keys"),
            ("Alt+← / Alt+→", "back / forward"),
            ("Esc  or  Backspace", "back (a text field: leave it)"),
            ("/", "filter this list"),
            ("Alt+M", "earlier messages (the toasts that went by)"),
            ("F5", "check the library for changes"),
            ("e", "edit mode on / off"),
            ("Alt+E", "edit mode on / off, in a text field too (a commandline)"),
        ],
    ),
    (
        "Searching and filtering (everywhere)",
        &[
            ("words", "anywhere in title, artists, album"),
            ("key:value", "only there: artist:, year:, status:… (the Commands tab has them all)"),
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
            ("Shift+P", "take it out of a playlist (asks which of yours has it; on a playlist's page: out of it)"),
            ("q", "queue it (at the end of the queue)"),
            ("Shift+Q", "queue it first"),
            ("Ctrl+E", "change this track (a quick fix, the popup below; any mode)"),
        ],
    ),
    (
        "Marking many (any track list, any mode)",
        &[
            ("x, Ctrl+click  or  the box", "mark or unmark one"),
            ("Shift+↑/↓  or  Shift+click", "mark a range, from the row the cursor is on (from a row just unmarked: unmark)"),
            ("Ctrl+A", "mark everything shown"),
            ("Esc", "clear the marks"),
            ("s  /  + −  /  p, Shift+P  /  q, Shift+Q", "star (all starred: unstar)  /  rate  /  add to, take out of a playlist  /  queue the marked tracks"),
            ("Ctrl+E", "change the marked tracks (the editor, below; none marked: the selected one)"),
            ("Ctrl+M", "edit mode, two marked: merge them (a popup, the merge keys below)"),
            ("Ctrl+B", "edit mode: credited as… for the marked tracks (none marked: the selected one); its popup has a commandline: artist: only: to: as: why:, | for another, Ctrl+Enter credits them so"),
        ],
    ),
    (
        "The track editor (a track's page in edit mode, Review, Ctrl+E)",
        &[
            ("artist:Ann role:featured | status:demo", "what to change, typed (several at once: | between them): Enter stages it"),
            ("-genre:rap  /  -artist", "take one off  /  clear them all"),
            ("Tab", "take the choice listed and leave a space: the list then shows what can follow (role:, as:, n:…)"),
            ("Backspace (empty commandline)", "take the last change back to edit"),
            ("a word", "find what to change, or a tool (merge two, credited as…; a track's page: its actions)"),
            ("↓ (empty commandline, one track, or an album in edit mode)", "into the fields: Enter edits one, Delete takes it off"),
            ("r / a / m / o  on a field", "an artist's role / credited as · a cover made main · the file came from a source"),
            ("Alt+↑ / Alt+↓  on a field", "move an artist or a cover"),
            ("Ctrl+V", "an image (or an image file): a new cover"),
            ("Ctrl+Enter", "apply the preview"),
            ("Esc", "clear the commandline; then discard the changes (asks first); once applied: close"),
            ("/", "back to the commandline"),
        ],
    ),
    (
        "Your playlists and the queue",
        &[
            ("Delete  or  Shift+P", "take the track out (marked tracks: all of them); it stays, faded, until the page is opened again, and p puts it back where it was"),
            ("Ctrl+↑ / Ctrl+↓", "move it up / down"),
            ("F2", "rename the playlist"),
        ],
    ),
    (
        "Edit mode (e)",
        &[
            ("Enter  or  Ctrl+E", "a track's page: the cursor to its editor's commandline"),
            ("Ctrl+Enter", "apply the editor's changes; an album's or artist's changes or form: save them"),
            ("Ctrl+Enter in the lyrics", "save the lyrics (their own revision)"),
            ("Alt+↑ / Alt+↓  or  Alt+K / Alt+J", "an album's tracks: move the selected one up / down (on its disc; Ctrl+Enter saves the new order)"),
            ("Esc", "discard the changes (asks first: Esc again)"),
        ],
    ),
    (
        "An artist's page",
        &[
            ("/", "its commandline (it has the cursor when the page opens; | between commands; a word finds the rest, edit too)"),
            ("Ctrl+Enter", "save the changes staged (each kind on its own)"),
            ("Backspace (empty commandline)", "take the last change back to edit"),
            ("commands", "alias:, og:, name:, source:, extra:, note:… (the Commands tab)"),
        ],
    ),
    (
        "A track's page",
        &[
            ("/", "its commandline: the editor's (listen mode: the line alone; a word finds the rest, edit too)"),
            ("Ctrl+Enter", "apply the editor's changes and save the page's (each on its own)"),
            ("commands", "play:, star:, lyrics:, file:, version:… (the Commands tab)"),
        ],
    ),
    (
        "An album's page",
        &[
            ("/", "its commandline (it has the cursor when the page opens; | between commands; a word finds the rest, edit too)"),
            ("Ctrl+Enter", "save the changes staged (the album's own as one revision)"),
            ("commands", "play:, tracks:, artist:, cover:, move:… (the Commands tab)"),
        ],
    ),
    (
        "Notes and extras",
        &[
            ("Ctrl+Enter in a note", "save it"),
            ("Esc  /  Esc again", "leave the note's text  /  throw it away (asked first)"),
            ("↑ ↓  or  j k  (an extra)", "what it belongs to, then its files"),
            ("Enter  /  d", "open the link or play the file  /  save the file"),
            ("Delete (edit mode)", "take the file off, or unlink (both kept)"),
        ],
    ),
    (
        "Review (6)",
        &[
            ("Ctrl+Enter", "apply; nothing left to fix: mark it done, go to the next"),
            ("Ctrl+↓ / Ctrl+↑", "next / previous track (in a field too)"),
            ("j / k  or  ↓ ↑", "next / previous track"),
            ("Space", "play the track (or pause it)"),
            ("u", "skip it"),
            ("d", "mark its flags done (the chip's kind, or all)"),
            ("Ctrl+M", "compare with its possible duplicate, to merge (again: close)"),
            ("c  on the artist", "ambiguous artist: credit the next candidate"),
            ("Ctrl+V", "paste an image: a new cover (a change, applied with the rest)"),
        ],
    ),
    (
        "Merging (Ctrl+M on Review, or with two tracks marked)",
        &[
            ("↑ ↓  or  j k", "a row (rows that are the same are passed over)"),
            ("← →  or  h l", "take the left or the right one"),
            ("s", "take everything from the right, again for the left (sides stay)"),
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
        "Timing lyrics (Sync… in the lyrics, edit mode: a popup)",
        &[
            ("Enter", "this line starts now (then the next; headings are passed over)"),
            ("← / →", "its time 0.1 s earlier / later"),
            ("Delete", "no time for it"),
            ("g", "play from just before it"),
            ("↑ ↓  or  j k", "another line"),
            ("Space", "pause (or start the track)"),
            ("Shift+← / →", "5 s back / on"),
            ("F2  or  double click", "edit its words (Enter keeps them, Esc doesn't)"),
            ("o", "a new line after it"),
            ("h", "a heading before it: Verse 1: Wrenfield (no time needed)"),
            ("Shift+Delete", "remove it (the marked lines, if any)"),
            ("x, Ctrl+click", "mark it or not"),
            ("Shift+↑ ↓  or  Shift+click", "mark a range"),
            ("l", "its language, or the marked lines' (with “A language per line”)"),
            ("Ctrl+Z", "undo the last change"),
            ("Ctrl+Enter", "save the lyrics"),
            ("Esc", "back to the text"),
        ],
    ),
    (
        "Command palette",
        &[
            ("↑ ↓", "choose"),
            ("Tab", "write the key or command in full"),
            ("Enter", "open (a track: play it)"),
            ("Shift+Enter", "play (a track: open its album)"),
            ("go:  do:", "pages  /  other commands (the Commands tab)"),
            ("Esc", "empty the line, then close"),
        ],
    ),
];

/// Draws the overlay while `open`; Esc, `?`, F1 or a click outside closes it. Two tabs: the
/// keys, and the commandlines' commands (Tab, or a click, goes to the other).
pub fn overlay(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    let tab_id = egui::Id::new("keys-tab-commands");
    let mut commands = ctx.data(|d| d.get_temp::<bool>(tab_id)).unwrap_or(false);
    let (close, switch) = ctx.input_mut(|i| {
        let close = i.consume_key(Modifiers::NONE, Key::Escape)
            || i.consume_key(Modifiers::NONE, Key::F1)
            // "?" as typed (it's on different keys on different layouts).
            || i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "?"));
        (close, i.consume_key(Modifiers::NONE, Key::Tab))
    });
    if close {
        *open = false;
        return;
    }
    commands ^= switch;
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
    // What's shown: (title, a line under it, rows), as groups.
    let lines = super::commands::all();
    let groups: Vec<(String, String, Vec<(String, String)>)> = if commands {
        lines
            .iter()
            .map(|c| {
                let entries = c.schema.entries();
                let prefix = super::commands::all_prefix(&entries);
                let place = if prefix { format!("{} · a key by any start of its names", c.place) } else { c.place.to_string() };
                (c.title.to_string(), place, entries.iter().map(|e| (super::commands::keys(e), super::commands::what(e, !prefix))).collect())
            })
            .collect()
    } else {
        GROUPS.iter().map(|(title, bindings)| (title.to_string(), String::new(), bindings.iter().map(|(k, w)| (k.to_string(), w.to_string())).collect())).collect()
    };
    // Three columns on a wide window, else two; groups spread so the columns come out
    // about even; and it scrolls if it still doesn't fit.
    let width = (screen.width() - 80.0).clamp(560.0, 1500.0);
    let count = if width >= 1200.0 { 3 } else { 2 };
    let sizes: Vec<usize> = groups.iter().map(|g| g.2.len() + if g.1.is_empty() { 2 } else { 3 }).collect();
    let columns = spread(&sizes, count);
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
                        for (title, is_commands) in [("Keys", false), ("Commands", true)] {
                            let color = if commands == is_commands { P.text } else { P.faint };
                            if ui.add(egui::Label::new(RichText::new(title).font(font(24.0, bold())).color(color)).sense(Sense::click())).clicked() {
                                commands = is_commands;
                            }
                            ui.add_space(12.0);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("Tab: the other tab · Esc to close").color(P.faint));
                        });
                    });
                    ui.add_space(12.0);
                    egui::ScrollArea::vertical().id_salt(("keys-scroll", commands)).auto_shrink([false, true]).show(ui, |ui| {
                        ui.columns(count, |uis| {
                            for (column, members) in columns.iter().enumerate() {
                                let ui = &mut uis[column];
                                for &n in members {
                                    let (title, place, rows) = &groups[n];
                                    ui.label(RichText::new(title).font(font(15.0, semibold())).color(P.accent));
                                    if !place.is_empty() {
                                        ui.add(egui::Label::new(RichText::new(place).font(font(12.5, medium())).color(P.faint)).wrap());
                                    }
                                    ui.add_space(4.0);
                                    egui::Grid::new(("keys", commands, n)).num_columns(2).spacing([18.0, 5.0]).show(ui, |ui| {
                                        for (keys, what) in rows {
                                            ui.label(RichText::new(keys).font(font(13.0, semibold())).color(P.text));
                                            ui.add(egui::Label::new(RichText::new(what).font(font(13.0, medium())).color(P.muted)).wrap());
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
    ctx.data_mut(|d| d.insert_temp(tab_id, commands));
}

/// The groups (their places in `sizes`, each a group's height in rows) for each of `count`
/// columns, in order, each column taking the next groups until it has about its share.
fn spread(sizes: &[usize], count: usize) -> Vec<Vec<usize>> {
    let total: usize = sizes.iter().sum();
    let share = total.div_ceil(count);
    let mut columns = vec![Vec::new(); count];
    let (mut column, mut filled) = (0, 0);
    for (n, size) in sizes.iter().enumerate() {
        if filled >= share && column + 1 < count {
            column += 1;
            filled = 0;
        }
        columns[column].push(n);
        filled += size;
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_group_once_in_order_about_even() {
        let sizes: Vec<usize> = GROUPS.iter().map(|g| g.1.len() + 2).collect();
        for count in [2, 3] {
            let columns = spread(&sizes, count);
            let flat: Vec<usize> = columns.iter().flatten().copied().collect();
            assert_eq!(flat, (0..GROUPS.len()).collect::<Vec<_>>());
            assert!(columns.iter().all(|c| !c.is_empty()), "{columns:?}");
        }
    }
}
