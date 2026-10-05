//! The table of tracks every track list uses: all tracks, playlists, Liked songs, an
//! album's tracks, an artist's. Each list says which columns it can have (and which show
//! to start with); right-click a column's name to show or hide the others (kept per kind
//! of list). Click a column's name to sort by it (again: reversed); type to filter
//! (title, artists, album), where the list has a filter. Only the rows on screen are
//! drawn, so 185k tracks scroll like 100.
//!
//! On an album: the track number, as disc-track ("2-5") when any track is on a disc
//! other than the first, with a row naming each disc between them (in the album's own
//! order); the artist only where it isn't the album's.
//!
//! Keys: arrows or jk move, Enter plays from there (in the order shown), `s` stars, `+`/`-`
//! rate, `p` adds to a playlist, `/` filters. A playlist's page adds its own keys, using
//! `Shown::selected`.
//!
//! Marking many, in edit mode (listen mode has none of it): the box in front of each row
//! (a click), `x`, or Ctrl+click marks or unmarks one; Shift+click or Shift+↑/↓ a range,
//! which takes the state of the row it starts from (so ranges unmark too); Ctrl+A marks
//! everything shown; Esc clears. Marks are kept by track, so sorting and filtering don't
//! lose them (nor leaving edit mode: they're there again when it's back). Ctrl+E (or the
//! button) opens the change for all of them, in a popup (see `edit::editor`). Ctrl+M with
//! two marked compares them side by side, to merge (see `merge`), in a popup too. (All of
//! it is `marking`'s, shared with Review's list.)

use std::collections::HashMap;
use std::time::Duration;

use eframe::egui::{self, Label, RichText, Sense, Vec2};
use egui_extras::{Column as Width, TableBuilder};
use uuid::Uuid;

use crate::catalog::index::{TrackRows, TrackSort};
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::edit::track_order::Place;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, thousands, widgets, Action, Shell};

/// A row's height (rows touch: the space between them is inside it, see `show`).
pub const ROW: f32 = 42.0;

/// A column a list can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// The row's place in the list's own order (a playlist's).
    Position,
    /// The track's number on the album (an album's list).
    Number,
    Cover,
    Title,
    Artist,
    Album,
    Released,
    Plays,
    Format,
    /// Whether its file is an original file (an og file): the OG badge.
    Og,
    Length,
    /// When it was added (to the playlist; starred, for Liked songs).
    Added,
}

impl Column {
    /// Its name in the settings (where the columns shown are kept).
    fn key(self) -> &'static str {
        match self {
            Column::Position => "position",
            Column::Number => "number",
            Column::Cover => "cover",
            Column::Title => "title",
            Column::Artist => "artist",
            Column::Album => "album",
            Column::Released => "released",
            Column::Plays => "plays",
            Column::Format => "format",
            Column::Og => "og",
            Column::Length => "length",
            Column::Added => "added",
        }
    }

    fn name(self, options: &Options) -> &'static str {
        match self {
            Column::Position | Column::Number => "#",
            Column::Cover => "",
            Column::Title => "Title",
            Column::Artist => "Artist",
            Column::Album => "Album",
            Column::Released => "Released",
            Column::Plays => "Plays",
            Column::Format => "Format",
            Column::Og => "OG",
            Column::Length => "Length",
            Column::Added => options.added.unwrap_or("Added"),
        }
    }

    /// Its name in the show/hide menu.
    fn menu_name(self, options: &Options) -> &'static str {
        match self {
            Column::Position => "Position",
            Column::Number => "Track number",
            Column::Cover => "Cover",
            Column::Og => "Original file (OG)",
            other => other.name(options),
        }
    }

    fn sort(self) -> Option<TrackSort> {
        match self {
            Column::Position | Column::Number => Some(TrackSort::Natural),
            Column::Cover | Column::Format => None,
            Column::Title => Some(TrackSort::Title),
            Column::Artist => Some(TrackSort::Artist),
            Column::Album => Some(TrackSort::Album),
            Column::Released => Some(TrackSort::Year),
            Column::Plays => Some(TrackSort::Plays),
            Column::Length => Some(TrackSort::Length),
            Column::Og => Some(TrackSort::Og),
            Column::Added => Some(TrackSort::Added),
        }
    }

    fn width(self) -> Width {
        match self {
            Column::Position => Width::exact(40.0),
            Column::Number => Width::exact(48.0),
            Column::Cover => Width::exact(36.0),
            Column::Title => Width::remainder().at_least(200.0).clip(true),
            Column::Artist => Width::initial(220.0).clip(true),
            Column::Album => Width::initial(200.0).clip(true),
            Column::Released => Width::exact(92.0),
            Column::Plays => Width::exact(52.0),
            Column::Format => Width::exact(52.0),
            Column::Og => Width::exact(40.0),
            Column::Length => Width::exact(56.0),
            Column::Added => Width::exact(96.0),
        }
    }
}

/// What a list is.
pub struct Options<'a> {
    /// Which kind of list (the columns shown are kept under it): "tracks", "playlist"…
    pub key: &'static str,
    /// The columns it can have, in order, and whether each shows to start with.
    pub columns: &'static [(Column, bool)],
    /// The Added column's name ("Added", "Starred").
    pub added: Option<&'static str>,
    /// A filter field, with this hint (None: none).
    pub filter_hint: Option<&'static str>,
    /// An album's list: the album and its artists (for numbers, discs, and the artist
    /// column showing only other ones).
    pub album: Option<(Uuid, &'a str)>,
    /// On an album: tracks moved, their places as they're to be (not in the library yet),
    /// shown in place of theirs (in the accent colour).
    pub places: Option<&'a HashMap<Uuid, Place>>,
    /// It takes the keys (arrows, Enter, marking…): on a page with several tables, only
    /// the one in use does.
    pub keys: bool,
    /// It scrolls on its own (in the room it's given). Off: it's as tall as its rows, for
    /// a page that scrolls as a whole (a table scrolling inside a scrolling page catches
    /// the wheel, and the page can't be scrolled past it). Only the rows on screen are
    /// drawn either way.
    pub scroll: bool,
}

/// What the table reports back.
pub struct Shown {
    /// The selected row (a position in the rows given), if any.
    pub selected: Option<usize>,
    /// The rows are shown in their own order, unfiltered (moving one makes sense).
    pub in_own_order: bool,
    /// A row was clicked (on a page with several tables, it becomes the one in use).
    pub clicked: bool,
}

/// A row of the table: a track (its place in `order`), or a disc's name before its
/// tracks (an album's, in its own order).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Line {
    Track(usize),
    Disc(i16),
}

pub struct TrackTable {
    filter: String,
    sort: TrackSort,
    descending: bool,
    /// The rows shown, in order (positions in the rows given).
    order: Vec<usize>,
    /// The table's lines: `order`, with disc names between discs where they show.
    lines: Vec<Line>,
    /// What `order` was made from (the caller's version of the rows).
    made_for: Option<u64>,
    selected: usize,
    scroll_to: Option<usize>,
    /// Marked tracks and the tools for them (edit mode; Ctrl+E on the selected track
    /// with nothing marked, in listen mode too).
    marking: super::marking::Marking,
}

/// In the rows' own order.
impl Default for TrackTable {
    fn default() -> Self {
        TrackTable::new(TrackSort::Natural)
    }
}

impl TrackTable {
    pub fn new(sort: TrackSort) -> TrackTable {
        TrackTable {
            filter: String::new(),
            sort,
            descending: false,
            order: Vec::new(),
            lines: Vec::new(),
            made_for: None,
            selected: 0,
            scroll_to: None,
            marking: Default::default(),
        }
    }

    /// The filter set to `text` (the rows follow at once).
    pub fn set_filter(&mut self, text: &str) {
        self.filter = text.to_string();
        self.made_for = None;
    }

    /// Tracks are marked (Esc clears them).
    pub fn has_marks(&self) -> bool {
        !self.marking.is_empty()
    }

    /// The marked tracks' popup is open (it takes the keys).
    pub fn popup_open(&self) -> bool {
        self.marking.popup_open()
    }

    /// Ctrl+M is the table's (merging the marked tracks) rather than the page's (merging
    /// the album or artist itself): in edit mode, with tracks marked or the popup open.
    pub fn takes_ctrl_m(&self, shell: &Shell) -> bool {
        shell.edit_mode && self.marking.takes_ctrl_m()
    }

    /// How many rows are shown (after the filter).
    pub fn shown(&self) -> usize {
        self.order.len()
    }

    /// The tracks shown, in order (for a Play button).
    pub fn tracks(&self, rows: &TrackRows) -> Vec<u32> {
        self.order.iter().map(|&r| rows.track(r)).collect()
    }

    /// Selects the shown row `n`.
    pub fn select(&mut self, n: usize) {
        self.selected = n;
        self.scroll_to = Some(n);
    }

    /// The columns shown: the user's choice for this kind of list, else the list's own.
    fn columns(shell: &Shell, options: &Options) -> Vec<Column> {
        match shell.settings.table_columns.get(options.key) {
            Some(keys) => options.columns.iter().map(|(c, _)| *c).filter(|c| *c == Column::Title || keys.iter().any(|k| k == c.key())).collect(),
            None => options.columns.iter().filter(|(_, on)| *on).map(|(c, _)| *c).collect(),
        }
    }

    /// `version`: changes whenever the rows do (a new catalog, a changed playlist).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        shell: &mut Shell,
        catalog: &Catalog,
        rows: &TrackRows,
        version: u64,
        player: &PlayerView,
        options: &Options,
    ) -> Shown {
        let ctx = ui.ctx().clone();
        let mut remake = self.made_for != Some(version);
        if let Some(hint) = options.filter_hint {
            if super::filter(ui, &mut self.filter, hint) {
                remake = true;
                self.selected = 0;
                self.scroll_to = Some(0);
            }
            ui.add_space(10.0);
        }
        if remake {
            self.remake(catalog, rows, version, options);
        }

        // ---- Marking (edit mode only) ----
        let marking = shell.edit_mode;
        let order = &self.order;
        let id_at = |n: usize| order.get(n).map(|&row| catalog.tracks[rows.track(row) as usize].id);
        if !marking {
            self.marking.close_popups();
        }
        self.marking.popups(&ctx, shell, player, order.len(), id_at, options.album.map(|a| a.0));
        let typing = ctx.egui_wants_keyboard_input();
        // Ctrl+E with nothing marked (or outside edit mode): the popup for the selected
        // track alone, a quick fix.
        if options.keys && !self.marking.popup_open() && !typing && !shell.palette_open && (!marking || self.marking.is_empty()) && shell.account.can("edit_metadata") {
            let id = id_at(self.selected);
            if let Some(id) = id
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::E))
            {
                self.marking.open_quick(id);
            }
        }
        if marking && options.keys {
            self.marking.keys(&ctx, shell, self.selected, order.len(), id_at);
        }
        if marking {
            self.marking.bar(ui, shell, catalog, order.len(), id_at, options.key);
        }
        let popup = self.marking.popup_open();

        // ---- Keys for the row under the cursor ----
        // (At most a window's worth: in a page that scrolls, the room below is endless.)
        let visible = ((ui.available_height().min(ctx.content_rect().height()) / ROW) as usize).max(1);
        let before = self.selected;
        // (The popup takes the keys while it's open.)
        let len = if popup || !options.keys { 0 } else { self.order.len() };
        let moved = widgets::move_selection(&ctx, &mut self.selected, len, 1, visible);
        if moved.moved {
            self.scroll_to = Some(self.selected);
            // Shift+↑/↓ (edit mode): the rows passed over take the starting row's state.
            if marking && ctx.input(|i| i.modifiers.shift) {
                let order = &self.order;
                self.marking.range_to(before, self.selected, |n| order.get(n).map(|&row| catalog.tracks[rows.track(row) as usize].id));
            }
        }
        if let Some(&row) = self.order.get(self.selected) {
            let id = catalog.tracks[rows.track(row) as usize].id;
            if moved.star {
                shell.act(Action::ToggleStar(Kind::Track, id));
            }
            if moved.rate_by != 0 {
                shell.act(Action::RateBy(Kind::Track, id, moved.rate_by));
            }
            if moved.add {
                shell.act(Action::AddToPlaylist(vec![id]));
            }
            if let Some(next) = moved.queue {
                shell.act(Action::Enqueue { tracks: vec![rows.track(row)], next });
            }
            if moved.info {
                shell.act(Action::Go(crate::ui::Page::Track(id)));
            }
            if moved.enter {
                self.play_from(shell, rows, self.selected);
            }
        }

        // ---- The table ----
        let columns = TrackTable::columns(shell, options);
        let playing = playing_track(player);
        // On an album: numbers as disc-track when any track is on another disc.
        let numbers = options.album.map(|(album, _)| AlbumNumbers::new(catalog, rows, album, options.places));
        // Its own id (by the kind of list): tables side by side don't share a scroll.
        // No gap between rows (it's in `ROW`): the row's background is painted half a gap
        // past each side, and the first row's top half would be cut off by the table's top.
        let spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        // Each cell's contents centred on the row's height.
        let mut builder = TableBuilder::new(ui)
            .id_salt(options.key)
            .sense(Sense::click())
            .animate_scrolling(false)
            .vscroll(options.scroll)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if !options.scroll {
            // All of it: its height isn't capped (the page around it scrolls).
            builder = builder.max_scroll_height(f32::INFINITY);
        }
        if marking {
            builder = builder.column(Width::exact(26.0));
        }
        for c in &columns {
            builder = builder.column(c.width());
        }
        if let Some(row) = self.scroll_to.take() {
            let line = self.lines.iter().position(|l| *l == Line::Track(row)).unwrap_or(row);
            builder = builder.scroll_to_row(line, None);
        }

        let mut sort_by = None;
        let mut shown_columns_changed: Option<Vec<Column>> = None;
        let (mut clicked, mut double, mut boxed) = (None, None, None);
        builder
            // 2 px more than the labels need: a little air above the first row.
            .header(28.0, |mut header| {
                if marking {
                    header.col(|_| {});
                }
                for c in &columns {
                    let (_, response) = header.col(|ui| {
                        let name = c.name(options);
                        let Some(sort) = c.sort() else {
                            ui.label(RichText::new(name).font(font(12.0, semibold())).color(P.faint));
                            return;
                        };
                        let active = self.sort == sort;
                        let arrow = if !active { "" } else if self.descending { " ↑" } else { " ↓" };
                        let color = if active { P.accent } else { P.faint };
                        let label = ui.add(Label::new(RichText::new(format!("{name}{arrow}")).font(font(12.0, semibold())).color(color)).sense(Sense::click()));
                        if label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            sort_by = Some(sort);
                        }
                    });
                    // Right-click: which columns show.
                    response.context_menu(|ui| {
                        ui.label(RichText::new("Columns").color(P.faint));
                        for (candidate, _) in options.columns {
                            if *candidate == Column::Title {
                                continue;
                            }
                            let mut on = columns.contains(candidate);
                            if ui.checkbox(&mut on, candidate.menu_name(options)).changed() {
                                let mut next: Vec<Column> = columns.clone();
                                if on {
                                    next.push(*candidate);
                                } else {
                                    next.retain(|c| c != candidate);
                                }
                                shown_columns_changed = Some(next);
                            }
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, self.lines.len(), |mut row| {
                    let line = self.lines[row.index()];
                    let n = match line {
                        Line::Track(n) => n,
                        Line::Disc(disc) => {
                            // A disc's name, across the row (drawn unclipped from the first cell).
                            row.col(|ui| {
                                let painter = ui.ctx().layer_painter(ui.layer_id());
                                let at = ui.max_rect().left_center() + Vec2::new(2.0, 4.0);
                                painter.text(at, egui::Align2::LEFT_CENTER, format!("Disc {disc}"), font(12.5, semibold()), P.muted);
                            });
                            for _ in columns.iter().skip(if marking { 0 } else { 1 }) {
                                row.col(|_| {});
                            }
                            return;
                        }
                    };
                    let at = self.order[n];
                    let t = &catalog.tracks[rows.track(at) as usize];
                    row.set_selected(n == self.selected);
                    let marked = self.marking.contains(&t.id);
                    let color = if playing == Some(t.id) { P.accent } else { P.text };
                    // The mark's box (edit mode): a click (un)marks the row.
                    if marking {
                        row.col(|ui| {
                        let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        super::marking::paint_box(ui.painter(), rect.translate(Vec2::new(2.0, 0.0)), marked, response.hovered());
                        if response.on_hover_text("Mark (x)").clicked() {
                            boxed = Some(n);
                        }
                        });
                    }
                    for c in &columns {
                        row.col(|ui| match c {
                            Column::Position => {
                                ui.label(RichText::new((at + 1).to_string()).font(font(12.5, medium())).color(P.faint));
                            }
                            Column::Number => {
                                let text = numbers.as_ref().map(|nb| nb.label(t)).unwrap_or_default();
                                // Moved, not in the library yet: in the accent colour.
                                let moved = numbers.as_ref().is_some_and(|nb| nb.moved(t));
                                ui.label(RichText::new(text).font(font(12.5, medium())).color(if moved { P.accent } else { P.faint }));
                            }
                            Column::Cover => {
                                // Centred in the whole cell (not at its top left, where the
                                // cell's edge and the table's top cut it off).
                                let cell = ui.max_rect();
                                ui.allocate_rect(cell, Sense::hover());
                                let rect = egui::Rect::from_center_size(cell.center(), Vec2::splat(28.0));
                                shell.cover(ui, t.cover(), t.id, rect, 4, 0.0);
                                // An original file's track: an amber edge (inside the cover,
                                // so nothing clips it).
                                if t.original_filename.is_some() {
                                    widgets::og_edge(ui.painter(), rect, 4, 1.5);
                                }
                            }
                            Column::Title => {
                                // The star on the title's line, not above it.
                                ui.horizontal(|ui| {
                                    if shell.mark(t.id).starred {
                                        let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
                                        widgets::draw_star(ui.painter(), rect.center(), 5.5, P.accent);
                                    }
                                    let clicked = match &t.original_filename {
                                        Some(name) => widgets::og_link(ui, &t.title, font(13.5, medium()), name),
                                        None => widgets::link(ui, &t.title, font(13.5, medium()), color),
                                    };
                                    if clicked {
                                        shell.act(Action::Go(crate::ui::Page::Track(t.id)));
                                    }
                                });
                            }
                            Column::Artist => {
                                // On an album: only artists other than the album's.
                                let same = options.album.is_some_and(|(_, artist)| artist == t.artist_display);
                                if !same {
                                    ui.horizontal(|ui| {
                                        super::artist_links(ui, shell, catalog, &t.credits, &t.artist_display, font(13.0, medium()), P.muted);
                                    });
                                }
                            }
                            Column::Album => {
                                if let Some(album) = catalog.main_album(t)
                                    && widgets::link(ui, &album.title, font(13.0, medium()), P.muted)
                                {
                                    shell.act(Action::Go(crate::ui::Page::Album(album.id)));
                                }
                            }
                            Column::Released => {
                                ui.label(RichText::new(t.release_date.as_deref().unwrap_or("")).color(P.faint));
                            }
                            Column::Plays => {
                                let plays = shell.mark(t.id).play_count;
                                let text = if plays > 0 { thousands(plays as usize) } else { String::new() };
                                ui.label(RichText::new(text).color(P.faint));
                            }
                            Column::Format => {
                                ui.label(RichText::new(t.codec.as_deref().unwrap_or("")).color(P.faint).small());
                            }
                            Column::Og => {
                                let cell = ui.max_rect();
                                let response = ui.allocate_rect(cell, Sense::hover());
                                if let Some(name) = &t.original_filename {
                                    widgets::og_badge(ui.painter(), cell.left_center() + Vec2::new(4.0, 0.0), 15.0);
                                    response.on_hover_text(format!("Original file · {name}"));
                                }
                            }
                            Column::Length => {
                                ui.label(RichText::new(widgets::length(t.duration_ms)).color(P.muted));
                            }
                            Column::Added => {
                                let added = rows.added.and_then(|a| a.get(at)).map(|a| a.get(..10).unwrap_or(a)).unwrap_or("");
                                ui.label(RichText::new(added).color(P.faint));
                            }
                        });
                    }
                    let response = row.response();
                    widgets::share_menu(&response, shell, crate::links::Target::Track(catalog.tracks[rows.track(self.order[n]) as usize].id));
                    if response.double_clicked() {
                        double = Some(n);
                    } else if response.clicked() {
                        clicked = Some(n);
                    }
                });
            });
        ui.spacing_mut().item_spacing.y = spacing;

        if let Some(next) = shown_columns_changed {
            shell.settings.table_columns.insert(options.key.to_string(), next.iter().map(|c| c.key().to_string()).collect());
            shell.settings.save(&shell.data_dir);
        }
        if let Some(sort) = sort_by {
            if self.sort == sort {
                self.descending = !self.descending;
            } else {
                self.sort = sort;
                self.descending = false;
            }
            self.selected = 0;
            self.scroll_to = Some(0);
            self.remake(catalog, rows, version, options);
            ctx.request_repaint();
        }
        let modifiers = ctx.input(|i| i.modifiers);
        let order = &self.order;
        let id_at = |n: usize| order.get(n).map(|&row| catalog.tracks[rows.track(row) as usize].id);
        if let Some(n) = boxed {
            self.marking.toggle(n, id_at);
            self.selected = n;
        } else if let Some(n) = clicked {
            // (Listen mode: a click only moves the cursor.)
            if marking {
                self.marking.click(n, self.selected, modifiers, id_at);
            }
            self.selected = n;
        }
        if let Some(n) = double {
            self.selected = n;
            self.play_from(shell, rows, n);
        }
        if self.order.is_empty() {
            ui.label(RichText::new(if self.filter.is_empty() { "No tracks" } else { "Nothing matches the filter" }).color(P.muted));
        }
        // Ctrl+L on a list: the selected track's link.
        if options.keys
            && let Some(&row) = self.order.get(self.selected)
        {
            shell.list_target = Some(crate::links::Target::Track(catalog.tracks[rows.track(row) as usize].id));
        }
        Shown {
            selected: self.order.get(self.selected).copied(),
            in_own_order: self.sort == TrackSort::Natural && !self.descending && self.filter.trim().is_empty(),
            clicked: clicked.is_some() || double.is_some() || boxed.is_some(),
        }
    }

    fn remake(&mut self, catalog: &Catalog, rows: &TrackRows, version: u64, options: &Options) {
        self.order = catalog.order_tracks(rows, &self.filter, self.sort, self.descending);
        self.made_for = Some(version);
        self.selected = self.selected.min(self.order.len().saturating_sub(1));
        // Disc names between discs: an album's, in its own order, when it has several.
        let own_order = self.sort == TrackSort::Natural && !self.descending;
        let discs = options.album.filter(|_| own_order).map(|(album, _)| AlbumNumbers::new(catalog, rows, album, options.places));
        self.lines = match discs.filter(|d| d.several_discs) {
            Some(numbers) => lines_with_discs(&self.order.iter().map(|&r| numbers.disc(&catalog.tracks[rows.track(r) as usize])).collect::<Vec<_>>()),
            None => (0..self.order.len()).map(Line::Track).collect(),
        };
    }

    /// Plays every row shown, starting at `n` (so shuffle mixes all of them, and
    /// Previous goes back up the list).
    fn play_from(&self, shell: &mut Shell, rows: &TrackRows, n: usize) {
        let tracks: Vec<u32> = self.order.iter().map(|&r| rows.track(r)).collect();
        shell.act(Action::Play { tracks, start: n });
    }
}

/// The lines for tracks on these discs (in order): a disc's name before its first track
/// (and before a change of disc).
fn lines_with_discs(discs: &[i16]) -> Vec<Line> {
    let mut lines = Vec::new();
    for (n, disc) in discs.iter().enumerate() {
        if n == 0 || discs[n - 1] != *disc {
            lines.push(Line::Disc(*disc));
        }
        lines.push(Line::Track(n));
    }
    lines
}

/// Tracks' numbers on one album: whether it has several discs, and each track's label.
struct AlbumNumbers<'a> {
    album: Uuid,
    several_discs: bool,
    /// Moved tracks' places as they're to be.
    places: Option<&'a HashMap<Uuid, Place>>,
}

impl<'a> AlbumNumbers<'a> {
    fn new(catalog: &Catalog, rows: &TrackRows, album: Uuid, places: Option<&'a HashMap<Uuid, Place>>) -> AlbumNumbers<'a> {
        let numbers = AlbumNumbers { album, several_discs: false, places };
        let several_discs = (0..rows.len(catalog)).any(|r| numbers.place(&catalog.tracks[rows.track(r) as usize]).is_some_and(|p| p.0.is_some_and(|d| d != 1)));
        AlbumNumbers { several_discs, ..numbers }
    }

    /// The track's disc and number on the album (None: it isn't on it).
    fn place(&self, t: &crate::catalog::model::Track) -> Option<Place> {
        self.places.and_then(|p| p.get(&t.id).copied()).or_else(|| t.albums.iter().find(|m| m.0 == self.album).map(|m| (m.1, m.2)))
    }

    /// The track was moved (its place isn't the library's yet).
    fn moved(&self, t: &crate::catalog::model::Track) -> bool {
        self.places.is_some_and(|p| p.contains_key(&t.id))
    }

    /// The track's disc on the album (1 when it has none).
    fn disc(&self, t: &crate::catalog::model::Track) -> i16 {
        self.place(t).and_then(|p| p.0).unwrap_or(1)
    }

    /// "5", or "2-5" on an album with several discs; "·" without a number.
    fn label(&self, t: &crate::catalog::model::Track) -> String {
        let Some((disc, number)) = self.place(t) else { return String::new() };
        match (number, self.several_discs) {
            (Some(n), true) => format!("{}-{n}", disc.unwrap_or(1)),
            (Some(n), false) => n.to_string(),
            (None, _) => "·".into(),
        }
    }
}

/// "12 tracks · 48:10".
pub fn count_and_length(catalog: &Catalog, tracks: impl Iterator<Item = u32>) -> String {
    let (mut n, mut total) = (0usize, 0i64);
    for t in tracks {
        n += 1;
        total += catalog.tracks[t as usize].duration_ms.unwrap_or(0);
    }
    let count = if n == 1 { "1 track".to_string() } else { format!("{} tracks", thousands(n)) };
    format!("{count} · {}", widgets::clock(Duration::from_millis(total.max(0) as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disc_name_before_each_disc() {
        assert_eq!(
            lines_with_discs(&[1, 1, 2, 2, 3]),
            [Line::Disc(1), Line::Track(0), Line::Track(1), Line::Disc(2), Line::Track(2), Line::Track(3), Line::Disc(3), Line::Track(4)]
        );
        assert!(lines_with_discs(&[]).is_empty());
    }
}
