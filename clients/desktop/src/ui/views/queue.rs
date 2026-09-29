//! The Queue page: what plays, then the tracks queued by hand (they play first), then
//! the rest of the list played from, then (dimmed) the list's tracks already played.
//! Shuffle and loop can be switched here too.
//!
//! Keys: arrows or jk move, Enter plays it now, Delete takes it out, Ctrl+↑/↓ moves a
//! queued track within the queue, `i` its page, `s` star.

use eframe::egui::{self, Align2, Key, Modifiers, Pos2, Rect, RichText, Sense, Vec2};

use crate::catalog::marks::Kind;
use crate::playback::queue::Place;
use crate::playback::{PlayerView, QueueItem};
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

const ROW: f32 = 46.0;

/// A row of the page.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Heading(&'static str),
    Current,
    Queued(usize),
    List(usize),
}

#[derive(Default)]
pub struct QueuePage {
    /// The selected row (a place, so it stays on the same track as rows come and go).
    selected: Option<Row>,
    scroll_to: Option<usize>,
    /// The page was just opened: select and show what plays.
    fresh: bool,
}

impl QueuePage {
    pub fn opened(&mut self) {
        self.fresh = true;
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
        let ctx = ui.ctx().clone();
        let rows = rows(player);
        let item = |row: Row| -> Option<&QueueItem> {
            match row {
                Row::Current => player.current.as_ref(),
                Row::Queued(i) => player.queued.get(i),
                Row::List(i) => player.list.get(i),
                Row::Heading(_) => None,
            }
        };
        let current_row = rows.iter().position(|r| *r == Row::Current);
        if self.fresh {
            self.fresh = false;
            self.selected = Some(Row::Current);
            self.scroll_to = current_row;
        }

        let upcoming = player.queued.len() + player.list.len().saturating_sub(player.list_index + 1);
        let left_ms: i64 = player.queued.iter().chain(player.list.iter().skip(player.list_index + 1)).filter_map(|i| i.duration_ms).sum();
        super::heading(ui, "Queue", upcoming);
        ui.horizontal(|ui| {
            let next = if upcoming == 1 { "1 track next".to_string() } else { format!("{} tracks next", thousands(upcoming)) };
            ui.label(RichText::new(format!("{next} · {} to go", widgets::length(Some(left_ms)))).color(P.muted));
            ui.add_space(12.0);
            if widgets::chip(ui, if player.shuffle { "Shuffle: on  z" } else { "Shuffle  z" }, player.shuffle) {
                shell.act(Action::ToggleShuffle);
            }
            if widgets::chip(ui, if player.repeat { "Looping this track  r" } else { "Loop this track  r" }, player.repeat) {
                shell.act(Action::ToggleRepeat);
            }
            if !player.queued.is_empty() && widgets::chip(ui, "Clear the queue", false) {
                shell.act(Action::ClearQueued);
            }
        });
        ui.add_space(14.0);
        if player.current.is_none() && player.queued.is_empty() {
            ui.label(RichText::new("Nothing playing. q queues the selected track at the end of the queue, Shift+Q at its start").color(P.muted));
            return;
        }

        // Keys: the selection moves over tracks, not headings.
        let tracks: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| !matches!(r, Row::Heading(_))).map(|(i, _)| i).collect();
        let mut at = self.selected.and_then(|s| tracks.iter().position(|&i| rows[i] == s)).unwrap_or(0);
        let visible = ((ui.available_height() / ROW) as usize).max(1);
        let moved = widgets::move_selection(&ctx, &mut at, tracks.len(), 1, visible);
        let selected_row = tracks.get(at).map(|&i| rows[i]);
        self.selected = selected_row;
        if moved.moved {
            self.scroll_to = tracks.get(at).copied();
        }
        let place = |row: Row| match row {
            Row::Queued(i) => Some(Place::Queued(i)),
            Row::List(i) => Some(Place::List(i)),
            _ => None,
        };
        if let Some(row) = selected_row {
            if let (true, Some(place)) = (moved.enter, place(row)) {
                shell.act(Action::PlayPlace(place));
                self.selected = Some(Row::Current);
            }
            if let Some(item) = item(row) {
                if moved.star {
                    shell.act(Action::ToggleStar(Kind::Track, item.track_id));
                }
                if moved.info {
                    shell.act(Action::Go(Page::Track(item.track_id)));
                }
            }
            if !ctx.egui_wants_keyboard_input() {
                let (delete, up, down) = ui.input_mut(|i| {
                    (
                        i.consume_key(Modifiers::NONE, Key::Delete),
                        i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
                        i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                    )
                });
                if let (true, Some(place)) = (delete, place(row)) {
                    shell.act(Action::RemovePlace(place));
                }
                if let Row::Queued(i) = row {
                    let to = if up && i > 0 { Some(i - 1) } else if down && i + 1 < player.queued.len() { Some(i + 1) } else { None };
                    if let Some(to) = to {
                        shell.act(Action::MoveQueued(i, to));
                        self.selected = Some(Row::Queued(to));
                    }
                }
            }
        }

        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("queue");
        if let Some(row) = self.scroll_to.take() {
            let offset = ctx.data(|d| d.get_temp::<f32>(egui::Id::new("queue-offset"))).unwrap_or(0.0);
            let (top, height) = (row as f32 * ROW, ui.available_height());
            if top < offset || top + ROW > offset + height {
                scroll = scroll.vertical_scroll_offset((top - height / 3.0).max(0.0));
            }
        }
        let output = scroll.show_rows(ui, ROW, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for n in range {
                let row = rows[n];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                if let Row::Heading(title) = row {
                    ui.painter().text(rect.left_bottom() + Vec2::new(4.0, -10.0), Align2::LEFT_BOTTOM, title, font(15.0, semibold()), P.muted);
                    continue;
                }
                let Some(item) = item(row) else { continue };
                let rect = rect.shrink2(Vec2::new(0.0, 1.0));
                let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.1);
                let fill = if Some(row) == self.selected {
                    P.surface_hi
                } else if row == Row::Current {
                    P.accent_soft
                } else {
                    widgets::lerp(P.bg, P.surface, hover)
                };
                ui.painter().rect_filled(rect, 8, fill);
                let cover = Rect::from_center_size(rect.left_center() + Vec2::new(26.0, 0.0), Vec2::splat(34.0));
                shell.cover(ui, item.cover, item.track_id, cover, 5, 0.0);
                if row == Row::Current && player.playing {
                    ui.painter().rect_filled(cover, 5, egui::Color32::from_black_alpha(150));
                    widgets::equalizer(ui.painter(), cover.center(), ui.input(|input| input.time) as f32, !player.paused);
                    if !player.paused {
                        ctx.request_repaint();
                    }
                }
                let played = matches!(row, Row::List(i) if i <= player.list_index);
                let (title_color, sub_color) = match row {
                    _ if played => (P.faint, P.faint),
                    Row::Current => (P.accent, P.muted),
                    _ => (P.text, P.muted),
                };
                let left = rect.left() + 56.0;
                let width = rect.width() - 56.0 - 80.0;
                let og = super::original_filename(shell, item.track_id).is_some();
                widgets::text_in_og(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y - 17.0), Vec2::new(width, 18.0)), &item.title, font(14.0, medium()), title_color, og);
                let sub = match &item.album {
                    Some(album) => format!("{}  ·  {album}", item.artist),
                    None => item.artist.clone(),
                };
                widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y + 1.0), Vec2::new(width, 16.0)), &sub, font(12.0, medium()), sub_color);
                ui.painter().text(rect.right_center() - Vec2::new(16.0, 0.0), Align2::RIGHT_CENTER, widgets::length(item.duration_ms), font(13.0, medium()), P.faint);
                if response.clicked() {
                    self.selected = Some(row);
                }
                if response.double_clicked()
                    && let Some(place) = place(row)
                {
                    shell.act(Action::PlayPlace(place));
                }
            }
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("queue-offset"), output.state.offset.y));
    }
}

/// The page's rows: what plays, the queue, the rest of the list, the list's played part.
fn rows(player: &PlayerView) -> Vec<Row> {
    let mut rows = Vec::new();
    if player.current.is_some() {
        rows.push(Row::Heading("Now playing"));
        rows.push(Row::Current);
    }
    if !player.queued.is_empty() {
        rows.push(Row::Heading("Next in the queue"));
        rows.extend((0..player.queued.len()).map(Row::Queued));
    }
    // The list's current track is shown as "now playing" (when it plays from the list).
    let from = player.list_index + 1;
    if from < player.list.len() {
        rows.push(Row::Heading("Next from the list"));
        rows.extend((from..player.list.len()).map(Row::List));
    }
    let played_until = if player.current_queued { player.list_index + 1 } else { player.list_index };
    if played_until > 0 {
        rows.push(Row::Heading("Earlier in the list"));
        rows.extend((0..played_until.min(player.list.len())).map(Row::List));
    }
    rows
}
