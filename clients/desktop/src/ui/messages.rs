//! The messages shown as toasts, kept so one that went by too fast can be read again
//! (Alt+M, or "Earlier messages" in the palette).

use std::time::Instant;

use eframe::egui::{self, Align2, Color32, Key, Modifiers, Pos2, RichText, Sense, Shadow};

use super::theme::{bold, font, medium, PALETTE as P};

/// How many are kept.
const KEPT: usize = 100;

pub struct Message {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Default)]
pub struct Messages {
    /// Oldest first.
    pub list: Vec<Message>,
    pub open: bool,
}

impl Messages {
    /// Keeps a message the toast is showing, once (by when it was made).
    pub fn record(&mut self, text: &str, at: Instant, error: bool) {
        if self.list.iter().rev().take(8).any(|m| m.at == at) {
            return;
        }
        self.list.push(Message { text: text.to_string(), at, error });
        if self.list.len() > KEPT {
            self.list.remove(0);
        }
    }

    /// The list, newest first, while open; Esc, Alt+M or a click outside closes it.
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let close = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::ALT, Key::M));
        let screen = ctx.content_rect();
        let veil = egui::Area::new(egui::Id::new("messages-veil")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(rect, 0, Color32::from_black_alpha(140));
            response.clicked()
        });
        if close || veil.inner {
            self.open = false;
            return;
        }
        let width = (screen.width() - 80.0).clamp(300.0, 640.0);
        egui::Area::new(egui::Id::new("messages"))
            .order(egui::Order::Tooltip)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(Pos2::new(screen.center().x, screen.center().y))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(P.surface)
                    .corner_radius(14)
                    .stroke(egui::Stroke::new(1.0, P.line))
                    .shadow(Shadow { offset: [0, 16], blur: 48, spread: 0, color: Color32::from_black_alpha(170) })
                    .inner_margin(egui::Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.label(RichText::new("Earlier messages").font(font(20.0, bold())));
                        ui.add_space(10.0);
                        if self.list.is_empty() {
                            ui.label(RichText::new("None yet.").font(font(13.0, medium())).color(P.muted));
                        }
                        egui::ScrollArea::vertical().max_height((screen.height() - 160.0).max(120.0)).show(ui, |ui| {
                            for m in self.list.iter().rev() {
                                ui.horizontal_top(|ui| {
                                    ui.add_sized([64.0, 16.0], egui::Label::new(RichText::new(ago(m.at)).font(font(11.5, medium())).color(P.faint)));
                                    let color = if m.error { P.danger } else { P.text };
                                    ui.add(egui::Label::new(RichText::new(&m.text).font(font(13.0, medium())).color(color)).wrap());
                                });
                                ui.add_space(6.0);
                            }
                        });
                    });
            });
    }
}

fn ago(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..=4 => "now".into(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}
