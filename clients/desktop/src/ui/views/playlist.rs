//! A playlist's page, and "Liked songs": the starred tracks, a playlist that's always
//! there (not one on the server: starring and unstarring is what changes it).
//!
//! Both are a track table (see `track_table`), sortable and filterable. On a playlist of
//! your own: Delete removes the selected track, Ctrl+↑/↓ moves it (in the playlist's own
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
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Shell};

pub struct PlaylistPage {
    table: TrackTable,
    history: bool,
    /// The trash button was clicked once: the next click moves it there.
    confirm_trash: bool,
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
    (Column::Length, true),
    (Column::Added, true),
];

impl Default for PlaylistPage {
    fn default() -> Self {
        PlaylistPage { table: TrackTable::new(TrackSort::Natural), history: false, confirm_trash: false }
    }
}

impl PlaylistPage {
    pub fn show_liked(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, player: &PlayerView) {
        let (tracks, starred) = liked(shell, catalog);
        header(ui, "LIKED SONGS", "Liked songs", &format!("{} · star a track to add it here (s)", count_and_length(catalog, tracks.iter().copied())));
        let rows = TrackRows { tracks: Some(&tracks), added: Some(&starred) };
        let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
        if play_button(ui, "liked-play", play_rect) && !tracks.is_empty() {
            shell.act(Action::Play { tracks: self.table.tracks(&rows), start: 0 });
        }
        ui.add_space(16.0);
        let options = Options { key: "liked", columns: PLAYLIST_COLUMNS, added: Some("Starred"), filter_hint: Some("Filter liked songs: words, or artist:… year:…"), album: None };
        self.table.show(ui, shell, catalog, &rows, version(shell.generation, &tracks), player, &options);
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
        let mut tracks = Vec::new();
        let mut entries = Vec::new();
        let mut added = Vec::new();
        for e in &detail.entries {
            if let Some(t) = catalog.track_index(e.track_id) {
                tracks.push(t);
                entries.push(e.id);
                added.push(e.added_at.clone());
            }
        }
        let rows = TrackRows { tracks: Some(&tracks), added: Some(&added) };

        let owner = if detail.mine { String::new() } else { format!(" · by {}", detail.owner) };
        let visibility = if detail.public { "public" } else { "private" };
        header(ui, "PLAYLIST", &detail.name, &format!("{}{owner} · {visibility}", count_and_length(catalog, tracks.iter().copied())));
        if !detail.description.is_empty() {
            ui.label(RichText::new(&detail.description).color(P.muted));
        }

        ui.horizontal(|ui| {
            let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
            if play_button(ui, "playlist-play", play_rect) && !tracks.is_empty() {
                shell.act(Action::Play { tracks: self.table.tracks(&rows), start: 0 });
            }
            ui.add_space(8.0);
            if detail.mine {
                if widgets::chip(ui, if detail.public { "Public" } else { "Private" }, detail.public) {
                    shell.playlists.update(id, None, Some(!detail.public));
                }
                if widgets::chip(ui, "Rename  F2", false) {
                    shell.act(Action::RenamePlaylist(id));
                }
            }
            if widgets::chip(ui, "History", self.history) {
                self.history = !self.history;
                if self.history {
                    shell.playlists.fetch_log(id);
                }
            }
            if detail.mine {
                let label = if self.confirm_trash { "Click again: move to trash" } else { "Move to trash" };
                if widgets::chip(ui, label, self.confirm_trash) {
                    if self.confirm_trash {
                        shell.playlists.trash(id, detail.name.clone());
                        shell.act(Action::Back);
                    }
                    self.confirm_trash = !self.confirm_trash;
                }
            }
        });
        ui.add_space(16.0);

        if self.history {
            self.history_panel(ui, shell, id, detail.mine);
            ui.add_space(12.0);
        }

        let options = Options { key: "playlist", columns: PLAYLIST_COLUMNS, added: Some("Added"), filter_hint: Some("Filter this playlist: words, or artist:… year:…"), album: None };
        let shown = self.table.show(ui, shell, catalog, &rows, version(shell.generation, &entries), player, &options);

        // Keys for your own playlist (outside text fields).
        if detail.mine && !ui.ctx().egui_wants_keyboard_input() {
            let (delete, up, down, rename) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Delete),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::F2),
                )
            });
            if let Some(row) = shown.selected {
                if delete {
                    shell.playlists.remove(id, vec![entries[row]]);
                }
                let by: isize = if up { -1 } else if down { 1 } else { 0 };
                if by != 0 && !shown.in_own_order {
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
                        if mine && entry.undoable() && ui.add(egui::Button::new(RichText::new("Undo").color(P.accent)).frame(false)).clicked() {
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
        .filter(|(_, t)| shell.mark(catalog.tracks[*t as usize].id).starred)
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
