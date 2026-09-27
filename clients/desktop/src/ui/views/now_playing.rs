//! Now playing: a big cover with a glow that breathes with the bass, the title, what's
//! next, and the spectrum visualizer.

use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Shadow, Vec2};

use crate::playback::spectrum::Spectrum;
use crate::catalog::marks::Kind;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, PALETTE as P};
use crate::ui::{widgets, Shell};

pub fn show(ui: &mut egui::Ui, shell: &mut Shell, player: &PlayerView, spectrum: &mut Spectrum, dt: f32) {
    let ctx = ui.ctx().clone();
    let playing = player.playing && !player.paused;
    {
        let scope = shell.controller.scope.lock().unwrap();
        spectrum.update(&scope, playing, dt);
    }
    let moving = spectrum.bars.iter().any(|b| *b > 0.001);
    if playing || moving {
        ctx.request_repaint();
    }
    let area = ui.max_rect();
    let Some(item) = player.current().cloned() else {
        ui.painter().text(area.center(), Align2::CENTER_CENTER, "Nothing playing", font(18.0, medium()), P.faint);
        return;
    };

    let spectrum_height = (area.height() * 0.28).clamp(90.0, 220.0);
    let size = (area.height() - spectrum_height - 60.0).min(area.width() * 0.42).clamp(160.0, 520.0);
    let cover_rect = Rect::from_min_size(area.min + Vec2::new(0.0, 10.0), Vec2::splat(size));
    // A glow behind the cover that breathes with the bass.
    let bass = spectrum.bars[..6].iter().sum::<f32>() / 6.0;
    ui.painter().add(
        Shadow { offset: [0, 10], blur: (70.0 + bass * 50.0) as u8, spread: (6.0 + bass * 14.0) as u8, color: P.accent.gamma_multiply(0.10 + bass * 0.22) }
            .as_shape(cover_rect, 18),
    );
    shell.cover(ui, item.cover_id, item.track_id, cover_rect, 16, 1.0);

    let left = cover_rect.right() + 40.0;
    let width = area.right() - left;
    let middle = cover_rect.center().y;
    widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, middle - 52.0), Vec2::new(width, 50.0)), &item.title, font(40.0, bold()), P.text);
    widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(left, middle + 4.0), Vec2::new(width, 26.0)), &item.artist, font(19.0, medium()), P.muted);
    // Star, rating and plays of what plays.
    let marks_rect = Rect::from_min_size(Pos2::new(left, middle + 44.0), Vec2::new(width, 30.0));
    let mut marks_ui = ui.new_child(egui::UiBuilder::new().max_rect(marks_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    super::album::marks(&mut marks_ui, shell, Kind::Track, item.track_id);
    if let Some(next) = player.items.get(player.index + 1) {
        ui.painter().text(Pos2::new(left, middle + 92.0), Align2::LEFT_TOP, format!("Next  ·  {} — {}", next.title, next.artist), font(13.0, medium()), P.faint);
    }

    // The spectrum, with a faint reflection.
    let floor = area.bottom() - spectrum_height * 0.3;
    let bars = spectrum.bars.len();
    let gap = 4.0;
    let bar_width = (area.width() - gap * (bars - 1) as f32) / bars as f32;
    let painter = ui.painter();
    for (i, value) in spectrum.bars.iter().enumerate() {
        let height = 3.0 + value * spectrum_height * 0.7;
        let x = area.left() + i as f32 * (bar_width + gap);
        let color = widgets::lerp(P.accent, Color32::from_rgb(255, 214, 170), value * 0.8);
        painter.rect_filled(Rect::from_min_max(Pos2::new(x, floor - height), Pos2::new(x + bar_width, floor)), CornerRadius::same(3), color);
        let reflection = height * 0.35;
        painter.rect_filled(Rect::from_min_max(Pos2::new(x, floor + 3.0), Pos2::new(x + bar_width, floor + 3.0 + reflection)), CornerRadius::same(3), color.gamma_multiply(0.12));
    }
}
