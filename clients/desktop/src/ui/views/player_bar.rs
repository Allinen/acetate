//! The player bar: what plays (click: now playing), the buttons, the progress bar
//! (click or drag to seek; the seek happens on release) and errors from playback.

use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, Sense, Vec2};

use crate::catalog::marks::Kind;
use crate::playback::PlayerView;
use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

pub fn show(ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView) {
    let ctx = ui.ctx().clone();
    let full = ui.max_rect();
    let current = player.current().cloned();

    // Left: cover and what plays (click: now playing).
    let left = Rect::from_min_size(full.min, Vec2::new(full.width() * 0.3, full.height()));
    let cover_rect = Rect::from_min_size(left.min + Vec2::new(0.0, 4.0), Vec2::splat(56.0));
    let response = ui.interact(left, egui::Id::new("now-playing-link"), Sense::click());
    if response.clicked() {
        shell.act(Action::Go(Page::NowPlaying));
    }
    shell.cover_crossfade(ui, "player-bar", current.as_ref().and_then(|c| c.cover), current.as_ref().map(|c| c.track_id).unwrap_or_default(), cover_rect, 8, 0.0);
    if current.as_ref().is_some_and(|c| super::original_filename(shell, c.track_id).is_some()) {
        widgets::og_edge(ui.painter(), cover_rect, 8, 1.5);
    }
    if let Some(item) = &current {
        let text_left = cover_rect.right() + 14.0;
        // A star and a rating for what plays (Shift+S, + / − anywhere); a file that didn't
        // import isn't a track.
        let is_track = item.source == crate::playback::AudioSource::File;
        const RATING_SIZE: f32 = 14.0;
        let width = left.right() - text_left - 44.0 - if is_track { RATING_SIZE * 5.0 + 8.0 } else { 0.0 };
        if is_track {
            let mark = shell.mark(item.track_id);
            let star_rect = Rect::from_center_size(Pos2::new(left.right() - 22.0, cover_rect.center().y), Vec2::splat(28.0));
            let mut star_ui = ui.new_child(egui::UiBuilder::new().max_rect(star_rect));
            if widgets::star_button(&mut star_ui, mark.starred, 24.0) {
                shell.act(Action::ToggleStar(Kind::Track, item.track_id));
            }
            let rating_rect = Rect::from_center_size(
                Pos2::new(star_rect.left() - 4.0 - RATING_SIZE * 2.5, cover_rect.center().y),
                Vec2::new(RATING_SIZE * 5.0, RATING_SIZE),
            );
            let mut rating_ui = ui.new_child(egui::UiBuilder::new().max_rect(rating_rect));
            if let Some(rating) = widgets::rating(&mut rating_ui, mark.rating, RATING_SIZE) {
                shell.act(Action::Rate(Kind::Track, item.track_id, rating));
            }
        }
        // The title (to its album) and artists (to theirs) are links; the rest of this
        // area goes to Now playing.
        super::playing_links(
            ui,
            shell,
            item.track_id,
            &item.title,
            &item.artist,
            Rect::from_min_size(Pos2::new(text_left, cover_rect.top() + 8.0), Vec2::new(width, 20.0)),
            Rect::from_min_size(Pos2::new(text_left, cover_rect.top() + 30.0), Vec2::new(width, 18.0)),
            font(14.0, semibold()),
            font(13.5, medium()),
        );
    } else {
        ui.painter().text(cover_rect.right_center() + Vec2::new(14.0, 0.0), Align2::LEFT_CENTER, "Nothing playing", font(13.0, medium()), P.faint);
    }

    // Center: buttons, and the progress bar under them.
    let center_x = full.center().x;
    let buttons_y = full.top() + 22.0;
    if widgets::icon_button(ui, "prev", Pos2::new(center_x - 52.0, buttons_y), 16.0, widgets::Icon::Previous, false) {
        shell.act(Action::Previous);
    }
    let playing = player.playing && !player.paused;
    if widgets::icon_button(ui, "play", Pos2::new(center_x, buttons_y), 20.0, if playing { widgets::Icon::Pause } else { widgets::Icon::Play }, true) {
        shell.act(Action::TogglePause);
    }
    if widgets::icon_button(ui, "next", Pos2::new(center_x + 52.0, buttons_y), 16.0, widgets::Icon::Next, false) {
        shell.act(Action::Next);
    }
    let shuffle_tip = if player.shuffle { "Shuffle is on (z)" } else { "Shuffle (z)" };
    if widgets::toggle_button(ui, "shuffle", Pos2::new(center_x - 100.0, buttons_y), 14.0, widgets::Icon::Shuffle, player.shuffle, shuffle_tip) {
        shell.act(Action::ToggleShuffle);
    }
    let repeat_tip = if player.repeat { "Looping this track (r)" } else { "Loop this track (r)" };
    if widgets::toggle_button(ui, "repeat", Pos2::new(center_x + 100.0, buttons_y), 14.0, widgets::Icon::Repeat, player.repeat, repeat_tip) {
        shell.act(Action::ToggleRepeat);
    }

    let bar = Rect::from_center_size(Pos2::new(center_x, full.bottom() - 12.0), Vec2::new((full.width() * 0.42).min(620.0), 16.0));
    let total = current.as_ref().and_then(|c| c.duration_ms).map(|ms| Duration::from_millis(ms as u64));
    let position = player.position_now();
    let response = ui.interact(bar, egui::Id::new("progress"), Sense::click_and_drag());
    let hover = ctx.animate_bool_with_time(response.id, response.hovered() || response.dragged(), 0.12);
    let track = Rect::from_center_size(bar.center(), Vec2::new(bar.width(), 4.0 + hover * 2.0));
    let pointer_fraction = response.interact_pointer_pos().map(|p| ((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
    // While dragging, the bar follows the pointer; the seek happens on release.
    let fraction = match (response.dragged(), pointer_fraction, total) {
        (true, Some(at), _) => at,
        (_, _, Some(total)) if total > Duration::ZERO => (position.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0),
        _ => 0.0,
    };
    let position = match (response.dragged(), total) {
        (true, Some(total)) => total.mul_f32(fraction),
        _ => position,
    };
    ui.painter().rect_filled(track, 3, P.surface_hi);
    let filled = Rect::from_min_size(track.min, Vec2::new(track.width() * fraction, track.height()));
    ui.painter().rect_filled(filled, 3, widgets::lerp(P.text, P.accent, hover));
    if hover > 0.0 {
        ui.painter().circle_filled(Pos2::new(filled.right(), track.center().y), 6.0 * hover, P.text);
    }
    ui.painter().text(bar.left_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, widgets::clock(position), font(11.0, medium()), P.muted);
    // The right-hand time: the length, or (click) what's left, with a "-" prefix.
    if let Some(total) = total {
        let remaining = shell.settings.show_remaining;
        let text = if remaining {
            if total.saturating_sub(position) >= Duration::from_secs(1){
                format!("-{}", widgets::clock(total.saturating_sub(position)))
            } else {
                widgets::clock(total.saturating_sub(position))
            }
        } else { widgets::clock(total) };
        let size = ui.painter().layout_no_wrap(text.clone(), font(11.0, medium()), P.muted).size();
        let rect = Rect::from_min_size(Pos2::new(bar.right() + 10.0, bar.center().y - 10.0), Vec2::new(size.x.max(34.0), 20.0));
        let response = ui.interact(rect, egui::Id::new("total-time"), Sense::click());
        let color = if response.hovered() { P.text } else { P.muted };
        ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, text, font(11.0, medium()), color);
        if response.on_hover_text(if remaining { "Show the length" } else { "Show the time left" }).clicked() {
            shell.settings.show_remaining = !remaining;
            shell.settings.save(&shell.data_dir);
        }
    }
    if let (Some(total), Some(at)) = (total, pointer_fraction)
        && (response.clicked() || response.drag_stopped())
    {
        shell.controller.seek(total.mul_f32(at));
    }
    if playing {
        // TODO consider if it makes sense to lower this value to make it smoother or decrease only when focused etc. For it to be worth it, we need a re-design of the bar fill to support anti-aliasing / otherwise be smoother
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    // Right: the newest message for a few seconds (errors a little longer), else the
    // queue position.
    let error = shell.controller.error.lock().unwrap().clone().filter(|(_, at)| at.elapsed() < Duration::from_secs(8));
    let notice = shell.playlists.notice.clone().filter(|(_, at)| at.elapsed() < Duration::from_secs(4));
    let message = match (error, notice) {
        (Some(e), Some(n)) if n.1 > e.1 => Some((n.0, P.accent)),
        (Some(e), _) => Some((e.0, P.danger)),
        (None, Some(n)) => Some((n.0, P.accent)),
        (None, None) => None,
    };
    // The volume, at the far right; messages to its left.
    let volume_rect = Rect::from_min_max(Pos2::new(full.right() - 150.0, full.top()), full.right_bottom());
    volume(ui, shell, player, volume_rect);
    let messages_right = volume_rect.left() - 16.0;
    let width = (messages_right - bar.right() - 70.0).max(80.0);
    let right_rect = Rect::from_min_max(Pos2::new(messages_right - width, full.top()), Pos2::new(messages_right, full.bottom()));
    if let Some((text, color)) = message {
        // Gone after a few seconds: draw again then.
        ctx.request_repaint_after(Duration::from_millis(500));
        let galley_width = ui.painter().layout_no_wrap(text.clone(), font(12.5, medium()), color).size().x.min(width);
        let rect = Rect::from_min_max(Pos2::new(right_rect.right() - galley_width, right_rect.top()), right_rect.max);
        widgets::text_in(ui.painter(), rect, &text, font(12.5, medium()), color);
    } else if current.is_some() {
        // Where playback is: in the hand-made queue, or at a place in the list.
        let place = if player.current_queued { "from the queue".to_string() } else { format!("{} of {}", player.list_index + 1, player.list.len()) };
        let queued = if player.queued.is_empty() { String::new() } else { format!("  ·  {} queued", player.queued.len()) };
        ui.painter().text(right_rect.right_center(), Align2::RIGHT_CENTER, format!("{place}{queued}"), font(12.0, medium()), P.faint);
    }
}

/// The volume: a speaker (click: mute) and a slider (click or drag; the wheel works too).
fn volume(ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView, rect: Rect) {
    let ctx = ui.ctx().clone();
    let level = if player.muted { 0.0 } else { player.volume };
    // The speaker: a box and a cone, and up to two arcs for how loud.
    let speaker = Rect::from_center_size(Pos2::new(rect.left() + 12.0, rect.center().y), Vec2::splat(24.0));
    let response = ui.interact(speaker, egui::Id::new("mute"), Sense::click());
    let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.12);
    let color = widgets::lerp(P.muted, P.text, hover);
    let c = speaker.center();
    let painter = ui.painter();
    painter.rect_filled(Rect::from_center_size(c - Vec2::new(4.0, 0.0), Vec2::new(4.0, 6.0)), 1, color);
    painter.add(egui::Shape::convex_polygon(
        vec![Pos2::new(c.x - 3.0, c.y - 3.0), Pos2::new(c.x + 2.0, c.y - 7.0), Pos2::new(c.x + 2.0, c.y + 7.0), Pos2::new(c.x - 3.0, c.y + 3.0)],
        color,
        egui::Stroke::NONE,
    ));
    if level == 0.0 {
        let stroke = egui::Stroke::new(1.6, color);
        painter.line_segment([Pos2::new(c.x + 5.0, c.y - 3.0), Pos2::new(c.x + 11.0, c.y + 3.0)], stroke);
        painter.line_segment([Pos2::new(c.x + 5.0, c.y + 3.0), Pos2::new(c.x + 11.0, c.y - 3.0)], stroke);
    } else {
        for (n, radius) in [5.0, 9.0].into_iter().enumerate() {
            if level > n as f32 * 0.5 {
                let points: Vec<Pos2> = (0..=8).map(|i| c + Vec2::angled(-0.8 + 1.6 * i as f32 / 8.0) * radius + Vec2::new(2.0, 0.0)).collect();
                painter.add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
            }
        }
    }
    if response.on_hover_text(if player.muted { "Unmute (m)" } else { "Mute (m)" }).clicked() {
        shell.act(Action::ToggleMute);
    }

    // The slider.
    let track = Rect::from_min_max(Pos2::new(speaker.right() + 8.0, rect.center().y - 8.0), Pos2::new(rect.right() - 6.0, rect.center().y + 8.0));
    let response = ui.interact(track, egui::Id::new("volume"), Sense::click_and_drag());
    let hover = ctx.animate_bool_with_time(response.id, response.hovered() || response.dragged(), 0.12);
    if let Some(pointer) = response.interact_pointer_pos()
        && (response.clicked() || response.dragged())
    {
        shell.act(Action::SetVolume(((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0)));
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            shell.act(Action::VolumeBy(scroll / 1000.0));
        }
    }
    let bar = Rect::from_center_size(track.center(), Vec2::new(track.width(), 4.0 + hover * 2.0));
    let painter = ui.painter();
    painter.rect_filled(bar, 3, P.surface_hi);
    let filled = Rect::from_min_size(bar.min, Vec2::new(bar.width() * level, bar.height()));
    painter.rect_filled(filled, 3, widgets::lerp(P.muted, P.accent, hover));
    if hover > 0.0 {
        painter.circle_filled(Pos2::new(filled.right(), bar.center().y), 5.0 * hover, P.text);
    }
    response.on_hover_text(format!("Volume {}%  (9 / 0)", (player.volume * 100.0).round()));
}

