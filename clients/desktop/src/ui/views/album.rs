//! An album's page: its cover and facts, a Play button, and its tracks. Also the track
//! list used here and on artist pages. Keys: arrows or jk move, Enter plays from there.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, widgets, Action, Shell};

#[derive(Default)]
pub struct AlbumPage {
    list: TrackList,
    /// The details panel is open.
    details: bool,
    /// This album and another side by side, to merge (edit mode), in place of the list.
    merge: Option<super::album_merge::AlbumMergeView>,
    /// "Merge into another album…" asked the palette for the other one.
    picking_merge: bool,
    /// Undoing merges into this album (in the details).
    undo: super::album_merge::UndoPanel,
}

impl AlbumPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        // Ctrl+M: merge into another album (again: close it). In a text field too.
        if !shell.palette_open && ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if !shell.edit_mode {
                shell.playlists.notice = Some(("Merging is in edit mode: e, then Ctrl+M.".into(), std::time::Instant::now()));
            } else if self.merge.is_some() {
                self.merge = None;
            } else {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
        }
        let Some(album) = catalog.album(id) else {
            ui.label(RichText::new("This album isn't in the library (anymore).").color(P.muted));
            return;
        };
        let tracks = catalog.album_tracks(id);

        // Header: a big cover, and what the album is.
        let header = ui.allocate_exact_size(Vec2::new(ui.available_width(), 232.0), Sense::hover()).0;
        let cover_rect = Rect::from_min_size(header.min, Vec2::splat(220.0));
        shell.cover(ui, album.cover(), album.id, cover_rect, 12, 0.6);
        let left = cover_rect.right() + 28.0;
        let painter = ui.painter();
        painter.text(Pos2::new(left, cover_rect.top() + 70.0), Align2::LEFT_BOTTOM, album.kind.to_uppercase(), font(11.0, semibold()), P.accent);
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
        if widgets::chip(&mut marks_ui, "Details", self.details) {
            self.details = !self.details;
        }
        if shell.edit_mode && self.merge.is_none() {
            marks_ui.add_space(8.0);
            let tip = "The same album twice: choose the other one, then see both before merging";
            if marks_ui.button("Merge into another album…  (Ctrl+M)").on_hover_text(tip).clicked() {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
        }
        ui.add_space(22.0);

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
        if self.details {
            details_panel(ui, "album-details", |ui| {
                let cached = shell.album_details.get(id);
                let Some(d) = cached.value.clone() else {
                    ui.label(RichText::new(if cached.refreshing { "Loading…" } else { "Couldn't load the details" }).color(P.muted));
                    return;
                };
                if !d.credits.is_empty() {
                    ui.label(RichText::new("Credits").font(font(14.0, semibold())));
                    for c in &d.credits {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&c.role).color(P.faint));
                            let name = c.alias.clone().unwrap_or_else(|| c.artist_name.clone());
                            if widgets::link(ui, &name, font(14.0, medium()), P.text) {
                                shell.act(Action::Go(crate::ui::Page::Artist(c.artist_id)));
                            }
                        });
                    }
                    ui.add_space(8.0);
                }
                if !d.covers.is_empty() {
                    ui.label(RichText::new(format!("Covers ({})", d.covers.len())).font(font(14.0, semibold())));
                    ui.horizontal_wrapped(|ui| {
                        for c in &d.covers {
                            let (rect, response) = ui.allocate_exact_size(Vec2::splat(110.0), Sense::hover());
                            shell.cover(ui, c.cover(), c.id, rect, 6, 0.1);
                            let size = match (c.width, c.height) {
                                (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                                _ => c.source.clone(),
                            };
                            response.on_hover_text(size);
                        }
                    });
                    ui.add_space(8.0);
                }
                if !d.sources.is_empty() {
                    ui.label(RichText::new(format!("Sources ({})", d.sources.len())).font(font(14.0, semibold())));
                    super::track::sources(ui, &d.sources);
                }
                if !d.merges.is_empty() {
                    ui.label(RichText::new("Merged into this album").font(font(14.0, semibold())));
                    let can = shell.account.can("edit_metadata");
                    self.undo.show(ui, shell, id, &d.merges, can);
                }
            });
            ui.add_space(12.0);
        }

        // Edit mode: the album's own data (title, kind, date, artists).
        if shell.edit_mode {
            edit_panel(ui, shell, catalog, id);
            ui.add_space(12.0);
        }

        if tracks.is_empty() {
            ui.label(RichText::new("No tracks you can see").color(P.muted));
            return;
        }
        self.list.show(ui, shell, catalog, tracks, player, Rows::Album { id, artist: &album.artist_display });
    }
}

/// The album's form (edit mode). Kept in the shell per album, so changes not saved yet
/// are still there after looking elsewhere.
fn edit_panel(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid) {
    use crate::ui::edit::album_form::AlbumForm;
    let cached = shell.album_details.get(id);
    let Some(d) = cached.value.clone() else {
        ui.label(RichText::new(if cached.refreshing { "Loading the album to edit…" } else { "Couldn't load the album to edit" }).color(P.muted));
        return;
    };
    details_panel(ui, "album-edit", |ui| {
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let Some(mut form) = shell.album_forms.remove(&id).or_else(|| AlbumForm::from_detail(&d)) else {
            ui.label(RichText::new("This album has no revision to build on, so it can't be edited here.").color(P.muted));
            return;
        };
        form.show(ui, shell, Some(catalog), &d);
        shell.album_forms.insert(id, form);
    });
}

/// How a track list shows its rows.
#[derive(Clone, Copy)]
pub enum Rows<'a> {
    /// On album `id`: the track number; the artist only where it differs from the album's.
    Album { id: Uuid, artist: &'a str },
    /// Anywhere else: a small cover, the artist and the album.
    Mixed,
}

/// A list of tracks, virtual (only the rows on screen are drawn), with a keyboard
/// selection. Enter or a double click plays the list from that track.
#[derive(Default)]
pub struct TrackList {
    selected: usize,
    scroll_to: Option<usize>,
}

impl TrackList {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, tracks: &[u32], player: &PlayerView, rows: Rows) {
        let ctx = ui.ctx().clone();
        let row_height = 44.0;
        let visible = ((ui.available_height() / row_height) as usize).max(1);
        let moved = widgets::move_selection(&ctx, &mut self.selected, tracks.len(), 1, visible);
        if moved.moved {
            self.scroll_to = Some(self.selected);
        }
        if let Some(&i) = tracks.get(self.selected) {
            let id = catalog.tracks[i as usize].id;
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
                shell.act(Action::Enqueue { tracks: vec![i], next });
            }
            if moved.info {
                shell.act(Action::Go(crate::ui::Page::Track(id)));
            }
        }
        if moved.enter {
            shell.act(Action::Play { tracks: tracks.to_vec(), start: self.selected });
        }
        let playing = playing_track(player);
        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("track-list");
        if let Some(row) = self.scroll_to.take() {
            // Just enough to bring the row into view (egui scrolls to the offset given).
            let top = row as f32 * row_height;
            let offset = ctx.data(|d| d.get_temp::<f32>(egui::Id::new("track-list-offset"))).unwrap_or(0.0);
            let height = ui.available_height();
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + row_height > offset + height {
                scroll = scroll.vertical_scroll_offset(top + row_height - height);
            }
        }
        let output = scroll.show_rows(ui, row_height, tracks.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for n in range {
                let track = &catalog.tracks[tracks[n] as usize];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_height), Sense::click());
                let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.1);
                let fill = if n == self.selected { P.surface_hi } else { widgets::lerp(P.bg, P.surface, hover) };
                let rect = rect.shrink2(Vec2::new(0.0, 1.0));
                ui.painter().rect_filled(rect, 8, fill);
                let is_playing = playing == Some(track.id);
                let lead = rect.left_center() + Vec2::new(22.0, 0.0);
                let mut text_left = rect.left() + 52.0;
                match rows {
                    Rows::Album { id, .. } => {
                        if is_playing {
                            widgets::equalizer(ui.painter(), lead, ui.input(|i| i.time) as f32, !player.paused);
                            if !player.paused {
                                ctx.request_repaint();
                            }
                        } else {
                            let number = track
                                .albums
                                .iter()
                                .find(|m| m.0 == id)
                                .and_then(|m| m.2)
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "·".into());
                            ui.painter().text(lead, Align2::CENTER_CENTER, number, font(13.0, medium()), P.faint);
                        }
                    }
                    Rows::Mixed => {
                        let cover = Rect::from_center_size(rect.left_center() + Vec2::new(24.0, 0.0), Vec2::splat(34.0));
                        shell.cover(ui, track.cover(), track.id, cover, 5, 0.0);
                        if is_playing {
                            ui.painter().rect_filled(cover, 5, egui::Color32::from_black_alpha(150));
                            widgets::equalizer(ui.painter(), cover.center(), ui.input(|i| i.time) as f32, !player.paused);
                            if !player.paused {
                                ctx.request_repaint();
                            }
                        }
                        text_left = rect.left() + 54.0;
                    }
                }
                let title_color = if is_playing { P.accent } else { P.text };
                let second = match rows {
                    Rows::Album { artist, .. } if track.artist_display != artist => Some(track.artist_display.clone()),
                    Rows::Album { .. } => None,
                    Rows::Mixed => Some(match catalog.main_album(track) {
                        Some(album) if album.kind != "single" => format!("{}  ·  {}", track.artist_display, album.title),
                        _ => track.artist_display.clone(),
                    }),
                };
                let text_width = rect.right() - 90.0 - text_left;
                let title_y = if second.is_some() { -8.0 } else { 0.0 };
                widgets::text_in(ui.painter(), Rect::from_center_size(Pos2::new(text_left + text_width / 2.0, rect.center().y + title_y), Vec2::new(text_width, 18.0)), &track.title, font(14.0, medium()), title_color);
                if let Some(second) = second {
                    widgets::text_in(ui.painter(), Rect::from_center_size(Pos2::new(text_left + text_width / 2.0, rect.center().y + 9.0), Vec2::new(text_width, 16.0)), &second, font(12.0, medium()), P.muted);
                }
                if shell.mark(track.id).starred {
                    widgets::draw_star(ui.painter(), rect.right_center() - Vec2::new(76.0, 0.0), 6.0, P.accent);
                }
                ui.painter().text(rect.right_center() - Vec2::new(18.0, 0.0), Align2::RIGHT_CENTER, widgets::length(track.duration_ms), font(13.0, medium()), P.muted);
                if response.clicked() {
                    self.selected = n;
                }
                if response.double_clicked() {
                    shell.act(Action::Play { tracks: tracks.to_vec(), start: n });
                }
            }
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("track-list-offset"), output.state.offset.y));
    }
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

/// A panel for an album's or artist's details: framed, scrolling if long.
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

