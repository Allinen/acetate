//! An album's page: its cover and facts, a Play button, its commandline (`/`, see
//! `album_commands`), and its tracks. Keys: arrows or jk move, Enter plays from there; in
//! edit mode, Alt+↑/↓ or Alt+K/J moves the selected track in the tracklist, saved together
//! with Ctrl+Enter (see `edit::track_order`).

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::catalog::marks::Kind;
use super::album_commands::{Act, AlbumCommands, Filters, Here, Reveal};
use super::track_table::{Column, Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Shell};

/// An album's columns (and which show to start with).
const ALBUM_COLUMNS: &[(Column, bool)] = &[
    (Column::Number, true),
    (Column::Cover, false),
    (Column::Title, true),
    (Column::Artist, true),
    (Column::Released, false),
    (Column::Plays, true),
    (Column::Format, false),
    (Column::Og, false),
    (Column::Length, true),
];

#[derive(Default)]
pub struct AlbumPage {
    list: TrackTable,
    /// Its notes (folded under their titles), and writing them in edit mode.
    notes: crate::ui::edit::notes::NotesView,
    /// Its extras, and attaching them in edit mode.
    extras: crate::ui::edit::extras::ExtrasView,
    /// This album and another side by side, to merge (edit mode), in place of the list.
    merge: Option<super::album_merge::AlbumMergeView>,
    /// "Merge into another album…" asked the palette for the other one.
    picking_merge: bool,
    /// Undoing merges into this album (edit mode).
    undo: super::album_merge::UndoPanel,
    /// Its sources (edit mode).
    sources: crate::ui::edit::sources::SourcesEditor,
    /// Its covers: order, trash, copies, pasted ones (edit mode).
    covers: crate::ui::edit::cover_list::CoverList,
    /// The groupings it's in.
    part_of: crate::ui::edit::part_of::PartOf,
    /// Tracks moved in its tracklist (edit mode): drafts, saved together.
    order: crate::ui::edit::track_order::TrackOrder,
    /// Its tracks filtered by words, only original files (the chip): what's chosen.
    filters: Filters,
    /// The filter last given to the track list.
    applied: Option<String>,
    /// The album the filters are for (another one: its words start empty).
    filtered: Option<Uuid>,
    /// The commandline under the header.
    commands: AlbumCommands,
    /// A part of the page the commandline asked for: opened and scrolled to.
    reveal: Option<Reveal>,
}

impl AlbumPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        // Ctrl+M: merge into another album (again: close it). In a text field too. With
        // tracks marked, it merges those instead (the list takes it).
        if !shell.palette_open && !self.list.takes_ctrl_m(shell) && ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if !shell.edit_mode {
                shell.playlists.notice = Some(("Merging is in edit mode: e, then Ctrl+M.".into(), std::time::Instant::now()));
            } else if self.merge.is_some() {
                self.merge = None;
            } else {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
        }
        if !shell.edit_mode {
            self.undo.close();
        }
        let Some(album) = catalog.album(id) else {
            ui.label(RichText::new("This album isn't in the library (anymore).").color(P.muted));
            return;
        };
        self.order.update(shell, catalog, id);
        let moved = self.order.tracks(shell, catalog, id, catalog.album_tracks(id));
        let tracks: &[u32] = moved.as_deref().unwrap_or(catalog.album_tracks(id));
        if self.filtered != Some(id) {
            self.filtered = Some(id);
            self.filters.words.clear();
        }
        let og_count = tracks.iter().filter(|&&t| catalog.tracks[t as usize].original_filename.is_some()).count();
        if og_count == 0 {
            self.filters.og_only = false;
        }

        // What the commandline knows.
        let details = shell.album_details.get(id).value.clone();
        let vocab = shell.account.vocab.clone().unwrap_or_default();
        let credits = details.as_ref().map_or(&[][..], |d| &d.credits[..]);
        // The other names of artists typed with `as:` (looked up as they're typed).
        let aliases: Vec<(Uuid, Vec<crate::details::AliasInfo>)> =
            self.commands.artists_named(catalog, credits).into_iter().filter_map(|a| shell.artist_details.get(a).value.as_ref().map(|d| (a, d.aliases.clone()))).collect();
        let mut sections = Vec::new();
        if let Some(d) = &details {
            let edit = shell.edit_mode;
            for (section, there) in [
                (Reveal::Credits, edit || !d.credits.is_empty()),
                (Reveal::Covers, if edit { true } else { d.covers.len() > 1 }),
                (Reveal::Sources, edit || !d.sources.is_empty()),
                (Reveal::Notes, crate::ui::edit::notes::NotesView::worth_showing(shell, &d.notes) && !tracks.is_empty()),
                (Reveal::Extras, crate::ui::edit::extras::ExtrasView::worth_showing(shell, &d.extras) && !tracks.is_empty()),
                (Reveal::Merges, edit && !d.merges.is_empty()),
            ] {
                if there {
                    sections.push(section);
                }
            }
        }
        let mark = shell.mark(id);
        let may_edit = shell.edit_mode && shell.account.can("edit_metadata");
        let here = Here {
            id,
            catalog,
            vocab: &vocab,
            tracks,
            og_count,
            sections: &sections,
            starred: mark.starred,
            rating: mark.rating,
            edit_mode: shell.edit_mode,
            may_edit,
            title: &album.title,
            kind: &album.kind,
            release_date: album.release_date.as_deref().unwrap_or(""),
            credits,
            aliases: &aliases,
            groupings: &album.groupings,
            covers: details.as_ref().map_or(&[][..], |d| &d.covers[..]),
            sources: details.as_ref().map_or(&[][..], |d| &d.sources[..]),
            may_source: shell.edit_mode && shell.account.can("edit_sources"),
            extras: details.as_ref().map_or(&[][..], |d| &d.extras[..]),
            may_access: shell.edit_mode && shell.account.can("manage_access"),
            notes: details.as_ref().map_or(&[][..], |d| &d.notes[..]),
        };
        // As the chips are, with what's being typed in the commandline on top.
        let filters = self.commands.preview(&here, &self.filters);
        let filter = filters.text();
        if self.applied.as_ref() != Some(&filter) {
            self.list.set_filter(&filter);
            self.applied = Some(filter);
        }
        // One scroll for the whole page (header, edit panels, notes, the tracks as tall
        // as they are): nothing in it scrolls on its own.
        egui::ScrollArea::vertical().id_salt(("album-page", id)).auto_shrink([false, false]).show(ui, |ui| {
            // Header: a big cover, and what the album is.
            let header = ui.allocate_exact_size(Vec2::new(ui.available_width(), 232.0), Sense::hover()).0;
            let cover_rect = Rect::from_min_size(header.min, Vec2::splat(220.0));
            shell.cover_with_original(ui, album.cover(), album.id, cover_rect, 12, 0.6);
            // Every track an original file (an og file): it says so, with an amber edge.
            let all_og = og_count > 0 && og_count == tracks.len();
            if all_og {
                widgets::og_edge(ui.painter(), cover_rect, 12, 2.0);
            }
            let left = cover_rect.right() + 28.0;
            let painter = ui.painter();
            let kind = match (all_og, tracks.len()) {
                (false, _) => album.kind.to_uppercase(),
                (true, 1) => format!("{}  ·  ORIGINAL FILE", album.kind.to_uppercase()),
                (true, _) => format!("{}  ·  ORIGINAL FILES", album.kind.to_uppercase()),
            };
            painter.text(Pos2::new(left, cover_rect.top() + 70.0), Align2::LEFT_BOTTOM, kind, font(11.0, semibold()), P.accent);
            widgets::text_in(painter, Rect::from_min_size(Pos2::new(left, cover_rect.top() + 76.0), Vec2::new(header.right() - left, 44.0)), &album.title, font(36.0, bold()), P.text);
            let mut facts = Vec::new();
            if let Some(date) = &album.release_date {
                facts.push(date.clone());
            }
            let count = tracks.len();
            facts.push(format!("{count} track{}", if count == 1 { "" } else { "s" }));
            let total: i64 = tracks.iter().filter_map(|&t| catalog.tracks[t as usize].duration_ms).sum();
            facts.push(widgets::clock(Duration::from_millis(total.max(0) as u64)));
            painter.text(Pos2::new(left, cover_rect.top() + 150.0), Align2::LEFT_TOP, facts.join("  ·  "), font(14.0, medium()), P.muted);

            // The album's artists, each a link.
            let artists_rect = Rect::from_min_size(Pos2::new(left, cover_rect.top() + 126.0), Vec2::new(header.right() - left, 20.0));
            let mut artists_ui = ui.new_child(egui::UiBuilder::new().max_rect(artists_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            super::artist_links(&mut artists_ui, shell, catalog, &album.credits, &album.artist_display, font(15.0, medium()), P.text);

            let play_rect = Rect::from_min_size(Pos2::new(left, cover_rect.bottom() - 44.0), Vec2::new(116.0, 42.0));
            if play_button(ui, "album-play", play_rect) && !tracks.is_empty() {
                shell.act(Action::Play { tracks: tracks.to_vec(), start: 0 });
            }
            let marks_rect = Rect::from_min_size(Pos2::new(play_rect.right() + 18.0, play_rect.top()), Vec2::new(460.0, play_rect.height()));
            let mut marks_ui = ui.new_child(egui::UiBuilder::new().max_rect(marks_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
            marks(&mut marks_ui, shell, Kind::Album, id);
            marks_ui.add_space(8.0);
            widgets::share_chip(&mut marks_ui, shell, crate::links::Target::Album(id));
            // Its original files (og files): a chip that shows only those (`og:` in the filter).
            if og_count > 0 {
                marks_ui.add_space(4.0);
                if widgets::chip(&mut marks_ui, &format!("Original files  {og_count}"), filters.og_only) {
                    self.filters.og_only = !self.filters.og_only;
                }
            }
            if shell.edit_mode && self.merge.is_none() {
                marks_ui.add_space(8.0);
                let tip = "The same album twice: choose the other one, then see both before merging";
                if marks_ui.button("Merge into another album…  (Ctrl+M)").on_hover_text(tip).clicked() {
                    self.picking_merge = true;
                    shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
                }
            }
            ui.add_space(10.0);
            // The commandline (`/`): playing, filters, the parts of the page, and the rest by name.
            if let Some(act) = self.commands.show(ui, shell, id, &here, &mut self.filters) {
                self.act(shell, catalog, id, tracks, act);
            }
            ui.add_space(10.0);
            let groupings = album.groupings.clone();
            self.part_of.show(ui, shell, catalog, crate::ui::edit::part_of::Member::Album(id), &groupings);
            ui.add_space(12.0);

            // The other album, picked in the palette (not a new one, not this one).
            if self.picking_merge
                && let Some(picked) = shell.picker.take()
            {
                self.picking_merge = false;
                if let (crate::ui::edit::PickKind::Album, Some(other)) = (picked.kind, picked.id)
                    && other != id
                {
                    self.merge = Some(super::album_merge::AlbumMergeView::new(id, other));
                }
            }
            if let Some(view) = self.merge.as_mut() {
                use super::album_merge::Outcome;
                shell.form_dirty = true;
                match view.show(ui, shell, catalog) {
                    Some(Outcome::Merged { kept, gone }) => {
                        self.merge = None;
                        shell.album_details.refresh(kept);
                        shell.album_details.refresh(gone);
                        shell.act(Action::RefreshLibrary);
                        if kept != id {
                            shell.act(Action::Go(crate::ui::Page::Album(kept)));
                        }
                    }
                    Some(Outcome::Closed) => self.merge = None,
                    None => {}
                }
                return;
            }
            // Listen mode: the rest of what's known about it, folded (opt-in, as on a
            // track's page). Edit mode has all of it in its panels below.
            if !shell.edit_mode
                && let Some(d) = shell.album_details.get(id).value.clone()
            {
                // An id of their own (not this album's): what's opened stays open on
                // the next album.
                let reveal = self.reveal.take_if(|r| matches!(r, Reveal::Credits | Reveal::Covers | Reveal::Sources));
                ui.scope_builder(egui::UiBuilder::new().id(egui::Id::new("album-sections")), |ui| view_sections(ui, shell, &d, reveal));
            }

            // Edit mode: the album's own data (title, kind, date, artists).
            if shell.edit_mode {
                // A track or album picked to copy a cover from (before the form takes it).
                self.covers.take_pick(shell);
                let reveal = self.reveal;
                let heading = |ui: &mut egui::Ui, title: &str, section: Reveal| {
                    let label = ui.label(RichText::new(title).font(font(14.0, semibold())));
                    if reveal == Some(section) {
                        label.scroll_to_me(Some(egui::Align::Min));
                    }
                };
                edit_panel(ui, shell, catalog, id, reveal == Some(Reveal::Credits));
                ui.add_space(12.0);
                if let Some(d) = shell.album_details.get(id).value.clone() {
                    panel(ui, "album-covers", |ui| {
                        heading(ui, "Covers", Reveal::Covers);
                        self.covers.show(ui, shell, crate::ui::edit::cover_list::Owner::Album(id), &d.covers, true);
                    });
                    ui.add_space(12.0);
                    panel(ui, "album-sources", |ui| {
                        heading(ui, "Sources", Reveal::Sources);
                        self.sources.show(ui, shell, crate::ui::edit::sources::Owner::Album(id), &d.sources);
                    });
                    ui.add_space(12.0);
                    if !d.merges.is_empty() {
                        panel(ui, "album-merges", |ui| {
                            heading(ui, "Merged into this album", Reveal::Merges);
                            let can = shell.account.can("edit_metadata");
                            self.undo.show(ui, shell, id, &d.merges, can);
                        });
                        ui.add_space(12.0);
                    }
                }
            }

            // (Edit mode's panels are drawn: what they were scrolled to is done.)
            if shell.edit_mode {
                self.reveal.take_if(|r| !matches!(r, Reveal::Notes | Reveal::Extras));
            }
            if tracks.is_empty() {
                ui.label(RichText::new("No tracks you can see").color(P.muted));
                return;
            }
            let rows = TrackRows { tracks: Some(tracks), added: None };
            // Its notes, folded under their titles (above the tracks: they're short to pass).
            if let Some(d) = shell.album_details.get(id).value.clone()
                && crate::ui::edit::notes::NotesView::worth_showing(shell, &d.notes)
            {
                let heading = ui.label(RichText::new("Notes").font(font(16.0, semibold())));
                if self.reveal.take_if(|r| *r == Reveal::Notes).is_some() {
                    heading.scroll_to_me(Some(egui::Align::Min));
                }
                self.notes.show(ui, shell, crate::ui::edit::notes::Owner::Album(id), &d.notes, true, true);
                ui.add_space(10.0);
            }
            if let Some(d) = shell.album_details.get(id).value.clone()
                && crate::ui::edit::extras::ExtrasView::worth_showing(shell, &d.extras)
            {
                let heading = ui.label(RichText::new("Extras").font(font(16.0, semibold())));
                if self.reveal.take_if(|r| *r == Reveal::Extras).is_some() {
                    heading.scroll_to_me(Some(egui::Align::Min));
                }
                self.extras.show(ui, shell, crate::ui::edit::extras::Owner::Album(id), &d.extras, false, true);
                ui.add_space(10.0);
            }
            self.order.bar(ui, shell, id, self.list.has_marks());
            let options = Options { key: "album", columns: ALBUM_COLUMNS, added: None, filter_hint: None, album: Some((id, &album.artist_display)), places: self.order.places(shell, id), keys: true, scroll: false };
            let shown = self.list.show(ui, shell, catalog, &rows, version(shell.generation, tracks), player, &options);

            // Alt+↑/↓ or Alt+K/J: the selected track a place earlier or later (edit mode).
            if !self.list.popup_open() && !shell.palette_open && !ui.ctx().egui_wants_keyboard_input() {
                use egui::{Key, Modifiers};
                let (up, down) = ui.ctx().input_mut(|i| {
                    let up = i.consume_key(Modifiers::ALT, Key::ArrowUp) || i.consume_key(Modifiers::ALT, Key::K);
                    let down = i.consume_key(Modifiers::ALT, Key::ArrowDown) || i.consume_key(Modifiers::ALT, Key::J);
                    (up, down)
                });
                let by: isize = if up { -1 } else if down { 1 } else { 0 };
                if by != 0
                    && let Some(row) = shown.selected
                {
                    let notice = if !shell.edit_mode {
                        Some("Moving tracks is in edit mode: e, then Alt+↑/↓.")
                    } else if !shell.account.can("edit_metadata") {
                        Some("Moving tracks needs the edit_metadata permission.")
                    } else if !shown.in_own_order {
                        Some("To move tracks, sort by # and clear the filter")
                    } else {
                        None
                    };
                    match notice {
                        Some(text) => shell.playlists.notice = Some((text.into(), std::time::Instant::now())),
                        None => {
                            if let Some(to) = self.order.move_track(shell, catalog, id, tracks, row, by) {
                                self.list.select(to);
                            }
                        }
                    }
                }
            }
        });
    }
}

impl AlbumPage {
    /// What the commandline asked for.
    fn act(&mut self, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, tracks: &[u32], act: Act) {
        use crate::links::Target;
        match act {
            Act::Play(start) => shell.act(Action::Play { tracks: tracks.to_vec(), start }),
            Act::Queue(next) => shell.act(Action::Enqueue { tracks: tracks.to_vec(), next }),
            Act::Star(on) => {
                if shell.mark(id).starred != on {
                    shell.act(Action::ToggleStar(Kind::Album, id));
                }
            }
            Act::Rate(rating) => shell.act(Action::Rate(Kind::Album, id, rating)),
            Act::CopyLink => shell.act(Action::CopyLink(Target::Album(id))),
            Act::ShareLink => shell.act(Action::CopyShareLink(Target::Album(id))),
            Act::Go(page) => shell.act(Action::Go(page)),
            Act::Reveal(section) => self.reveal = Some(section),
            Act::Move(from, to) => {
                // One place at a time, as Alt+↑/↓ (each move is on the tracklist's draft).
                let mut row = from;
                for _ in 0..tracks.len() {
                    if row == to {
                        break;
                    }
                    let order = self.order.tracks(shell, catalog, id, catalog.album_tracks(id)).unwrap_or_else(|| catalog.album_tracks(id).to_vec());
                    match self.order.move_track(shell, catalog, id, &order, row, if to < row { -1 } else { 1 }) {
                        Some(at) if at != row => row = at,
                        _ => break,
                    }
                }
                self.list.select(row);
            }
            Act::Merge => {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
            Act::ToggleEdit => shell.act(Action::ToggleEditMode),
            Act::Note(note, title) => {
                let d = shell.album_details.get(id).value.clone();
                let found = d.as_ref().and_then(|d| d.notes.iter().find(|n| Some(n.id) == note));
                self.notes.open(crate::ui::edit::notes::Owner::Album(id), found, &title);
            }
            Act::PasteNotes => {
                let made = self.notes.add_files(crate::ui::edit::notes::Owner::Album(id), crate::ui::files::clipboard_files());
                if made == 0 {
                    shell.playlists.notice = Some(("No text files in the clipboard (copy .txt, .md or .nfo files in a file manager first).".into(), std::time::Instant::now()));
                }
            }
        }
    }
}

/// The list's version: the library's, and the order shown (moved tracks change it).
fn version(generation: u64, tracks: &[u32]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    generation.hash(&mut hasher);
    tracks.hash(&mut hasher);
    hasher.finish()
}

/// Listen mode: its credits, covers (more than the one shown) and sources, each folded
/// until opened (`reveal`: that one opened and scrolled to).
fn view_sections(ui: &mut egui::Ui, shell: &mut Shell, d: &crate::details::AlbumDetail, reveal: Option<Reveal>) {
    let section = |ui: &mut egui::Ui, title: &str, which: Reveal, add: &mut dyn FnMut(&mut egui::Ui)| {
        super::track::section_at(ui, title, false, reveal == Some(which), |ui| add(ui));
    };
    if !d.credits.is_empty() {
        section(ui, "Credits", Reveal::Credits, &mut |ui| {
            for c in &d.credits {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.role).color(P.faint));
                    let name = c.alias.clone().unwrap_or_else(|| c.artist_name.clone());
                    if widgets::link(ui, &name, font(14.0, medium()), P.text) {
                        shell.act(Action::Go(crate::ui::Page::Artist(c.artist_id)));
                    }
                });
            }
        });
    }
    if d.covers.len() > 1 {
        section(ui, &format!("Covers ({})", d.covers.len()), Reveal::Covers, &mut |ui| {
            ui.horizontal_wrapped(|ui| {
                for c in &d.covers {
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(140.0), Sense::click());
                    shell.cover(ui, c.cover(), c.id, rect, 8, 0.2);
                    let size = match (c.width, c.height) {
                        (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                        _ => c.source.clone(),
                    };
                    let response = response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text(format!("{size} · click: the original"));
                    if response.clicked()
                        && let Some(cover) = c.cover()
                    {
                        shell.open_original(ui.ctx(), cover);
                    }
                }
            });
        });
    }
    if !d.sources.is_empty() {
        section(ui, &format!("Sources ({})", d.sources.len()), Reveal::Sources, &mut |ui| super::track::sources(ui, &d.sources));
    }
}

/// The album's form (edit mode). Kept in the shell per album, so changes not saved yet
/// are still there after looking elsewhere.
fn edit_panel(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, reveal: bool) {
    use crate::ui::edit::album_form::AlbumForm;
    let cached = shell.album_details.get(id);
    let Some(d) = cached.value.clone() else {
        ui.label(RichText::new(if cached.refreshing { "Loading the album to edit…" } else { "Couldn't load the album to edit" }).color(P.muted));
        return;
    };
    panel(ui, "album-edit", |ui| {
        if reveal {
            ui.scroll_to_cursor(Some(egui::Align::Min));
        }
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let Some(mut form) = shell.album_forms.remove(&id).or_else(|| AlbumForm::from_detail(&d)) else {
            ui.label(RichText::new("This album has no revision to build on, so it can't be edited here.").color(P.muted));
            return;
        };
        form.show(ui, shell, Some(catalog), &d);
        shell.album_forms.insert(id, form);
    });
}


/// A star button and a rating for a track, album or artist (the header's).
pub fn marks(ui: &mut egui::Ui, shell: &mut Shell, kind: Kind, id: Uuid) {
    let mark = shell.mark(id);
    ui.spacing_mut().item_spacing.x = 12.0;
    if widgets::star_button(ui, mark.starred, 26.0) {
        shell.act(Action::ToggleStar(kind, id));
    }
    if let Some(rating) = widgets::rating(ui, mark.rating, 20.0) {
        shell.act(Action::Rate(kind, id, rating));
    }
    if mark.play_count > 0 {
        let plays = if mark.play_count == 1 { "1 play".to_string() } else { format!("{} plays", crate::ui::thousands(mark.play_count as usize)) };
        ui.label(RichText::new(plays).font(font(12.5, medium())).color(P.faint));
    }
}

/// A panel on a page that scrolls as a whole (album, artist): framed, as tall as what's in
/// it (no scrolling of its own, which would catch the page's).
pub fn panel(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.push_id(id, add);
    });
}

/// A panel for details on a page that doesn't scroll as a whole: framed, scrolling if long.
pub fn details_panel(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id).max_height(300.0).show(ui, add);
    });
}

/// The big round-cornered Play button.
pub fn play_button(ui: &mut egui::Ui, id: &str, rect: Rect) -> bool {
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    ui.painter().rect_filled(rect.expand(hover * 2.0), 21, P.accent);
    widgets::draw_icon(ui.painter(), widgets::Icon::Play, rect.left_center() + Vec2::new(28.0, 0.0), 7.0, P.bg);
    ui.painter().text(rect.left_center() + Vec2::new(44.0, 0.0), Align2::LEFT_CENTER, "Play", font(14.0, semibold()), P.bg);
    response.clicked()
}

