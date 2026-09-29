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
    // Layers: the veil over the page (Middle), the panel above it (Foreground). Not higher:
    // a dropdown in the panel opens on the Foreground layer too (egui's own popups), and
    // one that just appeared goes on top of its layer; with the panel on the Tooltip layer
    // it opened *behind* it, out of sight.
    egui::Area::new(egui::Id::new((id, "veil"))).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(rect, 0, Color32::from_black_alpha(140));
    });
    let size = Vec2::new(size.x.min(screen.width() - 48.0), size.y.min(screen.height() - 48.0));
    let top_left = Pos2::new(screen.center().x - size.x / 2.0, screen.center().y - size.y / 2.0);
    egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(top_left).show(ctx, |ui| {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropdown in a popup opens in front of it (it used to open behind it, out of sight).
    #[test]
    fn a_dropdown_in_a_popup_opens_in_front_of_it() {
        let ctx = egui::Context::default();
        crate::ui::theme::apply(&ctx);
        let screen = egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0));
        // The button's place, and the layer the dropdown's list is drawn on (once open).
        let mut button = egui::Rect::NOTHING;
        let mut list: Option<egui::LayerId> = None;
        let frame = |events: Vec<egui::Event>, button: &mut egui::Rect, list: &mut Option<egui::LayerId>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                show(ui.ctx(), "test-popup", "Test", Vec2::new(600.0, 400.0), |ui| {
                    let response = egui::ComboBox::from_id_salt("test-combo").selected_text("choose").show_ui(ui, |ui| {
                        *list = Some(ui.layer_id());
                        for n in 0..5 {
                            let _ = ui.selectable_label(false, format!("option {n}"));
                        }
                    });
                    *button = response.response.rect;
                });
            });
            // No renderer here: the textures it would upload are let go.
            output.textures_delta.clear();
        };
        frame(Vec::new(), &mut button, &mut list);
        let at = button.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(vec![egui::Event::PointerMoved(at)], &mut button, &mut list);
        frame(vec![press(true)], &mut button, &mut list);
        frame(vec![press(false)], &mut button, &mut list);
        frame(Vec::new(), &mut button, &mut list);
        let list = list.expect("the click opened the dropdown");
        // Just below the button: the dropdown's list, on top of the popup.
        let below = Pos2::new(button.center().x, button.bottom() + 20.0);
        assert_eq!(ctx.layer_id_at(below), Some(list), "the dropdown is in front, not behind the popup");
    }
}
