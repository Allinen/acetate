//! The queue: what's played (dimmed), what plays (lit) and what's next, in order. Shuffle
//! and loop can be switched here too.
//!
//! Keys: arrows or jk move, Enter plays from there, Delete takes one out, Ctrl+↑/↓ moves
//! it (not the one playing), `i` its page, `s` star.

use eframe::egui::{self, Align2, Key, Modifiers, Pos2, Rect, RichText, Sense, Vec2};

use crate::catalog::marks::Kind;
use crate::playback::PlayerView;
use crate::ui::theme::{font, medium, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

const ROW: f32 = 46.0;

#[derive(Default)]
pub struct QueuePage {
    selected: usize,
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
        let items = player.items.clone();
        let current = player.index;
        if self.fresh {
            self.fresh = false;
            self.selected = current;
            self.scroll_to = Some(current.saturating_sub(2));
        }

        let upcoming = items.len().saturating_sub(current + 1);
        let left_ms: i64 = items.iter().skip(current + 1).filter_map(|i| i.duration_ms).sum();
        super::heading(ui, "Queue", items.len());
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
        });
        ui.add_space(14.0);
        if items.is_empty() {
            ui.label(RichText::new("Nothing queued: q adds the selected track at the end, Shift+Q plays it next").color(P.muted));
            return;
        }

        // Keys.
        let visible = ((ui.available_height() / ROW) as usize).max(1);
        let moved = widgets::move_selection(&ctx, &mut self.selected, items.len(), 1, visible);
        if moved.moved {
            self.scroll_to = Some(self.selected);
        }
        let n = self.selected.min(items.len() - 1);
        let item = &items[n];
        if moved.enter {
            shell.act(Action::PlayQueueAt(n));
        }
        if moved.star {
            shell.act(Action::ToggleStar(Kind::Track, item.track_id));
        }
        if moved.info {
            shell.act(Action::Go(Page::Track(item.track_id)));
        }
        if !ctx.egui_wants_keyboard_input() {
            let (delete, up, down) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Delete),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowUp),
                    i.consume_key(Modifiers::COMMAND, Key::ArrowDown),
                )
            });
            if delete && n != current {
                shell.act(Action::RemoveFromQueue(n));
                if n > current && n + 1 >= items.len() {
                    self.selected = n.saturating_sub(1);
                }
            }
            let to = if up && n > 0 { Some(n - 1) } else if down && n + 1 < items.len() { Some(n + 1) } else { None };
            if let Some(to) = to.filter(|&to| n != current && to != current) {
                shell.act(Action::MoveInQueue(n, to));
                self.selected = to;
                self.scroll_to = Some(to);
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
        let output = scroll.show_rows(ui, ROW, items.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let item = &items[i];
                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
                let rect = rect.shrink2(Vec2::new(0.0, 1.0));
                let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.1);
                let fill = if i == self.selected { P.surface_hi } else if i == current { P.accent_soft } else { widgets::lerp(P.bg, P.surface, hover) };
                ui.painter().rect_filled(rect, 8, fill);
                let cover = Rect::from_center_size(rect.left_center() + Vec2::new(26.0, 0.0), Vec2::splat(34.0));
                shell.cover(ui, item.cover, item.track_id, cover, 5, 0.0);
                if i == current && player.playing {
                    ui.painter().rect_filled(cover, 5, egui::Color32::from_black_alpha(150));
                    widgets::equalizer(ui.painter(), cover.center(), ui.input(|input| input.time) as f32, !player.paused);
                    if !player.paused {
                        ctx.request_repaint();
                    }
                }
                let played = i < current;
                let (title_color, sub_color) = if played { (P.faint, P.faint) } else if i == current { (P.accent, P.muted) } else { (P.text, P.muted) };
                let left = rect.left() + 56.0;
                let width = rect.width() - 56.0 - 80.0;
                widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y - 17.0), Vec2::new(width, 18.0)), &item.title, font(14.0, medium()), title_color);
                let sub = match &item.album {
                    Some(album) => format!("{}  ·  {album}", item.artist),
                    None => item.artist.clone(),
                };
                widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, rect.center().y + 1.0), Vec2::new(width, 16.0)), &sub, font(12.0, medium()), sub_color);
                ui.painter().text(rect.right_center() - Vec2::new(16.0, 0.0), Align2::RIGHT_CENTER, widgets::length(item.duration_ms), font(13.0, medium()), P.faint);
                if response.clicked() {
                    self.selected = i;
                }
                if response.double_clicked() {
                    shell.act(Action::PlayQueueAt(i));
                }
            }
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("queue-offset"), output.state.offset.y));
    }
}
