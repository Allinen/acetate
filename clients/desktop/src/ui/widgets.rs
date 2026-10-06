//! Small drawing pieces used all over: icons drawn as shapes, the logo, placeholders
//! for covers still loading, the "playing" equalizer, one-line text that fits a box,
//! and keyboard movement through lists and grids.

use std::time::Duration;

use eframe::egui::{self, Color32, Key, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use uuid::Uuid;

use super::theme::PALETTE as P;

#[derive(Clone, Copy)]
pub enum Icon {
    Play,
    Pause,
    Previous,
    Next,
    Shuffle,
    /// Repeat one: a loop with a small "1".
    Repeat,
}

/// A small toggle button at `center`: the icon lit (accent, with a dot under it) when on.
pub fn toggle_button(ui: &mut egui::Ui, id: &str, center: Pos2, radius: f32, icon: Icon, on: bool, tip: &str) -> bool {
    let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let lit = ui.ctx().animate_bool_with_time(response.id.with("on"), on, 0.15);
    let painter = ui.painter();
    painter.circle_filled(center, radius, P.surface_hi.gamma_multiply(hover));
    let color = lerp(lerp(P.faint, P.text, hover), P.accent, lit);
    draw_icon(painter, icon, center, radius * 0.5, color);
    if lit > 0.0 {
        painter.circle_filled(center + Vec2::new(0.0, radius * 0.95), 1.8, P.accent.gamma_multiply(lit));
    }
    response.on_hover_text(tip).clicked()
}

/// A round icon button at `center`. `primary`: filled with the accent.
pub fn icon_button(ui: &mut egui::Ui, id: &str, center: Pos2, radius: f32, icon: Icon, primary: bool) -> bool {
    let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let press = ui.ctx().animate_bool_with_time(response.id.with("press"), response.is_pointer_button_down_on(), 0.06);
    let scale = 1.0 + hover * 0.08 - press * 0.08;
    let painter = ui.painter();
    if primary {
        painter.circle_filled(center, radius * scale, lerp(P.text, P.accent, hover));
        draw_icon(painter, icon, center, radius * 0.42 * scale, P.bg);
    } else {
        painter.circle_filled(center, radius * scale, P.surface_hi.gamma_multiply(hover));
        draw_icon(painter, icon, center, radius * 0.5 * scale, lerp(P.muted, P.text, hover));
    }
    response.clicked()
}

/// Icons as shapes (crisp at any size, no icon font).
pub fn draw_icon(painter: &egui::Painter, icon: Icon, c: Pos2, s: f32, color: Color32) {
    let triangle = |left: f32, right: f32| {
        Shape::convex_polygon(vec![Pos2::new(c.x + left, c.y - s), Pos2::new(c.x + right, c.y), Pos2::new(c.x + left, c.y + s)], color, Stroke::NONE)
    };
    match icon {
        Icon::Play => {
            painter.add(triangle(-s * 0.7, s * 1.1));
        }
        Icon::Pause => {
            for x in [-s * 0.55, s * 0.55] {
                painter.rect_filled(Rect::from_center_size(Pos2::new(c.x + x, c.y), Vec2::new(s * 0.55, s * 1.9)), 1, color);
            }
        }
        Icon::Next => {
            painter.add(triangle(-s * 0.9, s * 0.5));
            painter.rect_filled(Rect::from_center_size(Pos2::new(c.x + s * 0.75, c.y), Vec2::new(s * 0.3, s * 1.9)), 1, color);
        }
        Icon::Shuffle => {
            // Two crossing lines, each ending in an arrowhead on the right.
            let stroke = Stroke::new(s * 0.2, color);
            for (from, to) in [(-0.6, 0.6), (0.6, -0.6)] {
                let (a, b) = (Pos2::new(c.x - s, c.y + from * s), Pos2::new(c.x + s * 0.7, c.y + to * s));
                painter.line_segment([a, b], stroke);
                painter.add(Shape::convex_polygon(
                    vec![Pos2::new(b.x + s * 0.45, b.y), Pos2::new(b.x - s * 0.05, b.y - s * 0.35), Pos2::new(b.x - s * 0.05, b.y + s * 0.35)],
                    color,
                    Stroke::NONE,
                ));
            }
        }
        Icon::Repeat => {
            let stroke = Stroke::new(s * 0.2, color);
            let rect = Rect::from_center_size(c, Vec2::new(s * 2.0, s * 1.3));
            painter.rect_stroke(rect, s * 0.4, stroke, egui::StrokeKind::Middle);
            // An arrowhead on the top edge, pointing right.
            let tip = Pos2::new(c.x + s * 0.35, rect.top());
            painter.add(Shape::convex_polygon(
                vec![Pos2::new(tip.x + s * 0.4, tip.y), Pos2::new(tip.x - s * 0.1, tip.y - s * 0.35), Pos2::new(tip.x - s * 0.1, tip.y + s * 0.35)],
                color,
                Stroke::NONE,
            ));
            painter.text(c, egui::Align2::CENTER_CENTER, "1", egui::FontId::proportional(s * 0.9), color);
        }
        Icon::Previous => {
            painter.add(Shape::convex_polygon(vec![Pos2::new(c.x + s * 0.9, c.y - s), Pos2::new(c.x + s * 0.9, c.y + s), Pos2::new(c.x - s * 0.5, c.y)], color, Stroke::NONE));
            painter.rect_filled(Rect::from_center_size(Pos2::new(c.x - s * 0.75, c.y), Vec2::new(s * 0.3, s * 1.9)), 1, color);
        }
    }
}

/// The logo (the simple one: a note on amber), rounded like an app icon.
pub fn logo(ui: &mut egui::Ui, logo: &egui::TextureHandle, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    egui::Image::from_texture(logo).corner_radius((size * 0.22) as u8).paint_at(ui, rect);
}

/// The logo as a texture, made once at start: the brand file, scaled down (it's drawn
/// small; a big texture would only blur).
pub fn logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let bytes = include_bytes!("../../../../assets/brand/acetate-simple.jpg");
    let image = image::load_from_memory(bytes).expect("the logo").thumbnail(128, 128).to_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw());
    ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
}

/// While a cover loads: a disc on a color made from the album's id (so each looks
/// different, and the same every time).
pub fn placeholder(painter: &egui::Painter, rect: Rect, radius: u8, seed: Uuid) {
    let bytes = seed.as_bytes();
    let hue = bytes[15] as f32 / 255.0;
    let base = egui::ecolor::Hsva::new(hue, 0.35, 0.22, 1.0);
    painter.rect_filled(rect, radius, Color32::from(base));
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.28;
    painter.circle_filled(c, r, Color32::from_black_alpha(90));
    painter.circle_stroke(c, r * 0.7, Stroke::new(1.0, Color32::from_white_alpha(12)));
    painter.circle_filled(c, r * 0.3, Color32::from(egui::ecolor::Hsva::new(hue, 0.5, 0.55, 1.0)));
}

/// Three bouncing bars: what plays.
pub fn equalizer(painter: &egui::Painter, center: Pos2, time: f32, moving: bool) {
    for (i, phase) in [0.0f32, 1.7, 3.1].into_iter().enumerate() {
        let height = if moving { 4.0 + 8.0 * (0.5 + 0.5 * (time * 7.0 + phase).sin()) } else { 4.0 };
        let x = center.x - 6.0 + i as f32 * 5.0;
        painter.rect_filled(Rect::from_min_max(Pos2::new(x, center.y + 6.0 - height), Pos2::new(x + 3.0, center.y + 6.0)), 1, P.accent);
    }
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}

pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// One line of text painted inside `rect` (cut with "…" to fit), vertically centered.
/// Painted, not laid out: it takes no space and doesn't move what comes next (unlike
/// `ui.put`, which moves the layout cursor).
pub fn text_in(painter: &egui::Painter, rect: Rect, text: &str, font: egui::FontId, color: Color32) {
    use egui::text::{LayoutJob, TextFormat, TextWrapping};
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping { max_width: rect.width(), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let top = rect.center().y - galley.size().y / 2.0;
    painter.galley(Pos2::new(rect.left(), top), galley, color);
}

/// The "OG" badge (an original file): the letters in amber inside a rounded amber outline,
/// its left edge's middle at `at`, `height` tall. Returns its width.
pub fn og_badge(painter: &egui::Painter, at: Pos2, height: f32) -> f32 {
    let galley = painter.layout_no_wrap("OG".into(), og_badge_font(height), P.accent);
    let width = galley.size().x + height * 0.6;
    let rect = Rect::from_min_size(Pos2::new(at.x, at.y - height / 2.0), Vec2::new(width, height));
    painter.rect_stroke(rect, (height * 0.3) as u8, Stroke::new(1.0, P.accent.gamma_multiply(0.85)), egui::StrokeKind::Inside);
    painter.galley(rect.center() - galley.size() / 2.0, galley, P.accent);
    width
}

/// How wide the badge is at `height` (to leave room for it).
pub fn og_badge_width(painter: &egui::Painter, height: f32) -> f32 {
    painter.layout_no_wrap("OG".into(), og_badge_font(height), P.accent).size().x + height * 0.6
}

fn og_badge_font(height: f32) -> egui::FontId {
    super::theme::font(height * 0.62, super::theme::bold())
}

/// `text_in`, and (`og`) the OG badge right after the text: the text shortened to leave it
/// room.
pub fn text_in_og(painter: &egui::Painter, rect: Rect, text: &str, font: egui::FontId, color: Color32, og: bool) {
    if !og {
        return text_in(painter, rect, text, font, color);
    }
    use egui::text::{LayoutJob, TextFormat, TextWrapping};
    let height = (font.size * 1.05).round();
    let room = og_badge_width(painter, height) + 6.0;
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping { max_width: (rect.width() - room).max(0.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let top = rect.center().y - galley.size().y / 2.0;
    let right = rect.left() + galley.size().x;
    painter.galley(Pos2::new(rect.left(), top), galley, color);
    og_badge(painter, Pos2::new(right + 6.0, rect.center().y), height);
}

/// A length in milliseconds as a clock ("3:07"), or nothing.
pub fn length(ms: Option<i64>) -> String {
    ms.map(|ms| clock(Duration::from_millis(ms.max(0) as u64))).unwrap_or_default()
}

/// What a movement key did to a list or grid selection.
#[derive(Default)]
pub struct Moved {
    /// The selection changed (bring it into view).
    pub moved: bool,
    /// Enter was pressed (Shift held: `shift`).
    pub enter: bool,
    pub shift: bool,
    /// `s`: star (or unstar) the selected one.
    pub star: bool,
    /// `+` / `-`: rate it half a star up or down.
    pub rate_by: i16,
    /// `p`: add it to a playlist.
    pub add: bool,
    /// `i`: open its page (a track's: everything about it).
    pub info: bool,
    /// `q`: add it to the end of the queue (false); Shift+Q: play it next (true).
    pub queue: Option<bool>,
}

/// Moves `selected` through `len` items laid out in `columns` (1 for a list) with the
/// arrow keys or hjkl, Page Up/Down and Home/End; also reads the keys for the selected
/// item (Enter, `s` star, `+`/`-` rate, `p` add to a playlist, `q`/Shift+Q queue). Does nothing while a text field has
/// the keyboard, or with Ctrl/Alt held (those keys belong to playback and navigation).
pub fn move_selection(ctx: &egui::Context, selected: &mut usize, len: usize, columns: usize, page: usize) -> Moved {
    let mut out = Moved::default();
    if len == 0 || ctx.egui_wants_keyboard_input() {
        return out;
    }
    let modifiers = ctx.input(|i| i.modifiers);
    if modifiers.ctrl || modifiers.alt || modifiers.command {
        return out;
    }
    let key = |k: Key| ctx.input(|i| i.key_pressed(k));
    let last = len - 1;
    let mut at = (*selected).min(last) as isize;
    let columns = columns.max(1) as isize;
    let page = (page.max(1) as isize) * columns;
    if columns > 1 && !modifiers.shift && (key(Key::ArrowRight) || key(Key::L)) {
        at += 1;
    } else if columns > 1 && !modifiers.shift && (key(Key::ArrowLeft) || key(Key::H)) {
        at -= 1;
    } else if key(Key::ArrowDown) || key(Key::J) {
        at += columns;
    } else if key(Key::ArrowUp) || key(Key::K) {
        at -= columns;
    } else if key(Key::PageDown) {
        at += page;
    } else if key(Key::PageUp) {
        at -= page;
    } else if key(Key::Home) {
        at = 0;
    } else if key(Key::End) {
        at = last as isize;
    }
    let at = at.clamp(0, last as isize) as usize;
    if at != *selected {
        *selected = at;
        out.moved = true;
    }
    if key(Key::Enter) {
        out.enter = true;
        out.shift = modifiers.shift;
    }
    if !modifiers.shift && key(Key::S) {
        out.star = true;
    }
    if !modifiers.shift && key(Key::P) {
        out.add = true;
    }
    if !modifiers.shift && key(Key::I) {
        out.info = true;
    }
    if key(Key::Q) {
        out.queue = Some(modifiers.shift);
    }
    if key(Key::Plus) || key(Key::Equals) {
        out.rate_by = 1;
    } else if key(Key::Minus) {
        out.rate_by = -1;
    }
    out
}

/// Text that's a link: underlined on hover, a pointing hand. Returns true when clicked.
pub fn link(ui: &mut egui::Ui, text: &str, font: egui::FontId, color: Color32) -> bool {
    let label = egui::Label::new(egui::RichText::new(text).font(font).color(color)).sense(Sense::click()).truncate();
    let response = ui.add(label).on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().hline(response.rect.x_range(), response.rect.bottom(), Stroke::new(1.0, color.gamma_multiply(0.7)));
    }
    response.clicked()
}

/// A link for an original file's track (an "og file"): its title in the accent colour,
/// and while hovered, lighter and deeper tints of it rolling across the letters like a
/// wave; the file's own name shows on hover. Returns true when clicked.
pub fn og_link(ui: &mut egui::Ui, text: &str, font: egui::FontId, original_filename: &str) -> bool {
    // Hovered last frame (the text is laid out before its response is known).
    let key = ui.id().with(("og-hovered", text));
    let hovered = ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    let time = ui.input(|i| i.time);
    let job = og_text(text, font, hovered.then_some(time));
    let label = egui::Label::new(job).sense(Sense::click()).truncate();
    let response = ui.add(label).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(format!("Original file · {original_filename}"));
    if response.hovered() {
        ui.painter().hline(response.rect.x_range(), response.rect.bottom(), Stroke::new(1.0, P.accent.gamma_multiply(0.7)));
        ui.ctx().request_repaint();
    }
    if response.hovered() != hovered {
        ui.ctx().data_mut(|d| d.insert_temp(key, response.hovered()));
        ui.ctx().request_repaint();
    }
    response.clicked()
}

/// The amber edge on an og file's cover: inside the cover (so nothing clips it), `width`
/// thick, following its corners.
pub fn og_edge(painter: &egui::Painter, rect: Rect, radius: u8, width: f32) {
    painter.rect_stroke(rect, radius, Stroke::new(width, P.accent), egui::StrokeKind::Inside);
}

/// An og file's title: all accent, or (`wave`: the time) a letter at a time, its colour
/// shifting between a lighter and a deeper tint as a wave passes.
pub(crate) fn og_text(text: &str, font: egui::FontId, wave: Option<f64>) -> egui::text::LayoutJob {
    let format = |color| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
    let Some(time) = wave else {
        return egui::text::LayoutJob::single_section(text.to_string(), format(P.accent));
    };
    let (light, deep) = (Color32::from_rgb(0xff, 0xe0, 0xb0), Color32::from_rgb(0xe0, 0x78, 0x3c));
    let mut job = egui::text::LayoutJob::default();
    for (n, c) in text.chars().enumerate() {
        // A wave about 12 letters long, crossing 6 letters a second.
        let phase = (n as f64 / 12.0 - time * 0.5) * std::f64::consts::TAU;
        let t = phase.sin() as f32;
        let color = if t >= 0.0 { lerp_color(P.accent, light, t) } else { lerp_color(P.accent, deep, -t) };
        let mut buf = [0u8; 4];
        job.append(c.encode_utf8(&mut buf), 0.0, format(color));
    }
    job
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// The "Share" chip on a track's, album's or artist's page: a click copies the link (for
/// someone with an account), Shift+click opens the share link popup (for anyone).
pub fn share_chip(ui: &mut egui::Ui, shell: &mut crate::ui::Shell, target: crate::links::Target) {
    let clicked = chip(ui, "Share", false);
    if clicked {
        let shift = ui.input(|i| i.modifiers.shift);
        shell.act(if shift { crate::ui::Action::CopyShareLink(target) } else { crate::ui::Action::CopyLink(target) });
    }
}

/// A right-click menu on a track row or an album tile with "Share" in it: a click copies
/// the link, Shift+click makes a share link (as the pages' "Share").
pub fn share_menu(response: &egui::Response, shell: &mut crate::ui::Shell, target: crate::links::Target) {
    response.context_menu(|ui| {
        let item = ui.button("Share").on_hover_text("Copies a link, for someone with an account (Ctrl+L). Shift+click: the popup for a share link anyone can open (Ctrl+Shift+L).");
        if item.clicked() {
            let shift = ui.input(|i| i.modifiers.shift);
            shell.act(if shift { crate::ui::Action::CopyShareLink(target) } else { crate::ui::Action::CopyLink(target) });
            ui.close();
        }
    });
}

/// A small pill button with text; `active` fills it with the accent's soft color.
pub fn chip(ui: &mut egui::Ui, text: &str, active: bool) -> bool {
    let font = super::theme::font(12.5, super::theme::medium());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, P.text);
    let size = Vec2::new(galley.size().x + 22.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.1);
    let fill = if active { P.accent_soft } else { lerp(P.bg, P.surface, hover) };
    ui.painter().rect_filled(rect, 13, fill);
    if !active {
        ui.painter().rect_stroke(rect, 13, Stroke::new(1.0, P.line), egui::StrokeKind::Inside);
    }
    let color = if active { P.accent } else { lerp(P.muted, P.text, hover) };
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response.clicked()
}

/// A five-pointed star at `center`, `r` to its points, drawn as triangles around its
/// center (a star isn't convex, so it can't be one polygon).
pub fn draw_star(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let point = |i: usize| {
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        let radius = if i.is_multiple_of(2) { r } else { r * 0.45 };
        center + Vec2::angled(angle) * radius
    };
    for i in 0..10 {
        painter.add(Shape::convex_polygon(vec![center, point(i), point(i + 1)], color, Stroke::NONE));
    }
}

/// A star button: filled with the accent when `starred`. Returns true when clicked.
pub fn star_button(ui: &mut egui::Ui, starred: bool, size: f32) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.1);
    let on = ui.ctx().animate_bool_with_time(response.id.with("on"), starred, 0.18);
    let color = lerp(lerp(P.faint, P.text, hover), P.accent, on);
    // A little pop when it's starred.
    let pop = 1.0 + (on * std::f32::consts::PI).sin() * 0.25;
    draw_star(ui.painter(), rect.center(), size * 0.42 * pop, color);
    response.on_hover_text(if starred { "Unstar (s)" } else { "Star (s)" }).clicked()
}

/// Five stars for a 0–10 rating (half stars). Clicking the left or right half of a star
/// sets that; the far left edge of the first star, clicking the rating it already has,
/// or a right click clears it. Returns the new rating if changed.
pub fn rating(ui: &mut egui::Ui, value: i16, size: f32) -> Option<i16> {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(size * 5.0, size), Sense::click());
    // After a click, the stars show the rating itself (not the one under the pointer)
    // until the pointer leaves: otherwise clearing it would look like nothing happened.
    let clicked_id = response.id.with("clicked");
    let just_clicked = ui.ctx().data(|d| d.get_temp::<bool>(clicked_id)).unwrap_or(false);
    if !response.hovered() && just_clicked {
        ui.ctx().data_mut(|d| d.remove::<bool>(clicked_id));
    }
    // What a click here would set: half stars, and 0 on the first star's left edge.
    let at_pointer = response.hover_pos().map(|p| {
        let x = p.x - rect.left();
        if x < size * 0.2 { 0 } else { ((x / (size / 2.0)).ceil() as i16).clamp(1, 10) }
    });
    let preview = if just_clicked { None } else { at_pointer };
    let shown = preview.unwrap_or(value);
    let painter = ui.painter();
    for star in 0..5 {
        let center = Pos2::new(rect.left() + size * (star as f32 + 0.5), rect.center().y);
        let r = size * 0.4;
        draw_star(painter, center, r, P.surface_hi);
        let filled = (shown - star * 2).clamp(0, 2);
        if filled > 0 {
            let color = if preview.is_some() { P.accent.gamma_multiply(0.75) } else { P.accent };
            let width = if filled == 2 { size } else { size / 2.0 };
            let clip = Rect::from_min_size(Pos2::new(center.x - size / 2.0, rect.top()), Vec2::new(width, size));
            draw_star(&painter.with_clip_rect(clip), center, r, color);
        }
    }
    let response = response.on_hover_text("Rate (+ / −) · left edge or right click: clear");
    let new = if response.secondary_clicked() {
        Some(0)
    } else if response.clicked() {
        at_pointer.map(|new| if new == value { 0 } else { new })
    } else {
        None
    };
    if new.is_some() {
        ui.ctx().data_mut(|d| d.insert_temp(clicked_id, true));
    }
    new.filter(|&new| new != value)
}

#[cfg(test)]
mod og_tests {
    use super::*;

    #[test]
    fn an_og_title_waves_only_while_hovered() {
        let font = egui::FontId::proportional(13.0);
        let still = og_text("Glass Harbor", font.clone(), None);
        assert_eq!(still.sections.len(), 1);
        assert_eq!(still.sections[0].format.color, P.accent);
        let wave = og_text("Glass Harbor", font, Some(0.3));
        assert_eq!(wave.sections.len(), "Glass Harbor".chars().count(), "a colour per letter");
        let colors: std::collections::HashSet<Color32> = wave.sections.iter().map(|s| s.format.color).collect();
        assert!(colors.len() > 4, "the colours shift along the title");
    }
}
