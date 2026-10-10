//! A playlist's page, and "Liked songs": the starred tracks, a playlist that's always
//! there (not one on the server: starring and unstarring is what changes it).
//!
//! Both are a track table (see `track_table`), sortable and filterable. On a playlist of
//! your own: Delete or Shift+P removes the marked tracks (or the selected one), Ctrl+↑/↓ moves it (in the playlist's own
//! order, "#"), F2 renames the playlist; the header has public/private, the history
//! (every change, each one undoable) and the trash.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Instant;

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use uuid::Uuid;

use super::album::play_button;
use super::track_table::{count_and_length, Options, TrackTable, Column};
use crate::catalog::index::{TrackRows, TrackSort};
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use acetate_core::playlists::Entry;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Shell};

pub struct PlaylistPage {
    table: TrackTable,
    history: bool,
    /// The trash button was clicked once: the next click moves it there.
    confirm_trash: bool,
    /// The description being written (for this playlist), if it's open.
    describing: Option<(Uuid, String)>,
    /// Liked songs: every track shown since the page was opened, with when it was starred.
    /// One unstarred meanwhile stays where it was (unstarred), so starring it again is one
    /// key away; it's gone the next time the page opens.
    liked_seen: std::collections::HashMap<Uuid, String>,
    /// Entries taken out since the page opened, each with its place in the list then: still
    /// shown, faded, until the page is opened again (`p` on them puts them back).
    removed: Vec<(usize, Entry)>,
}

/// A playlist's columns: its own order first (and which show to start with).
const PLAYLIST_COLUMNS: &[(Column, bool)] = &[
    (Column::Position, true),
    (Column::Cover, true),
    (Column::Title, true),
    (Column::Artist, true),
    (Column::Album, true),
    (Column::Released, true),
    (Column::Plays, true),
    (Column::Format, true),
    (Column::Og, false),
    (Column::Length, true),
    (Column::Added, true),
];

impl Default for PlaylistPage {
    fn default() -> Self {
        PlaylistPage { table: TrackTable::new(TrackSort::Natural), history: false, confirm_trash: false, describing: None, liked_seen: Default::default(), removed: Vec::new() }
    }
}

impl PlaylistPage {
    pub fn show_liked(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, player: &PlayerView) {
        let (mut tracks, mut starred) = liked(shell, catalog);
        let facts = format!("{} · star a track to add it here (s)", count_and_length(catalog, tracks.iter().copied()));
        // Unstarred since the page opened: still here, in their place.
        for (&t, at) in tracks.iter().zip(&starred) {
            self.liked_seen.insert(catalog.track_at(t).id(), at.clone());
        }
        let kept: Vec<(u32, String)> = self
            .liked_seen
            .iter()
            .filter_map(|(id, at)| Some((catalog.track_index(*id)?, at.clone())))
            .filter(|(t, _)| !tracks.contains(t))
            .collect();
        if !kept.is_empty() {
            let mut all: Vec<(u32, String)> = tracks.into_iter().zip(starred).chain(kept).collect();
            // (Stable: the order among those starred at the same time stays.)
            all.sort_by(|a, b| b.1.cmp(&a.1));
            (tracks, starred) = all.into_iter().unzip();
        }
        header(ui, "LIKED SONGS", "Liked songs", &facts);
        // Unstarred since the page opened: faded, still in their place.
        let faded: Vec<bool> = tracks.iter().map(|&t| !shell.mark(catalog.track_at(t).id()).starred).collect();
        let rows = TrackRows { tracks: Some(&tracks), added: Some(&starred), faded: Some(&faded) };
        let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
        if play_button(ui, "liked-play", play_rect) && !tracks.is_empty() {
            shell.act(Action::Play { tracks: self.table.tracks(&rows), start: 0 });
        }
        ui.horizontal(|ui| widgets::download_chip(ui, shell, "liked", "", "Liked songs"));
        ui.add_space(16.0);
        let options = Options { key: "liked", columns: PLAYLIST_COLUMNS, added: Some("Starred"), filter_hint: Some("Filter liked songs: words, or artist:… year:…"), album: None, places: None, keys: true, scroll: true };
        let shown = self.table.show(ui, shell, catalog, &rows, version(shell.generation, &tracks), player, &options);
        // Shift+P unstars the marked tracks (or the selected one); `p` on faded ones stars them again.
        let target = |rows: Vec<usize>, starred: bool| -> Vec<Uuid> {
            rows.into_iter().filter(|&r| faded[r] != starred).map(|r| catalog.track_at(tracks[r]).id()).collect()
        };
        let mut change: Vec<Uuid> = Vec::new();
        if shown.remove {
            let rows = if self.table.has_marks() { self.table.marked_rows(catalog, &rows) } else { shown.selected.into_iter().collect() };
            change = target(rows, true);
        } else if !shown.restore.is_empty() {
            change = target(shown.restore.clone(), false);
        }
        if !change.is_empty() {
            for id in change {
                shell.act(Action::ToggleStar(Kind::Track, id));
            }
            self.table.clear_marks();
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let cached = shell.playlists.detail(id);
        let (detail, refreshing, error) = (cached.value.clone(), cached.refreshing, cached.error.clone());
        let Some(detail) = detail else {
            let text = error.unwrap_or_else(|| if refreshing { "Loading…".into() } else { "Couldn't load it".into() });
            ui.label(RichText::new(text).color(P.muted));
            return;
        };
        // The entries as catalog tracks (an entry of a track not in the catalog yet, just
        // added elsewhere, shows after the next library refresh), with their entry ids
        // and when they were added.
        // Entries taken out meanwhile stay, in their place, faded (unless they're back).
        let mut list: Vec<(Entry, bool)> = detail.entries.iter().map(|e| (e.clone(), false)).collect();
        self.removed.retain(|(_, e)| !detail.entries.iter().any(|d| d.id == e.id));
        let mut ghosts: Vec<&(usize, Entry)> = self.removed.iter().collect();
        ghosts.sort_by_key(|(at, _)| *at);
        for (at, e) in ghosts {
            list.insert((*at).min(list.len()), (e.clone(), true));
        }
        let mut tracks = Vec::new();
        let mut entries = Vec::new();
        let mut added = Vec::new();
        let mut faded = Vec::new();
        // Each row's place in `list`.
        let mut places = Vec::new();
        for (n, (e, gone)) in list.iter().enumerate() {
            if let Some(t) = catalog.track_index(e.track_id) {
                tracks.push(t);
                entries.push(e.id);
                // (Added here and not sent yet: the server hasn't said when.)
                added.push(if e.waiting { "waiting".to_string() } else { e.added_at.clone() });
                faded.push(*gone);
                places.push(n);
            }
        }
        let rows = TrackRows { tracks: Some(&tracks), added: Some(&added), faded: Some(&faded) };
        let live: Vec<u32> = tracks.iter().zip(&faded).filter(|(_, f)| !**f).map(|(t, _)| *t).collect();

        let owner = if detail.mine { String::new() } else { format!(" · by {}", detail.owner) };
        let visibility = if detail.public { "public" } else { "private" };
        header(ui, "PLAYLIST", &detail.name, &format!("{}{owner} · {visibility}", count_and_length(catalog, live.iter().copied())));
        if !detail.description.is_empty() {
            ui.label(RichText::new(&detail.description).color(P.muted));
        }

        ui.horizontal(|ui| {
            let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
            if play_button(ui, "playlist-play", play_rect) && !live.is_empty() {
                // (Not the ones just taken out.)
                let mut tracks = self.table.tracks(&rows);
                if !self.removed.is_empty() {
                    tracks.retain(|t| live.contains(t));
                }
                shell.act(Action::Play { tracks, start: 0 });
            }
            ui.add_space(8.0);
            if detail.mine {
                if widgets::chip(ui, if detail.public { "Public" } else { "Private" }, detail.public) && shell.needs_server("changing a playlist") {
                    shell.playlists.update(id, None, Some(!detail.public), None);
                }
                if widgets::chip(ui, "Rename  F2", false) && shell.needs_server("renaming a playlist") {
                    shell.act(Action::RenamePlaylist(id));
                }
                let open = self.describing.as_ref().is_some_and(|(d, _)| *d == id);
                if widgets::chip(ui, "Describe…", open) && (open || shell.needs_server("changing a playlist")) {
                    self.describing = if open { None } else { Some((id, detail.description.clone())) };
                }
            }
            if widgets::chip(ui, "History", self.history) && (self.history || shell.needs_server("a playlist's history")) {
                self.history = !self.history;
                if self.history {
                    shell.playlists.fetch_log(id);
                }
            }
            if detail.mine {
                let label = if self.confirm_trash { "Click again: move to trash" } else { "Move to trash" };
                if widgets::chip(ui, label, self.confirm_trash) && shell.needs_server("moving a playlist to the trash") {
                    if self.confirm_trash {
                        shell.playlists.trash(id, detail.name.clone());
                        shell.act(Action::Back);
                    }
                    self.confirm_trash = !self.confirm_trash;
                }
            }
        });
        // The description, being written.
        if let Some((_, text)) = self.describing.as_mut().filter(|(d, _)| *d == id) {
            ui.add_space(8.0);
            ui.add(egui::TextEdit::multiline(text).hint_text("What it's for, what's in it (optional)").desired_rows(3).desired_width(520.0));
            let text = text.trim().to_string();
            ui.horizontal(|ui| {
                if ui.button("Save the description").clicked() && shell.needs_server("changing a playlist") {
                    shell.playlists.update(id, None, None, Some(text));
                    self.describing = None;
                }
                if ui.button("Cancel").clicked() {
                    self.describing = None;
                }
            });
        }
        ui.add_space(16.0);

        if self.history {
            self.history_panel(ui, shell, id, detail.mine);
            ui.add_space(12.0);
        }

        let options = Options { key: "playlist", columns: PLAYLIST_COLUMNS, added: Some("Added"), filter_hint: Some("Filter this playlist: words, or artist:… year:…"), album: None, places: None, keys: true, scroll: true };
        let shown = self.table.show(ui, shell, catalog, &rows, version(shell.generation, &entries), player, &options);

        // `p` on faded rows: back into the playlist (at its end), and no longer faded.
        if !shown.restore.is_empty() && detail.mine {
            let back: Vec<Uuid> = shown.restore.iter().map(|&row| list[places[row]].0.track_id).collect();
            let gone: Vec<Uuid> = shown.restore.iter().map(|&row| entries[row]).collect();
            self.removed.retain(|(_, e)| !gone.contains(&e.id));
            shell.playlists.add(id, detail.name.clone(), back);
            self.table.clear_marks();
        }
        // Keys for your own playlist (outside text fields).
        if shown.remove && !detail.mine {
            shell.playlists.notice = Some(("Only its owner can take tracks out of a playlist".into(), Instant::now()));
        }
        if detail.mine && !ui.ctx().egui_wants_keyboard_input() {
            let (delete, up, down, rename) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Delete),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::F2),
                )
            });
            let remove = delete || shown.remove;
            // (Taking tracks out and moving them need the server: adding doesn't.)
            let (remove, up, down, rename) = if (remove || up || down || rename) && !shell.needs_server("changing a playlist's tracks") {
                (false, false, false, false)
            } else {
                (remove, up, down, rename)
            };
            if remove {
                // The marked entries (just the copy marked of a track in it twice), or the selected one.
                let marked = self.table.marked_rows(catalog, &rows);
                let out: Vec<usize> = if !marked.is_empty() { marked } else { shown.selected.into_iter().collect() };
                let out: Vec<usize> = out.into_iter().filter(|&row| !faded[row]).collect();
                if !out.is_empty() {
                    shell.playlists.remove(id, out.iter().map(|&row| entries[row]).collect());
                    self.removed.extend(out.iter().map(|&row| (places[row], list[places[row]].0.clone())));
                    self.table.clear_marks();
                }
            }
            if let Some(row) = shown.selected {
                let by: isize = if up { -1 } else if down { 1 } else { 0 };
                if by != 0 && !self.removed.is_empty() {
                    shell.playlists.notice = Some(("Not while the tracks just taken out are shown: open the playlist again".into(), Instant::now()));
                } else if by != 0 && !shown.in_own_order {
                    shell.playlists.notice = Some(("To move tracks, sort by # and clear the filter".into(), Instant::now()));
                } else if by != 0 && (row as isize + by) >= 0 && ((row as isize + by) as usize) < entries.len() {
                    shell.playlists.move_entry(id, entries[row], by);
                    self.table.select((row as isize + by) as usize);
                }
            }
            if rename {
                shell.act(Action::RenamePlaylist(id));
            }
        }
        if tracks.is_empty() {
            ui.label(RichText::new("Select a track anywhere and press p to add it here").color(P.faint));
        }
    }

    fn history_panel(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, mine: bool) {
        let Some(log) = shell.playlists.log(id).map(<[_]>::to_vec) else {
            ui.label(RichText::new("Loading the history…").color(P.muted));
            return;
        };
        egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if log.is_empty() {
                ui.label(RichText::new("No changes yet").color(P.muted));
            }
            egui::ScrollArea::vertical().max_height(180.0).id_salt("playlist-history").show(ui, |ui| {
                for entry in &log {
                    ui.horizontal(|ui| {
                        let when = entry.at.get(..16).unwrap_or(&entry.at).replace('T', " ");
                        ui.label(RichText::new(when).font(font(12.0, medium())).color(P.faint));
                        let color = if entry.undone_at.is_some() { P.faint } else { P.text };
                        let mut text = entry.describe();
                        if let Some(by) = &entry.by {
                            text.push_str(&format!(" · {by}"));
                        }
                        if entry.undone_at.is_some() {
                            text.push_str(" · undone");
                        }
                        ui.label(RichText::new(text).color(color));
                        if mine && entry.undoable() && ui.add(egui::Button::new(RichText::new("Undo").color(P.accent)).frame(false)).clicked() && shell.needs_server("undoing") {
                            shell.playlists.undo(id, entry.id);
                        }
                    });
                }
            });
        });
    }
}

/// The starred tracks and when each was starred: starred in this session first
/// (newest first, "today"), then by when they were starred (newest first).
fn liked(shell: &Shell, catalog: &Catalog) -> (Vec<u32>, Vec<String>) {
    let mut tracks = Vec::new();
    let mut starred = Vec::new();
    for (id, now) in shell.marks.changed_stars() {
        let was = catalog.annotations.get(&id).is_some_and(|a| a.starred_at.is_some());
        if let (true, false, Some(t)) = (now, was, catalog.track_index(id)) {
            tracks.push(t);
            // Sorts after any date, so newest first.
            starred.push("today".to_string());
        }
    }
    let mut earlier: Vec<(&str, u32)> = catalog
        .annotations
        .values()
        .filter(|a| a.kind == "track")
        .filter_map(|a| Some((a.starred_at.as_deref()?, catalog.track_index(a.id)?)))
        // Unstarred in this session: gone.
        .filter(|(_, t)| shell.mark(catalog.track_at(*t).id()).starred)
        .collect();
    earlier.sort_by(|a, b| b.0.cmp(a.0));
    for (at, t) in earlier {
        tracks.push(t);
        starred.push(at.to_string());
    }
    (tracks, starred)
}

/// A number that changes when the rows do (for the table to sort and filter again).
fn version<T: Hash>(generation: u64, rows: &[T]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    generation.hash(&mut hasher);
    rows.hash(&mut hasher);
    hasher.finish()
}

fn header(ui: &mut egui::Ui, kind: &str, title: &str, facts: &str) {
    ui.label(RichText::new(kind).font(font(11.0, semibold())).color(P.accent));
    ui.label(RichText::new(title).font(font(40.0, bold())));
    ui.label(RichText::new(facts).font(font(14.0, medium())).color(P.muted));
    ui.add_space(12.0);
}
