//! All artists: a list with their other names and how many tracks and albums they have
//! (that you can see), sorted by name or by track count, filtered as you type (names and
//! aliases). Enter opens the artist.

use std::sync::Arc;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};

use crate::catalog::index::ArtistSort;
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

pub struct Artists {
    filter: String,
    sort: ArtistSort,
    list: Vec<u32>,
    made_for: Option<u64>,
    selected: usize,
    scroll_to: Option<usize>,
    offset: f32,
}

impl Default for Artists {
    fn default() -> Self {
        Artists { filter: String::new(), sort: ArtistSort::Name, list: Vec::new(), made_for: None, selected: 0, scroll_to: None, offset: 0.0 }
    }
}

impl Artists {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>) {
        let ctx = ui.ctx().clone();
        let mut remake = self.made_for != Some(shell.generation);
        super::heading(ui, "Artists", self.list.len());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if super::filter(ui, &mut self.filter, "Filter artists (aliases too)") {
                remake = true;
                self.selected = 0;
                self.scroll_to = Some(0);
            }
            ui.add_space(12.0);
            for (label, sort) in [("Name", ArtistSort::Name), ("Most tracks", ArtistSort::Tracks)] {
                if widgets::chip(ui, label, self.sort == sort) && self.sort != sort {
                    self.sort = sort;
                    remake = true;
                    self.selected = 0;
                    self.scroll_to = Some(0);
                }
            }
        });
        ui.add_space(14.0);
        if remake {
            self.list = catalog.artist_list(&self.filter, self.sort);
            self.made_for = Some(shell.generation);
        }

        let row_height = 48.0;
        let visible = ((ui.available_height() / row_height) as usize).max(1);
        let moved = widgets::move_selection(&ctx, &mut self.selected, self.list.len(), 1, visible);
        if moved.moved {
            self.scroll_to = Some(self.selected);
        }
        if let Some(&i) = self.list.get(self.selected) {
            let id = catalog.artists[i as usize].id;
            if moved.star {
                shell.act(Action::ToggleStar(Kind::Artist, id));
            }
            if moved.rate_by != 0 {
                shell.act(Action::RateBy(Kind::Artist, id, moved.rate_by));
            }
            if moved.add {
                let tracks = catalog.artist_tracks(id).iter().map(|&t| catalog.tracks[t as usize].id).collect();
                shell.act(Action::AddToPlaylist(tracks));
            }
        }
        if moved.enter {
            if let Some(&i) = self.list.get(self.selected) {
                shell.act(Action::Go(Page::Artist(catalog.artists[i as usize].id)));
            }
        }
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("artists");
        if let Some(row) = self.scroll_to.take() {
            let top = row as f32 * row_height;
            let height = ui.available_height();
            if top < self.offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + row_height > self.offset + height {
                scroll = scroll.vertical_scroll_offset(top + row_height - height);
            }
        }
        let output = scroll.show_rows(ui, row_height, self.list.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for n in range {
                let artist = &catalog.artists[self.list[n] as usize];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::click());
                let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.1);
                let rect = rect.shrink2(Vec2::new(0.0, 2.0));
                let fill = if n == self.selected { P.surface_hi } else { widgets::lerp(P.bg, P.surface, hover) };
                ui.painter().rect_filled(rect, 8, fill);
                // A round badge with the initial, colored like the artist's placeholder.
                let badge = rect.left_center() + Vec2::new(24.0, 0.0);
                let hue = artist.id.as_bytes()[15] as f32 / 255.0;
                ui.painter().circle_filled(badge, 15.0, egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.35, 0.3, 1.0)));
                let initial = artist.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                ui.painter().text(badge, Align2::CENTER_CENTER, initial, font(13.0, medium()), P.text);

                let left = rect.left() + 52.0;
                let width = rect.width() - 52.0 - 200.0;
                let aliases = if artist.aliases.is_empty() { None } else { Some(format!("also {}", artist.aliases.join(", "))) };
                let name = if artist.disambiguation.is_empty() { artist.name.clone() } else { format!("{} ({})", artist.name, artist.disambiguation) };
                let y = if aliases.is_some() { -8.0 } else { 0.0 };
                widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y + y - 9.0), Vec2::new(width, 18.0)), &name, font(14.0, medium()), P.text);
                if let Some(aliases) = aliases {
                    widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y + 1.0), Vec2::new(width, 16.0)), &aliases, font(12.0, medium()), P.faint);
                }
                let counts = format!(
                    "{} track{} · {} album{}",
                    thousands(artist.track_count as usize),
                    if artist.track_count == 1 { "" } else { "s" },
                    thousands(artist.album_count as usize),
                    if artist.album_count == 1 { "" } else { "s" }
                );
                let counts_rect = ui.painter().text(rect.right_center() - Vec2::new(18.0, 0.0), Align2::RIGHT_CENTER, counts, font(12.5, medium()), P.muted);
                if shell.mark(artist.id).starred {
                    widgets::draw_star(ui.painter(), Pos2::new(counts_rect.left() - 14.0, rect.center().y), 6.0, P.accent);
                }
                if response.clicked() {
                    self.selected = n;
                    shell.act(Action::Go(Page::Artist(artist.id)));
                }
            }
        });
        self.offset = output.state.offset.y;
        if self.list.is_empty() {
            ui.label(RichText::new("No artists").color(P.muted));
        }
    }
}
