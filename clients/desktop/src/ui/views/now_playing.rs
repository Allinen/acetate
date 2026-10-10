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
    shell.cover_crossfade(ui, "now-playing", item.cover, item.track_id, cover_rect, 16, 1.0);
    let original = super::original_filename(shell, item.track_id);
    if original.is_some() {
        widgets::og_edge(ui.painter(), cover_rect, 16, 2.0);
    }
    shell.original_on_click(ui, item.cover, cover_rect);

    let left = cover_rect.right() + 40.0;
    let width = area.right() - left;
    // With lyrics, the title goes to the top and the lyrics fill the column below it;
    // without, the title sits in the middle, beside the cover.
    // (A file that didn't import isn't a track: no details, marks or lyrics.)
    let is_track = item.source == crate::playback::AudioSource::File;
    // (From the catalog, offline too, unless the server just sent fresher ones.)
    let lyrics = if is_track {
        let all = crate::details::lyrics_to_show(shell.catalog.value.as_deref(), shell.track_details.get(item.track_id), item.track_id);
        all.iter().find(|l| l.is_synced).or(all.first()).cloned()
    } else {
        None
    };
    let middle = if lyrics.is_some() { cover_rect.top() + 52.0 } else { cover_rect.center().y };
    super::playing_links(
        ui,
        shell,
        item.track_id,
        &item.title,
        &item.artist,
        Rect::from_min_size(Pos2::new(left, middle - 52.0), Vec2::new(width, 50.0)),
        Rect::from_min_size(Pos2::new(left, middle + 4.0), Vec2::new(width, 26.0)),
        font(40.0, bold()),
        font(19.0, medium()),
    );
    // Star, rating and plays of what plays.
    let marks_rect = Rect::from_min_size(Pos2::new(left, middle + 44.0), Vec2::new(width, 30.0));
    let mut marks_ui = ui.new_child(egui::UiBuilder::new().max_rect(marks_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if is_track {
        super::album::marks(&mut marks_ui, shell, Kind::Track, item.track_id);
    }
    // An original file: said quietly, next to the marks, with its name.
    if let Some(name) = &original {
        marks_ui.add_space(12.0);
        marks_ui.add(egui::Label::new(egui::RichText::new(format!("Original file · {name}")).font(font(12.5, medium())).color(P.accent.gamma_multiply(0.8))).truncate());
    }
    let next = player.next();
    if let Some(next) = next {
        let text = format!("Next  ·  {} — {}", next.title, next.artist);
        let og = super::original_filename(shell, next.track_id).is_some();
        widgets::text_in_og(ui.painter(), Rect::from_min_size(Pos2::new(left, middle + 88.0), Vec2::new(width, 18.0)), &text, font(13.0, medium()), P.faint, og);
    }

    // The lyrics, down to the visualizer.
    if let Some(lyrics) = lyrics {
        let bottom = area.bottom() - spectrum_height - 8.0;
        let rect = Rect::from_min_max(Pos2::new(left, middle + 124.0), Pos2::new(area.right(), bottom));
        if rect.height() > 60.0 {
            let mut lyrics_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
            let followed_id = egui::Id::new(("now-playing-lyrics", item.track_id));
            let mut followed = ctx.data(|d| d.get_temp::<Option<usize>>(followed_id)).flatten();
            egui::ScrollArea::vertical()
                .id_salt(followed_id)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, false])
                .show(&mut lyrics_ui, |ui| {
                    // Synced: room above and below, so the first and last lines can be in
                    // the middle.
                    let room = if lyrics.is_synced { rect.height() / 2.0 - 20.0 } else { 0.0 };
                    ui.add_space(room);
                    super::track::lyrics_lines(ui, shell, &lyrics, true, player, &mut followed, 17.0);
                    ui.add_space(room);
                });
            ctx.data_mut(|d| d.insert_temp(followed_id, followed));
        }
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
