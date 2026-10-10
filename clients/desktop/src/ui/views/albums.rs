//! All albums as a grid of covers: sorted (added, title, artist, year), filtered as you
//! type. Keys: arrows or hjkl move, Enter opens, Shift+Enter plays.

use std::sync::Arc;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Shadow, Stroke, StrokeKind, Vec2};

use crate::catalog::index::AlbumSort;
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

pub struct Albums {
    filter: String,
    sort: AlbumSort,
    /// The albums shown, in order (positions in the catalog).
    list: Vec<u32>,
    /// What `list` was made from: the catalog's generation (None: not made yet).
    made_for: Option<u64>,
    selected: usize,
    scroll_to: Option<f32>,
    offset: f32,
}

impl Default for Albums {
    fn default() -> Self {
        Albums { filter: String::new(), sort: AlbumSort::Added, list: Vec::new(), made_for: None, selected: 0, scroll_to: None, offset: 0.0 }
    }
}

const SORTS: [(&str, AlbumSort); 4] =
    [("Recently added", AlbumSort::Added), ("Title", AlbumSort::Title), ("Artist", AlbumSort::Artist), ("Year", AlbumSort::Year)];

impl Albums {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        let ctx = ui.ctx().clone();
        let mut remake = self.made_for != Some(shell.generation);

        super::heading(ui, "Albums", self.list.len());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if super::filter(ui, &mut self.filter, "Filter: words, or artist:… year:… kind:ep…") {
                remake = true;
                self.selected = 0;
                self.scroll_to = Some(0.0);
            }
            ui.add_space(12.0);
            for (label, sort) in SORTS {
                if widgets::chip(ui, label, self.sort == sort) && self.sort != sort {
                    self.sort = sort;
                    remake = true;
                    self.selected = 0;
                    self.scroll_to = Some(0.0);
                }
            }
        });
        ui.add_space(14.0);
        if remake {
            self.list = catalog.album_list(&self.filter, self.sort);
            self.made_for = Some(shell.generation);
        }
        if self.list.is_empty() {
            ui.label(egui::RichText::new("No albums").color(P.muted));
            return;
        }

        let gap = 22.0;
        let width = ui.available_width() - 8.0;
        let columns = (((width + gap) / (180.0 + gap)).floor() as usize).max(1);
        let card = ((width - gap * (columns - 1) as f32) / columns as f32).min(240.0);
        let row_height = card + 58.0;
        let rows = self.list.len().div_ceil(columns);
        let visible_rows = ((ui.available_height() / row_height) as usize).max(1);

        let moved = widgets::move_selection(&ctx, &mut self.selected, self.list.len(), columns, visible_rows);
        if moved.moved {
            // Keep the selection on screen.
            let row_top = (self.selected / columns) as f32 * row_height;
            let view_height = ui.available_height();
            if row_top < self.offset {
                self.scroll_to = Some(row_top);
            } else if row_top + row_height > self.offset + view_height {
                self.scroll_to = Some(row_top + row_height - view_height);
            }
        }
        if let Some(&index) = self.list.get(self.selected) {
            album_keys(shell, catalog, &moved, catalog.album_at(index).id());
        }

        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("albums");
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset);
        }
        let output = scroll.show_rows(ui, row_height, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for column in 0..columns {
                        let n = row * columns + column;
                        let Some(&index) = self.list.get(n) else { break };
                        let album = catalog.album_at(index);
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(card, row_height - 8.0), Sense::click());
                        let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.16);
                        let cover_rect = Rect::from_min_size(rect.min - Vec2::new(0.0, hover * 5.0), Vec2::splat(card));
                        shell.cover(ui, album.cover(), album.id(), cover_rect, 10, hover);
                        let star = ctx.animate_bool_with_time(egui::Id::new(("album-star", album.id())), shell.mark(album.id()).starred, 0.18);
                        if star > 0.0 {
                            let badge = cover_rect.left_top() + Vec2::splat(18.0);
                            ui.painter().circle_filled(badge, 12.0 * star, Color32::from_black_alpha((140.0 * star) as u8));
                            widgets::draw_star(ui.painter(), badge, 7.0 * star, P.accent.gamma_multiply(star));
                        }
                        if n == self.selected {
                            ui.painter().rect_stroke(cover_rect.expand(3.0), 12, Stroke::new(2.0, P.accent), StrokeKind::Outside);
                        }
                        // A play button fades in on hover.
                        let button_center = cover_rect.right_bottom() - Vec2::splat(30.0);
                        let over_button = response.hover_pos().is_some_and(|p| p.distance(button_center) < 22.0);
                        if hover > 0.01 {
                            ui.painter().add(
                                Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha((120.0 * hover) as u8) }
                                    .as_shape(Rect::from_center_size(button_center, Vec2::splat(44.0)), 22),
                            );
                            ui.painter().circle_filled(button_center, 20.0 + if over_button { 2.0 } else { 0.0 }, P.accent.gamma_multiply(hover));
                            widgets::draw_icon(ui.painter(), widgets::Icon::Play, button_center, 9.0, P.bg.gamma_multiply(hover));
                        }
                        let text_top = cover_rect.bottom() + 10.0 + hover * 5.0;
                        widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), text_top), Vec2::new(card, 18.0)), album.title(), font(13.5, semibold()), P.text);
                        let sub = match album.year() {
                            Some(year) => format!("{} · {year}", album.artist_display()),
                            None => album.artist_display().to_string(),
                        };
                        widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), text_top + 19.0), Vec2::new(card, 16.0)), &sub, font(12.0, medium()), P.muted);
                        widgets::share_menu(&response, shell, crate::links::Target::Album(album.id()));
                        if n == self.selected {
                            shell.list_target = Some(crate::links::Target::Album(album.id()));
                        }
                        if response.clicked() {
                            self.selected = n;
                            if over_button {
                                play_album(shell, catalog, album.id());
                            } else {
                                shell.act(Action::Go(Page::Album(album.id())));
                            }
                        }
                    }
                });
            }
        });
        self.offset = output.state.offset.y;
    }
}

/// What the keys asked for the album selected in a grid (`widgets::move_selection`):
/// star, rate, queue, add to a playlist, Enter opens it (Shift+Enter plays it).
pub fn album_keys(shell: &mut Shell, catalog: &Catalog, moved: &widgets::Moved, id: uuid::Uuid) {
    if moved.star {
        shell.act(Action::ToggleStar(Kind::Album, id));
    }
    if moved.rate_by != 0 {
        shell.act(Action::RateBy(Kind::Album, id, moved.rate_by));
    }
    if let Some(next) = moved.queue {
        shell.act(Action::Enqueue { tracks: catalog.album_tracks(id).to_vec(), next });
    }
    if moved.add {
        let tracks = catalog.album_tracks(id).iter().map(|&t| catalog.track_at(t).id()).collect();
        shell.act(Action::AddToPlaylist(tracks));
    }
    if moved.enter {
        if moved.shift {
            play_album(shell, catalog, id);
        } else {
            shell.act(Action::Go(Page::Album(id)));
        }
    }
}

/// Plays an album from its first track.
pub fn play_album(shell: &mut Shell, catalog: &Catalog, album: uuid::Uuid) {
    let tracks = catalog.album_tracks(album).to_vec();
    if !tracks.is_empty() {
        shell.act(Action::Play { tracks, start: 0 });
    }
}
