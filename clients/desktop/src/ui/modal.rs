//! A popup over the page: a dimmed page behind, a titled panel in the middle, closed with
//! Esc (in a text field: the first Esc leaves the field) or its × (a click outside does
//! nothing: what's in it isn't lost by a stray click).
//! While one is open the page's and the app's keys wait (whoever shows it sets
//! `Shell::modal_open`), so typing in it doesn't reach the page.
//!
//! What's shown in it is kept by whoever opens it: closing only hides it.

use eframe::egui::{self, Color32, Key, Modifiers, Pos2, RichText, Sense, Shadow, Vec2};

use crate::ui::theme::{bold, font, PALETTE as P};

/// Shows the popup `id` with `title`, `add` drawing what's in it (given all the room the
/// panel has: up to `size`, less on a small window). Returns true when it asks to be
/// closed.
pub fn show(ctx: &egui::Context, id: &str, title: &str, size: Vec2, add: impl FnOnce(&mut egui::Ui)) -> bool {
    // Esc in a text field leaves the field first (the field takes it); then it closes.
    let mut close = !ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let screen = ctx.content_rect();
    // The dimmed page (it takes the clicks, so the page underneath doesn't get them).
    egui::Area::new(egui::Id::new((id, "veil"))).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(rect, 0, Color32::from_black_alpha(140));
    });
    let size = Vec2::new(size.x.min(screen.width() - 48.0), size.y.min(screen.height() - 48.0));
    let top_left = Pos2::new(screen.center().x - size.x / 2.0, screen.center().y - size.y / 2.0);
    egui::Area::new(egui::Id::new(id)).order(egui::Order::Tooltip).fixed_pos(top_left).show(ctx, |ui| {
        let frame = egui::Frame::NONE
            .fill(P.surface)
            .corner_radius(14)
            .stroke(egui::Stroke::new(1.0, P.line))
            .shadow(Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(170) })
            .inner_margin(egui::Margin::same(16));
        frame.show(ui, |ui| {
            let inner = size - Vec2::splat(32.0);
            ui.set_width(inner.x);
            ui.set_height(inner.y);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).font(font(18.0, bold())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let x = ui.add(egui::Button::new(RichText::new("×").font(font(15.0, bold())).color(P.muted)).frame(false));
                    if x.on_hover_text("Close (Esc)").clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            add(ui);
        });
    });
    close
}
