//! Watching a video (a music video: an extra's video file), over the whole window: the
//! video as big as fits, and under it play/pause, where it is (click or drag to seek), the
//! volume, fullscreen. Keys: Space pause, ←/→ 5 s, ↑/↓ volume, F fullscreen, M mute, Esc
//! out of fullscreen, then closes. A click on the video pauses, a double click is
//! fullscreen. The music pauses when it opens. Played by mpv (see `crate::video`).

use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, Rect, Sense, Vec2};
use uuid::Uuid;

use crate::ui::theme::{font, medium, semibold, PALETTE as P};
use crate::ui::widgets;
use crate::video::Player;

/// The controls' height, under the video.
const BAR: f32 = 64.0;

pub struct Watch {
    title: String,
    player: Result<Player, String>,
    texture: Option<egui::TextureHandle>,
    /// The size the last frame was rendered at (pixels): another means render again.
    rendered: [usize; 2],
    volume: f64,
    muted: bool,
    fullscreen: bool,
    /// Where a drag on the seek bar is (a fraction), until it's let go.
    dragging: Option<f32>,
}

impl Watch {
    /// Starts watching an extra's file (`file`): `title` above it.
    pub fn open(ctx: &egui::Context, api: &crate::api::Api, file: Uuid, title: &str, volume: f64) -> Watch {
        let player = match api.address_and_auth(&format!("/api/extra-files/{file}/stream")) {
            Some((url, header)) => Player::open(ctx, &url, &header, volume),
            None => Err("You're not logged in.".into()),
        };
        Watch { title: title.to_string(), player, texture: None, rendered: [0, 0], volume, muted: false, fullscreen: false, dragging: None }
    }

    /// Draws it; true when it's closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new(egui::Id::new("watch")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0, Color32::BLACK);
            let player = match &mut self.player {
                Ok(player) => player,
                Err(err) => {
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, err.as_str(), font(16.0, medium()), P.muted);
                    let x = Rect::from_center_size(rect.right_top() + Vec2::new(-28.0, 28.0), Vec2::splat(32.0));
                    close = ui.put(x, egui::Button::new("×").frame(false)).clicked() || ui.input(|i| i.key_pressed(Key::Escape));
                    return;
                }
            };
            player.drain_events();
            let video = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - BAR));
            // The frame, rendered at the size it's shown (in pixels).
            let ppp = ctx.pixels_per_point();
            let size = [(video.width() * ppp) as usize, (video.height() * ppp) as usize];
            let again = size != self.rendered || self.texture.is_none();
            if let Some(image) = player.frame(size[0], size[1], again) {
                self.rendered = size;
                match &mut self.texture {
                    Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                    None => self.texture = Some(ctx.load_texture("watch-frame", image, egui::TextureOptions::LINEAR)),
                }
            }
            match &self.texture {
                Some(texture) => {
                    ui.painter().image(texture.id(), video, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                None => {
                    ui.painter().text(video.center(), Align2::CENTER_CENTER, "Loading…", font(15.0, medium()), P.muted);
                }
            }
            // A click on the video pauses; a double click is fullscreen.
            let on_video = ui.interact(video, egui::Id::new("watch-video"), Sense::click());
            if on_video.double_clicked() {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            } else if on_video.clicked() {
                let _ = player.command(&["cycle", "pause"]);
            }

            // ---- The bar ----
            let bar = Rect::from_min_max(Pos2::new(rect.min.x, video.max.y), rect.max);
            ui.painter().rect_filled(bar, 0, P.surface);
            let paused = player.paused();
            let mid = bar.center().y;
            if widgets::icon_button(ui, "watch-play", Pos2::new(bar.min.x + 34.0, mid), 18.0, if paused { widgets::Icon::Play } else { widgets::Icon::Pause }, true) {
                let _ = player.command(&["cycle", "pause"]);
            }
            let position = player.position().unwrap_or(0.0);
            let duration = player.duration().unwrap_or(0.0);
            let track = Rect::from_min_max(Pos2::new(bar.min.x + 150.0, mid - 3.0), Pos2::new(bar.max.x - 330.0, mid + 3.0));
            let seek = ui.interact(track.expand2(Vec2::new(0.0, 10.0)), egui::Id::new("watch-seek"), Sense::click_and_drag());
            let pointer = seek.interact_pointer_pos().map(|p| ((p.x - track.min.x) / track.width()).clamp(0.0, 1.0));
            if seek.dragged() {
                self.dragging = pointer;
            }
            if (seek.clicked() || seek.drag_stopped())
                && let Some(at) = pointer.or(self.dragging)
                && duration > 0.0
            {
                let _ = player.command(&["seek", &format!("{:.2}", at as f64 * duration), "absolute"]);
                self.dragging = None;
            }
            let fraction = self.dragging.unwrap_or(if duration > 0.0 { (position / duration) as f32 } else { 0.0 });
            ui.painter().rect_filled(track, 3, P.surface_hi);
            ui.painter().rect_filled(Rect::from_min_size(track.min, Vec2::new(track.width() * fraction, track.height())), 3, P.accent);
            let shown_at = self.dragging.map(|f| f as f64 * duration).unwrap_or(position);
            ui.painter().text(Pos2::new(track.min.x - 10.0, mid), Align2::RIGHT_CENTER, clock(shown_at), font(12.0, medium()), P.muted);
            ui.painter().text(Pos2::new(track.max.x + 10.0, mid), Align2::LEFT_CENTER, clock(duration), font(12.0, medium()), P.muted);
            // Volume, fullscreen, close.
            let volume_rect = Rect::from_min_size(Pos2::new(bar.max.x - 270.0, mid - 10.0), Vec2::new(140.0, 20.0));
            let mut volume = self.volume;
            let slider = ui.put(volume_rect, egui::Slider::new(&mut volume, 0.0..=100.0).show_value(false));
            if slider.changed() {
                self.volume = volume;
                self.muted = false;
                player.set("volume", &format!("{volume:.0}"));
                player.set("mute", "no");
            }
            let full = Rect::from_center_size(Pos2::new(bar.max.x - 96.0, mid), Vec2::new(64.0, 28.0));
            if ui.put(full, egui::Button::new(if self.fullscreen { "Window" } else { "Full" })).on_hover_text("Fullscreen (F)").clicked() {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            let x = Rect::from_center_size(Pos2::new(bar.max.x - 32.0, mid), Vec2::splat(32.0));
            if ui.put(x, egui::Button::new("×")).on_hover_text("Close (Esc)").clicked() {
                close = true;
            }

            // ---- Keys ----
            let (space, left, right, up, down, f, m, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Space),
                    i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                    i.consume_key(Modifiers::NONE, Key::ArrowRight),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::F),
                    i.consume_key(Modifiers::NONE, Key::M),
                    i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if space {
                let _ = player.command(&["cycle", "pause"]);
            }
            if left || right {
                let _ = player.command(&["seek", if left { "-5" } else { "5" }, "relative"]);
            }
            if up || down {
                self.volume = (self.volume + if up { 5.0 } else { -5.0 }).clamp(0.0, 100.0);
                player.set("volume", &format!("{:.0}", self.volume));
            }
            if m {
                self.muted = !self.muted;
                player.set("mute", if self.muted { "yes" } else { "no" });
            }
            if f {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            if esc {
                if self.fullscreen {
                    self.fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                } else {
                    close = true;
                }
            }
            // Its name, over the video's top left, while paused.
            if paused {
                ui.painter().text(video.min + Vec2::new(20.0, 20.0), Align2::LEFT_TOP, &self.title, font(18.0, semibold()), Color32::WHITE);
            }
            if !paused {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        });
        if close && self.fullscreen {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        close
    }
}

/// Seconds as "3:07" (or "1:02:03").
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}
