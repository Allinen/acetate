//! The command palette (Ctrl+K, anywhere): type to find any artist, album or track
//! (fuzzy: letters in order, words anywhere) or a command. ↑/↓ choose, Enter opens (a
//! track: plays it, then the rest of its album), Shift+Enter plays (a track: opens its
//! album). Esc empties the line, and closes it when it's empty.
//!
//! The line is a commandline (`crate::commandline::schema`'s language): the search's keys
//! (`artist:` `album:` `year:` `status:`…) filter, `go:albums` lists the pages and `do:pause`
//! the other commands (a command's short word is shown under its name). Plain words find
//! commands too, and the keys themselves are listed (Tab or Enter writes one and carries on
//! typing). Tab completes the highlighted key or command to its `go:` / `do:` form; on
//! anything else it moves down.
//!
//! It also asks for things: which playlist to add tracks to (`p` on anything; typing a
//! new name offers to make that playlist), and a playlist's name (new, or a rename).
//!
//! The library search runs on a background thread (it can take tens of milliseconds on
//! a big library), so typing never waits: the list shows the last answer until the
//! next one arrives, and answers to older queries are dropped.


use std::sync::Arc;
use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, Rect, RichText, Sense, Shadow, Vec2};

use uuid::Uuid;

use crate::catalog::search::{Hits, Search};
use crate::commandline::schema::{self, Kind, Schema, Spec};
use crate::catalog::Catalog;
use crate::ui::task::Task;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::edit::PickKind;
use crate::ui::{widgets, Action, Page, Shell};

const INPUT_ID: &str = "palette-input";
const ROW: f32 = 44.0;

/// A command: its name, a short word to type for it (`go:albums`, `do:pause`), whether it
/// goes to a page (`go:`; else `do:`), its key (shown as a hint), and what it does.
struct Command {
    name: &'static str,
    word: &'static str,
    page: bool,
    keys: &'static str,
    action: fn() -> Action,
}

const COMMANDS: &[Command] = &[
    Command { name: "Go to Albums", word: "albums", page: true, keys: "1", action: || Action::Go(Page::Albums) },
    Command { name: "Go to Artists", word: "artists", page: true, keys: "2", action: || Action::Go(Page::Artists) },
    Command { name: "Go to Tracks", word: "tracks", page: true, keys: "3", action: || Action::Go(Page::Tracks) },
    Command { name: "Go to Now playing", word: "playing", page: true, keys: "4", action: || Action::Go(Page::NowPlaying) },
    Command { name: "Go to the queue", word: "queue", page: true, keys: "5", action: || Action::Go(Page::Queue) },
    Command { name: "Go to Review (fix the library)", word: "review", page: true, keys: "6", action: || Action::Go(Page::Review) },
    Command { name: "Play / pause", word: "pause", page: false, keys: "Space", action: || Action::TogglePause },
    Command { name: "Next track", word: "next", page: false, keys: "Ctrl+→", action: || Action::Next },
    Command { name: "Previous track", word: "previous", page: false, keys: "Ctrl+←", action: || Action::Previous },
    Command { name: "Loop this track (repeat) on / off", word: "repeat", page: false, keys: "r", action: || Action::ToggleRepeat },
    Command { name: "Shuffle on / off", word: "shuffle", page: false, keys: "z", action: || Action::ToggleShuffle },
    Command { name: "Check the library for changes", word: "refresh", page: false, keys: "F5", action: || Action::RefreshLibrary },
    Command { name: "Back", word: "back", page: false, keys: "Alt+←", action: || Action::Back },
    Command { name: "Forward", word: "forward", page: false, keys: "Alt+→", action: || Action::Forward },
    Command { name: "Go to Liked songs", word: "liked", page: true, keys: "", action: || Action::Go(Page::Liked) },
    Command { name: "Go to collectives & groupings", word: "groupings", page: true, keys: "", action: || Action::Go(Page::Groupings) },
    Command { name: "New playlist…", word: "playlist", page: false, keys: "", action: || Action::NewPlaylist },
    Command { name: "Settings", word: "settings", page: true, keys: "Ctrl+,", action: || Action::Go(Page::Settings) },
    Command { name: "Copy a link to this page (to send)", word: "link", page: false, keys: "Ctrl+L", action: || Action::CopyPageLink },
    Command { name: "Make a share link to this page (anyone can open it; options first)", word: "share", page: false, keys: "Ctrl+Shift+L", action: || Action::CopyPageShareLink },
    Command { name: "Trash (take things back out, clean up old files)", word: "trash", page: true, keys: "", action: || Action::Go(Page::Trash) },
    Command { name: "Changes (what was changed lately in the metadata)", word: "changes", page: true, keys: "", action: || Action::Go(Page::Changes) },
    Command { name: "Edit mode on / off", word: "edit", page: false, keys: "e", action: || Action::ToggleEditMode },
    Command { name: "Show every key", word: "keys", page: false, keys: "?", action: || Action::ShowKeys },
    Command { name: "Earlier messages", word: "messages", page: false, keys: "Alt+M", action: || Action::ShowMessages },
    Command { name: "Mute / unmute", word: "mute", page: false, keys: "m", action: || Action::ToggleMute },
    Command { name: "Log out", word: "logout", page: false, keys: "", action: || Action::LogOut },
];

/// What the palette is open for.
#[derive(Default, Clone)]
pub enum Mode {
    /// Finding anything, and commands.
    #[default]
    Search,
    /// Choosing the playlist to add these tracks to.
    AddTo(Vec<Uuid>),
    /// Choosing which of your playlists to take these tracks out of.
    RemoveFrom(Vec<Uuid>),
    /// A name for a new playlist (None) or for this one.
    Name(Option<Uuid>),
    /// Choosing an artist, album or track for an edit form (or making a new artist or
    /// album by that name). Not the one given (what asks: a track isn't merged with
    /// itself).
    Pick(PickKind, Option<Uuid>),
}

/// The palette's line: the search's keys (they go with each other, as in a track list), and
/// `go:` and `do:`, which take a command's word.
pub(crate) static PALETTE: Schema = Schema { commands: &crate::catalog::query::KEYS, also: GO_DO };

/// Its own: `go:` and `do:` (for the help: the search's are described with searching).
const GO_DO: &[Spec] = &[Spec::new("go", Kind::Text, "go to a page").aliases(&["goto"]), Spec::new("do", Kind::Text, "run a command").aliases(&["run"])];
pub(crate) static PALETTE_OWN: Schema = Schema { commands: GO_DO, also: &[] };

/// Keys listed when nothing (or the start of one) is typed: what to type, and what it's for.
const INSERTS: &[(&str, &str)] = &[
    ("go:", "go to a page"),
    ("do:", "run a command"),
    ("artist:", "find an artist"),
    ("album:", "find an album"),
    ("year:", "tracks and albums from a year"),
    ("status:", "tracks by their status"),
];

/// `go:…` / `do:…` at the start of the line: whether it's pages, and the words after it.
fn command_line(query: &str) -> Option<(bool, String)> {
    let line = schema::parse(&PALETTE, query);
    let first = line.commands.first()?;
    match first.spec.key {
        "go" => Some((true, first.value.clone())),
        "do" => Some((false, first.value.clone())),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Entry {
    Command(usize),
    /// A key to type (its place in `INSERTS`).
    Insert(usize),
    Artist(u32),
    Album(u32),
    Track(u32),
    /// A grouping (collective, era, project…): its place in the catalog's list.
    Grouping(u32),
    /// An extra (vocals, stems…): its place in the catalog's list.
    Extra(u32),
    Playlist(Uuid),
    Liked,
    /// Make a playlist named what was typed (or, when naming, use that name).
    Create,
}

/// A playlist as the palette lists it: id, name, whether it's yours, its size.
type PlaylistRow = (Uuid, String, bool, i64);

#[derive(Default)]
pub struct Palette {
    open: bool,
    mode: Mode,
    /// A rename's field was filled with the current name.
    prefilled: bool,
    query: String,
    selected: usize,
    entries: Vec<Entry>,
    /// Numbers each search, so an answer to an older query is dropped.
    asked: u64,
    /// The latest answer from the background: (its number, the hits).
    answer: Task<(u64, Hits)>,
    /// The library hits shown (from the latest answer).
    hits: Hits,
    /// The catalog generation the hits are for.
    made_for: Option<u64>,
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self, ctx: &egui::Context) {
        if self.open {
            self.open = false;
        } else {
            self.open(ctx, Mode::Search);
        }
    }

    pub fn open(&mut self, ctx: &egui::Context, mode: Mode) {
        self.open = true;
        self.mode = mode;
        self.prefilled = false;
        self.query.clear();
        self.selected = 0;
        self.hits = Hits::default();
        self.made_for = None;
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(INPUT_ID)));
    }

    pub fn show(&mut self, ctx: &egui::Context, shell: &mut Shell) {
        if !self.open {
            return;
        }
        let catalog = shell.catalog.value.clone();
        let mut playlists: Vec<PlaylistRow> =
            shell.playlists.list(catalog.as_deref()).iter().map(|p| (p.id, p.name.clone(), p.mine, p.entries)).collect();
        // Taking tracks out: only the playlists that have some of them.
        if let Mode::RemoveFrom(tracks) = &self.mode {
            let have: Vec<Uuid> = shell.playlists.containing(catalog.as_deref(), tracks).into_iter().map(|(id, _)| id).collect();
            playlists.retain(|p| have.contains(&p.0));
        }
        if let (Mode::Name(Some(id)), false) = (&self.mode, self.prefilled) {
            self.prefilled = true;
            if let Some(p) = playlists.iter().find(|p| p.0 == *id) {
                self.query = p.1.clone();
            }
        }
        if let Some((number, hits)) = self.answer.poll()
            && number == self.asked
        {
            self.hits = hits;
            self.selected = 0;
        }

        // Keys for the list, taken before the text field sees them.
        let (up, down, tab, enter, shift_enter, escape) = ctx.input_mut(|i| {
            // Shift+Enter first: a plain Enter may also match with Shift held.
            let shift_enter = i.consume_key(Modifiers::SHIFT, Key::Enter);
            let enter = i.consume_key(Modifiers::NONE, Key::Enter);
            let up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
            let down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
            (up, down, i.consume_key(Modifiers::NONE, Key::Tab), enter, shift_enter, i.consume_key(Modifiers::NONE, Key::Escape))
        });
        if escape {
            // Esc empties the line first, as on a page's commandline.
            if matches!(self.mode, Mode::Search) && !self.query.is_empty() {
                self.write(ctx, String::new());
            } else {
                self.open = false;
                return;
            }
        }
        // Tab writes a key or command in full; on anything else it moves down.
        let completes = tab.then(|| self.entries.get(self.selected).and_then(|e| completion(*e))).flatten();
        if let Some(text) = completes.clone() {
            self.write(ctx, text);
        }
        let down = down || (tab && completes.is_none());

        // A dim veil over the app; a click on it closes the palette.
        let screen = ctx.content_rect();
        let veil = egui::Area::new(egui::Id::new("palette-veil")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0, Color32::from_black_alpha(110));
            response.clicked()
        });
        if veil.inner {
            self.open = false;
            return;
        }

        let width = (screen.width() * 0.5).clamp(480.0, 720.0);
        let top = screen.top() + screen.height() * 0.14;
        let area = egui::Area::new(egui::Id::new("palette")).order(egui::Order::Tooltip).fixed_pos(Pos2::new(screen.center().x - width / 2.0, top));
        area.show(ctx, |ui| {
            let frame = egui::Frame::NONE
                .fill(P.surface)
                .corner_radius(14)
                .stroke(egui::Stroke::new(1.0, P.line))
                .shadow(Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(170) })
                .inner_margin(egui::Margin::same(10));
            frame.show(ui, |ui| {
                ui.set_width(width - 20.0);
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new(INPUT_ID))
                        .hint_text(match &self.mode {
                            Mode::Search => "Search anything (artist:… album:… year:… status:…), go: a page, do: a command".to_string(),
                            Mode::AddTo(tracks) if tracks.len() == 1 => "Add the track to which playlist? (or type a new name)".to_string(),
                            Mode::AddTo(tracks) => format!("Add the {} tracks to which playlist? (or type a new name)", tracks.len()),
                            Mode::RemoveFrom(tracks) if tracks.len() == 1 => "Take the track out of which playlist?".to_string(),
                            Mode::RemoveFrom(tracks) => format!("Take the {} tracks out of which playlist?", tracks.len()),
                            Mode::Name(None) => "The new playlist's name".to_string(),
                            Mode::Name(Some(_)) => "The playlist's new name".to_string(),
                            Mode::Pick(PickKind::Artist, _) => "Which artist? (or type a new one's name)".to_string(),
                            Mode::Pick(PickKind::Album, _) => "Which album? (or type a new one's title)".to_string(),
                            Mode::Pick(PickKind::Track, _) => "Which track?".to_string(),
                        })
                        .font(font(16.0, medium()))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(8, 8)),
                );
                if !input.has_focus() {
                    input.request_focus();
                }
                let generation = Some(shell.generation);
                if matches!(self.mode, Mode::Search | Mode::Pick(..)) && (input.changed() || self.made_for != generation) {
                    self.made_for = generation;
                    self.ask(shell, catalog.clone());
                }
                self.entries = self.entries(catalog.as_deref(), &playlists);
                if self.entries.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("  Nothing found").color(P.faint));
                    ui.add_space(6.0);
                    return;
                }
                if down {
                    self.selected = (self.selected + 1).min(self.entries.len() - 1);
                }
                if up {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.selected = self.selected.min(self.entries.len() - 1);
                ui.add_space(4.0);
                ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, P.line));
                ui.add_space(6.0);

                let mut chosen = None;
                // The height from the number of entries, every frame: left to itself the
                // list keeps the height of its smallest answer.
                let height = (self.entries.len() as f32 * ROW).min(ROW * 10.0);
                egui::ScrollArea::vertical()
                    .min_scrolled_height(height)
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (n, entry) in self.entries.clone().into_iter().enumerate() {
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                        if n == self.selected {
                            ui.painter().rect_filled(rect, 8, P.surface_hi);
                            if down || up {
                                ui.scroll_to_rect(rect, None);
                            }
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, 8, P.bg);
                        }
                        self.row(ui, shell, catalog.as_deref(), &playlists, entry, rect);
                        if response.clicked() {
                            chosen = Some((entry, ui.input(|i| i.modifiers.shift)));
                        }
                    }
                });
                if enter || shift_enter {
                    chosen = Some((self.entries[self.selected], shift_enter));
                }
                ui.add_space(4.0);
                ui.label(RichText::new("↑↓ choose · Enter opens · Shift+Enter plays · Tab completes · Esc empties").color(P.faint).small());
                // A key is written into the line, and the typing goes on.
                if let Some((Entry::Insert(i), _)) = chosen {
                    self.write(ctx, INSERTS[i].0.to_string());
                    return;
                }
                if let Some((entry, shift)) = chosen {
                    self.open = false;
                    let name = playlists.iter().find(|p| Entry::Playlist(p.0) == entry).map(|p| p.1.clone()).unwrap_or_default();
                    run(shell, catalog.as_deref(), &self.mode, self.query.trim(), entry, &name, shift);
                }
            });
        });
    }

    /// Replaces the line with `text`, the cursor after it, and asks the library again.
    fn write(&mut self, ctx: &egui::Context, text: String) {
        let id = egui::Id::new(INPUT_ID);
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        let end = egui::text::CCursor::new(text.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, id);
        self.query = text;
        self.selected = 0;
        self.made_for = None;
        ctx.memory_mut(|m| m.request_focus(id));
    }

    /// Starts a library search for the current query in the background.
    fn ask(&mut self, shell: &mut Shell, catalog: Option<Arc<Catalog>>) {
        self.asked += 1;
        let query = self.query.trim().to_string();
        if query.is_empty() || command_line(&query).is_some() {
            self.hits = Hits::default();
            return;
        }
        let Some(catalog) = catalog else { return };
        let number = self.asked;
        let limit = if matches!(self.mode, Mode::Pick(..)) { 12 } else { 8 };
        // (A newer query replaces this one's answer.)
        self.answer.start(shell, async move {
            let hits = tokio::task::spawn_blocking(move || Search::default().run_query(&catalog, &query, limit)).await.unwrap_or_default();
            (number, hits)
        });
    }

    /// What's listed, for the mode: library hits, playlists and commands; your
    /// playlists (and making one) to add to; or just the name.
    fn entries(&self, catalog: Option<&Catalog>, playlists: &[PlaylistRow]) -> Vec<Entry> {
        let query = self.query.trim();
        let command = command_line(query);
        let typed = command.as_ref().map(|c| c.1.as_str()).unwrap_or(query);
        let words: Vec<String> = typed.to_lowercase().split_whitespace().map(str::to_string).collect();
        let matches = |name: &str| {
            let name = name.to_lowercase();
            words.iter().all(|w| name.contains(w.as_str()))
        };
        let mut entries = Vec::new();
        match &self.mode {
            Mode::Search => {
                let commands_only = command.is_some();
                if !commands_only && catalog.is_some() {
                    entries.extend(self.hits.artists.iter().take(4).map(|h| Entry::Artist(h.0)));
                    entries.extend(self.hits.albums.iter().take(6).map(|h| Entry::Album(h.0)));
                    entries.extend(self.hits.tracks.iter().take(8).map(|h| Entry::Track(h.0)));
                    entries.extend(self.hits.extras.iter().take(4).map(|h| Entry::Extra(h.0)));
                }
                if !commands_only && !words.is_empty() {
                    if matches("liked songs") {
                        entries.push(Entry::Liked);
                    }
                    entries.extend(playlists.iter().filter(|p| matches(&p.1)).take(5).map(|p| Entry::Playlist(p.0)));
                    if let Some(c) = catalog {
                        entries.extend(c.groupings.iter().enumerate().filter(|(_, g)| matches(&g.name)).take(4).map(|(i, _)| Entry::Grouping(i as u32)));
                    }
                }
                // The keys, until something else is typed (or a start of one).
                if !commands_only && !query.contains(':') && words.len() <= 1 {
                    let start = words.first().map(String::as_str).unwrap_or("");
                    entries.extend(INSERTS.iter().enumerate().filter(|(_, ins)| ins.0.starts_with(start)).map(|(i, _)| Entry::Insert(i)));
                }
                for (i, c) in COMMANDS.iter().enumerate() {
                    if command.as_ref().is_none_or(|(page, _)| *page == c.page) && (matches(c.name) || matches(c.word)) {
                        entries.push(Entry::Command(i));
                    }
                }
            }
            Mode::AddTo(_) => {
                entries.extend(playlists.iter().filter(|p| p.2 && matches(&p.1)).map(|p| Entry::Playlist(p.0)));
                let exists = playlists.iter().any(|p| p.2 && p.1.eq_ignore_ascii_case(query));
                if !query.is_empty() && !exists {
                    entries.push(Entry::Create);
                }
            }
            Mode::RemoveFrom(_) => entries.extend(playlists.iter().filter(|p| matches(&p.1)).map(|p| Entry::Playlist(p.0))),
            Mode::Name(_) => {
                if !query.is_empty() {
                    entries.push(Entry::Create);
                }
            }
            Mode::Pick(kind, except) => {
                if let Some(c) = catalog {
                    let not = |id: Uuid| Some(id) != *except;
                    match kind {
                        PickKind::Artist => entries.extend(self.hits.artists.iter().filter(|h| not(c.artists[h.0 as usize].id)).map(|h| Entry::Artist(h.0))),
                        PickKind::Album => entries.extend(self.hits.albums.iter().filter(|h| not(c.album_at(h.0).id())).map(|h| Entry::Album(h.0))),
                        PickKind::Track => entries.extend(self.hits.tracks.iter().filter(|h| not(c.track_at(h.0).id())).map(|h| Entry::Track(h.0))),
                    }
                }
                // A new one, even when one by that name exists: two artists can share a
                // name (the list shows the existing ones first). Not when picking the
                // other one of a pair (what to merge into).
                if !query.is_empty() && *kind != PickKind::Track && except.is_none() {
                    entries.push(Entry::Create);
                }
            }
        }
        entries
    }

    fn row(&self, ui: &mut egui::Ui, shell: &mut Shell, catalog: Option<&Catalog>, playlists: &[PlaylistRow], entry: Entry, rect: Rect) {
        let painter = ui.painter().clone();
        let cover_rect = Rect::from_center_size(rect.left_center() + Vec2::new(24.0, 0.0), Vec2::splat(32.0));
        let text_left = rect.left() + 50.0;
        let width = rect.width() - 50.0 - 90.0;
        // An original file's track: the OG badge after its title.
        let og = matches!((entry, catalog), (Entry::Track(i), Some(c)) if c.track_at(i).original_filename().is_some());
        let (title, sub, kind) = match (entry, catalog) {
            (Entry::Playlist(id), _) => {
                let Some(p) = playlists.iter().find(|p| p.0 == id) else { return };
                playlist_icon(&painter, cover_rect.center());
                let n = crate::ui::count(p.3 as usize, "track");
                (p.1.clone(), Some(n), "Playlist".into())
            }
            (Entry::Liked, _) => {
                widgets::draw_star(&painter, cover_rect.center(), 10.0, P.accent);
                ("Liked songs".into(), Some("your starred tracks".into()), "Playlist".into())
            }
            (Entry::Create, _) => {
                painter.text(cover_rect.center(), Align2::CENTER_CENTER, "+", font(20.0, semibold()), P.accent);
                let name = self.query.trim();
                let title = match &self.mode {
                    Mode::Name(Some(_)) => format!("Rename to “{name}”"),
                    Mode::Pick(PickKind::Artist, _) => format!("New artist “{name}”"),
                    Mode::Pick(..) => format!("New album “{name}”"),
                    _ => format!("New playlist “{name}”"),
                };
                (title, None, "Enter".into())
            }
            (Entry::Command(i), _) => {
                painter.text(cover_rect.center(), Align2::CENTER_CENTER, "›", font(18.0, semibold()), P.accent);
                (COMMANDS[i].name.to_string(), completion(entry), COMMANDS[i].keys.to_string())
            }
            (Entry::Insert(i), _) => {
                painter.text(cover_rect.center(), Align2::CENTER_CENTER, ":", font(18.0, semibold()), P.accent);
                (INSERTS[i].0.to_string(), Some(INSERTS[i].1.to_string()), "Tab".into())
            }
            (Entry::Artist(i), Some(c)) => {
                let artist = &c.artists[i as usize];
                let hue = artist.id.as_bytes()[15] as f32 / 255.0;
                painter.circle_filled(cover_rect.center(), 16.0, Color32::from(egui::ecolor::Hsva::new(hue, 0.35, 0.3, 1.0)));
                let initial = artist.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                painter.text(cover_rect.center(), Align2::CENTER_CENTER, initial, font(13.0, medium()), P.text);
                let aliases = (!artist.aliases.is_empty()).then(|| format!("also {}", artist.aliases.join(", ")));
                (artist.label(), aliases, "Artist".into())
            }
            (Entry::Album(i), Some(c)) => {
                let album = c.album_at(i);
                shell.cover(ui, album.cover(), album.id(), cover_rect, 5, 0.0);
                let sub = match album.year() {
                    Some(year) => format!("{} · {year}", album.artist_display()),
                    None => album.artist_display().to_string(),
                };
                (album.title().to_string(), Some(sub), capitalized(album.kind()))
            }
            (Entry::Grouping(i), Some(c)) => {
                let grouping = &c.groupings[i as usize];
                painter.rect_filled(cover_rect.shrink(4.0), 6, P.accent_soft);
                let initial = grouping.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                painter.text(cover_rect.center(), Align2::CENTER_CENTER, initial, font(13.0, medium()), P.text);
                let sub = format!("{} tracks · {} releases", grouping.track_count, grouping.album_count);
                (grouping.name.clone(), Some(sub), capitalized(&grouping.kind))
            }
            (Entry::Extra(i), Some(c)) => {
                let extra = &c.extras[i as usize];
                // A video: its thumbnail; else a note.
                match extra.thumb() {
                    Some(thumb) => shell.cover(ui, Some(thumb), extra.id, cover_rect, 5, 0.0),
                    None => {
                        painter.rect_stroke(cover_rect.shrink(4.0), 6, egui::Stroke::new(1.5, P.accent), egui::StrokeKind::Inside);
                        painter.text(cover_rect.center(), Align2::CENTER_CENTER, if extra.video.is_some() { "▶" } else { "♪" }, font(15.0, medium()), P.accent);
                    }
                }
                // What it belongs to (the first thing, by name).
                let belongs = extra.links.iter().find_map(|(kind, id)| match kind.as_str() {
                    "track" => c.track_index(*id).map(|t| c.track_at(t).title().to_string()),
                    "album" => c.album(*id).map(|a| a.title().to_string()),
                    "grouping" => c.grouping(*id).map(|g| g.name.clone()),
                    _ => c.artist(*id).map(|a| a.name.clone()),
                });
                let files = crate::ui::edit::extras::files_text(extra.files as usize);
                let lock = if extra.is_restricted { " · restricted" } else { "" };
                let sub = match belongs {
                    Some(b) => format!("{} · {files} · {b}{lock}", extra.kind),
                    None => format!("{} · {files}{lock}", extra.kind),
                };
                (extra.name.clone(), Some(sub), "Extra".into())
            }
            (Entry::Track(i), Some(c)) => {
                let track = c.track_at(i);
                shell.cover(ui, track.cover(), track.id(), cover_rect, 5, 0.0);
                let sub = match c.main_album(track) {
                    Some(album) if album.kind() != "single" => format!("{} · {}", track.artist_display(), album.title()),
                    _ => track.artist_display().to_string(),
                };
                (track.title().to_string(), Some(sub), "Track".into())
            }
            _ => return,
        };
        let y = if sub.is_some() { -8.0 } else { 0.0 };
        widgets::text_in_og(&painter, Rect::from_min_size(Pos2::new(text_left, rect.center().y + y - 9.0), Vec2::new(width, 18.0)), &title, font(14.0, medium()), P.text, og);
        if let Some(sub) = sub {
            widgets::text_in(&painter, Rect::from_min_size(Pos2::new(text_left, rect.center().y + 1.0), Vec2::new(width, 16.0)), &sub, font(12.0, medium()), P.muted);
        }
        painter.text(rect.right_center() - Vec2::new(14.0, 0.0), Align2::RIGHT_CENTER, kind, font(11.5, medium()), P.faint);
    }
}

/// What Tab writes for `entry`: a key, or a command's `go:` / `do:` form.
fn completion(entry: Entry) -> Option<String> {
    match entry {
        Entry::Insert(i) => Some(INSERTS[i].0.to_string()),
        Entry::Command(i) => Some(format!("{}:{}", if COMMANDS[i].page { "go" } else { "do" }, COMMANDS[i].word)),
        _ => None,
    }
}

/// Does what choosing `entry` means. `shift`: the other way (play instead of open).
/// `name`: the chosen playlist's name.
fn run(shell: &mut Shell, catalog: Option<&Catalog>, mode: &Mode, query: &str, entry: Entry, name: &str, shift: bool) {
    match (mode, entry) {
        (Mode::AddTo(tracks), Entry::Playlist(id)) => shell.playlists.add(id, name.to_string(), tracks.clone()),
        (Mode::AddTo(tracks), Entry::Create) => {
            // (Adding to a playlist waits for the server; making one can't.)
            if shell.needs_server("making a playlist") {
                shell.playlists.create(query.to_string(), tracks.clone());
            }
        }
        (Mode::RemoveFrom(tracks), Entry::Playlist(id)) => shell.playlists.remove_tracks(id, tracks),
        (Mode::Name(None), Entry::Create) => shell.playlists.create(query.to_string(), Vec::new()),
        (Mode::Name(Some(id)), Entry::Create) => shell.playlists.update(*id, Some(query.to_string()), None, None),
        (Mode::Search, _) => search_result(shell, catalog, entry, shift),
        // Made when the form is saved.
        (Mode::Pick(kind, _), Entry::Create) => shell.picker.pick(*kind, None),
        (Mode::Pick(kind, _), entry) => {
            let Some(c) = catalog else { return };
            let id = match entry {
                Entry::Artist(i) => c.artists[i as usize].id,
                Entry::Album(i) => c.album_at(i).id(),
                Entry::Track(i) => c.track_at(i).id(),
                _ => return,
            };
            shell.picker.pick(*kind, Some(id));
        }
        _ => {}
    }
}

fn search_result(shell: &mut Shell, catalog: Option<&Catalog>, entry: Entry, shift: bool) {
    match (entry, catalog) {
        (Entry::Command(i), _) => shell.act((COMMANDS[i].action)()),
        (Entry::Playlist(id), _) => shell.act(Action::Go(Page::Playlist(id))),
        (Entry::Liked, _) => shell.act(Action::Go(Page::Liked)),
        (Entry::Extra(i), Some(c)) => {
            let extra = &c.extras[i as usize];
            // Shift: its video is watched, or its first audio file plays; else its page.
            match (shift, extra.video, extra.first_file) {
                (true, Some(video), _) => shell.watch(video, &extra.name),
                (true, None, Some(file)) => crate::ui::edit::extras::play_file(shell, &extra.name, file, None, &extra.name, extra.duration_ms),
                _ => shell.act(Action::Go(Page::Extra(extra.id))),
            }
        }
        (Entry::Grouping(i), Some(c)) => {
            let grouping = &c.groupings[i as usize];
            if shift {
                let tracks = c.grouping_tracks(grouping.id).to_vec();
                if !tracks.is_empty() {
                    shell.act(Action::Play { tracks, start: 0 });
                }
            } else {
                shell.act(Action::Go(Page::Grouping(grouping.id)));
            }
        }
        (Entry::Artist(i), Some(c)) => {
            let artist = &c.artists[i as usize];
            if shift {
                let tracks = c.artist_tracks(artist.id).to_vec();
                if !tracks.is_empty() {
                    shell.act(Action::Play { tracks, start: 0 });
                }
            } else {
                shell.act(Action::Go(Page::Artist(artist.id)));
            }
        }
        (Entry::Album(i), Some(c)) => {
            let album = c.album_at(i);
            if shift {
                super::albums::play_album(shell, c, album.id());
            } else {
                shell.act(Action::Go(Page::Album(album.id())));
            }
        }
        (Entry::Track(i), Some(c)) => {
            let track = c.track_at(i);
            let album = track.albums().first().map(|m| m.0);
            if shift {
                if let Some(album) = album {
                    shell.act(Action::Go(Page::Album(album)));
                }
                return;
            }
            // Play it, then the rest of its album (or just it, if it's on none).
            match album {
                Some(album) => {
                    let tracks = c.album_tracks(album).to_vec();
                    let start = tracks.iter().position(|&t| t == i).unwrap_or(0);
                    shell.act(Action::Play { tracks, start });
                }
                None => shell.act(Action::Play { tracks: vec![i], start: 0 }),
            }
        }
        _ => {}
    }
}

/// Three lines: a list.
fn playlist_icon(painter: &egui::Painter, center: Pos2) {
    for (i, width) in [16.0, 16.0, 10.0].into_iter().enumerate() {
        let y = center.y - 6.0 + i as f32 * 6.0;
        painter.rect_filled(Rect::from_min_size(Pos2::new(center.x - 8.0, y - 1.0), Vec2::new(width, 2.5)), 1, P.muted);
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_and_do_pick_the_group() {
        assert_eq!(command_line("go:alb"), Some((true, "alb".into())));
        assert_eq!(command_line("do:"), Some((false, String::new())));
        assert_eq!(command_line("run:pause"), Some((false, "pause".into())));
        // Search keys and plain words are searches.
        assert_eq!(command_line("artist:go:x"), None);
        assert_eq!(command_line("go to albums"), None);
        assert_eq!(command_line(""), None);
    }

    #[test]
    fn search_keys_are_not_swallowed() {
        assert_eq!(PALETTE.command("artist").map(|s| s.key), Some("artist"));
        assert_eq!(PALETTE.command("go").map(|s| s.key), Some("go"));
        assert_eq!(PALETTE.command("DO").map(|s| s.key), Some("do"));
        assert_eq!(PALETTE.command("ar").map(|s| s.key), Some("artist"), "the search's by their starts");
    }

    #[test]
    fn tab_completes_keys_and_commands() {
        let command = |word: &str| COMMANDS.iter().position(|c| c.word == word).unwrap();
        assert_eq!(completion(Entry::Command(command("albums"))), Some("go:albums".into()));
        assert_eq!(completion(Entry::Command(command("pause"))), Some("do:pause".into()));
        assert_eq!(completion(Entry::Insert(0)), Some("go:".into()));
        assert_eq!(completion(Entry::Liked), None);
    }

    #[test]
    fn every_command_word_is_unique() {
        let mut words: Vec<_> = COMMANDS.iter().map(|c| c.word).collect();
        words.sort();
        words.dedup();
        assert_eq!(words.len(), COMMANDS.len());
    }

    #[test]
    fn go_lists_only_pages() {
        let palette = Palette { query: "go:".into(), ..Palette::default() };
        let entries = palette.entries(None, &[]);
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| matches!(e, Entry::Command(i) if COMMANDS[*i].page)));
        let palette = Palette { query: "do:pau".into(), ..Palette::default() };
        assert_eq!(palette.entries(None, &[]).len(), 1);
    }

    #[test]
    fn empty_line_lists_the_keys_first() {
        let entries = Palette::default().entries(None, &[]);
        assert_eq!(entries[0], Entry::Insert(0));
        let palette = Palette { query: "ar".into(), ..Palette::default() };
        assert!(palette.entries(None, &[]).contains(&Entry::Insert(2)));
    }
}
