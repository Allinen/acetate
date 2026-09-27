//! A table of tracks with a filter, used for all tracks, playlists and Liked songs: click
//! a column's name to sort by it (again: reversed), type to filter (title, artists,
//! album). Only the rows on screen are drawn, so 185k tracks scroll like 100.
//!
//! Keys: arrows or jk move, Enter plays from there (in the order shown), `s` stars, `+`/`-`
//! rate, `p` adds to a playlist, `/` filters. A playlist's page adds its own keys, using
//! `Shown::selected`.

use std::time::Duration;

use eframe::egui::{self, Label, RichText, Sense, Vec2};
use egui_extras::{Column, TableBuilder};

use crate::catalog::index::{TrackRows, TrackSort};
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, thousands, widgets, Action, Shell};

/// How many tracks to queue when playing from the table (from the one chosen on).
const QUEUE: usize = 1000;
const ROW: f32 = 36.0;

/// What a table shows besides the usual columns.
pub struct Options {
    /// The rows' own order has a meaning (a playlist's): a "#" column sorts by it.
    pub numbered: bool,
    /// A column for when each row was added, with this name ("Added", "Starred").
    pub added: Option<&'static str>,
    pub filter_hint: &'static str,
}

/// What the table reports back.
pub struct Shown {
    /// The selected row (a position in the rows given), if any.
    pub selected: Option<usize>,
    /// The rows are shown in their own order, unfiltered (moving one makes sense).
    pub in_own_order: bool,
}

pub struct TrackTable {
    filter: String,
    sort: TrackSort,
    descending: bool,
    /// The rows shown, in order (positions in the rows given).
    order: Vec<usize>,
    /// What `order` was made from (the caller's version of the rows).
    made_for: Option<u64>,
    selected: usize,
    scroll_to: Option<usize>,
}

impl TrackTable {
    pub fn new(sort: TrackSort) -> TrackTable {
        TrackTable { filter: String::new(), sort, descending: false, order: Vec::new(), made_for: None, selected: 0, scroll_to: None }
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

    /// `version`: changes whenever the rows do (a new catalog, a changed playlist).
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
        if super::filter(ui, &mut self.filter, options.filter_hint) {
            remake = true;
            self.selected = 0;
            self.scroll_to = Some(0);
        }
        ui.add_space(10.0);
        if remake {
            self.remake(catalog, rows, version);
        }

        let visible = ((ui.available_height() / ROW) as usize).max(1);
        let moved = widgets::move_selection(&ctx, &mut self.selected, self.order.len(), 1, visible);
        if moved.moved {
            self.scroll_to = Some(self.selected);
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
            if moved.enter {
                self.play_from(shell, rows, self.selected);
            }
        }

        let playing = playing_track(player);
        let mut builder = TableBuilder::new(ui).sense(Sense::click()).animate_scrolling(false);
        if options.numbered {
            builder = builder.column(Column::exact(40.0));
        }
        builder = builder
            .column(Column::exact(36.0))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::initial(220.0).clip(true))
            .column(Column::initial(200.0).clip(true))
            .column(Column::exact(92.0))
            .column(Column::exact(52.0))
            .column(Column::exact(52.0))
            .column(Column::exact(56.0));
        if options.added.is_some() {
            builder = builder.column(Column::exact(96.0));
        }
        if let Some(row) = self.scroll_to.take() {
            builder = builder.scroll_to_row(row, None);
        }

        // (name, what it sorts by; None: it doesn't)
        let mut columns: Vec<(&str, Option<TrackSort>)> = Vec::new();
        if options.numbered {
            columns.push(("#", Some(TrackSort::Natural)));
        }
        columns.extend([
            ("", None),
            ("Title", Some(TrackSort::Title)),
            ("Artist", Some(TrackSort::Artist)),
            ("Album", Some(TrackSort::Album)),
            ("Released", Some(TrackSort::Year)),
            ("Plays", Some(TrackSort::Plays)),
            ("Format", None),
            ("Length", Some(TrackSort::Length)),
        ]);
        if let Some(added) = options.added {
            columns.push((added, Some(TrackSort::Added)));
        }

        let mut sort_by = None;
        let (mut clicked, mut double) = (None, None);
        builder
            .header(26.0, |mut header| {
                for (name, sort) in &columns {
                    header.col(|ui| {
                        let Some(sort) = sort else {
                            ui.label(RichText::new(*name).font(font(12.0, semibold())).color(P.faint));
                            return;
                        };
                        let active = self.sort == *sort;
                        let arrow = if !active { "" } else if self.descending { " ↑" } else { " ↓" };
                        let color = if active { P.accent } else { P.faint };
                        let label = ui.add(Label::new(RichText::new(format!("{name}{arrow}")).font(font(12.0, semibold())).color(color)).sense(Sense::click()));
                        if label.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            sort_by = Some(*sort);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(ROW, self.order.len(), |mut row| {
                    let n = row.index();
                    let at = self.order[n];
                    let t = &catalog.tracks[rows.track(at) as usize];
                    row.set_selected(n == self.selected);
                    let color = if playing == Some(t.id) { P.accent } else { P.text };
                    if options.numbered {
                        row.col(|ui| {
                            ui.label(RichText::new((at + 1).to_string()).font(font(12.5, medium())).color(P.faint));
                        });
                    }
                    row.col(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        shell.cover(ui, t.cover_id, t.id, rect, 4, 0.0);
                    });
                    row.col(|ui| {
                        // The star on the title's line, not above it.
                        ui.horizontal(|ui| {
                            if shell.mark(t.id).starred {
                                let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), Sense::hover());
                                widgets::draw_star(ui.painter(), rect.center(), 5.5, P.accent);
                            }
                            ui.add(Label::new(RichText::new(&t.title).color(color).font(font(13.5, medium()))).truncate());
                        });
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&t.artist_display).color(P.muted)).truncate());
                    });
                    row.col(|ui| {
                        let album = catalog.main_album(t).map(|a| a.title.as_str()).unwrap_or("");
                        ui.add(Label::new(RichText::new(album).color(P.muted)).truncate());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(t.release_date.as_deref().unwrap_or("")).color(P.faint));
                    });
                    row.col(|ui| {
                        let plays = shell.mark(t.id).play_count;
                        let text = if plays > 0 { thousands(plays as usize) } else { String::new() };
                        ui.label(RichText::new(text).color(P.faint));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(t.codec.as_deref().unwrap_or("")).color(P.faint).small());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(widgets::length(t.duration_ms)).color(P.muted));
                    });
                    if options.added.is_some() {
                        row.col(|ui| {
                            let added = rows.added.and_then(|a| a.get(at)).map(|a| a.get(..10).unwrap_or(a)).unwrap_or("");
                            ui.label(RichText::new(added).color(P.faint));
                        });
                    }
                    let response = row.response();
                    if response.double_clicked() {
                        double = Some(n);
                    } else if response.clicked() {
                        clicked = Some(n);
                    }
                });
            });

        if let Some(sort) = sort_by {
            if self.sort == sort {
                self.descending = !self.descending;
            } else {
                self.sort = sort;
                self.descending = false;
            }
            self.selected = 0;
            self.scroll_to = Some(0);
            self.remake(catalog, rows, version);
            ctx.request_repaint();
        }
        if let Some(n) = clicked {
            self.selected = n;
        }
        if let Some(n) = double {
            self.selected = n;
            self.play_from(shell, rows, n);
        }
        if self.order.is_empty() {
            ui.label(RichText::new(if self.filter.is_empty() { "No tracks" } else { "Nothing matches the filter" }).color(P.muted));
        }
        Shown {
            selected: self.order.get(self.selected).copied(),
            in_own_order: self.sort == TrackSort::Natural && !self.descending && self.filter.trim().is_empty(),
        }
    }

    fn remake(&mut self, catalog: &Catalog, rows: &TrackRows, version: u64) {
        self.order = catalog.order_tracks(rows, &self.filter, self.sort, self.descending);
        self.made_for = Some(version);
        self.selected = self.selected.min(self.order.len().saturating_sub(1));
    }

    fn play_from(&self, shell: &mut Shell, rows: &TrackRows, n: usize) {
        let tracks: Vec<u32> = self.order.iter().skip(n).take(QUEUE).map(|&r| rows.track(r)).collect();
        shell.act(Action::Play { tracks, start: 0 });
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
