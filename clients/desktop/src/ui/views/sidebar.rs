//! The sidebar: the logo, the sections (a highlight slides to the current one), the
//! playlists (Liked songs first, then yours, then others' public ones) and, at the
//! bottom, who's logged in and the library's state.

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};

use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

pub fn show(ui: &mut egui::Ui, shell: &mut Shell, page: Page, username: &str) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        widgets::logo(ui, &shell.logo, 24.0);
        ui.label(RichText::new("acetate").font(font(20.0, bold())));
    });
    ui.add_space(26.0);

    let items = [
        ("Albums", "1", Page::Albums),
        ("Artists", "2", Page::Artists),
        ("Tracks", "3", Page::Tracks),
        ("Now playing", "4", Page::NowPlaying),
        ("Queue", "5", Page::Queue),
    ];
    // An album's page belongs to Albums, an artist's to Artists.
    let active = match page {
        Page::Albums | Page::Album(_) => Some(0),
        Page::Artists | Page::Artist(_) => Some(1),
        Page::Tracks => Some(2),
        Page::NowPlaying => Some(3),
        Page::Queue => Some(4),
        // Playlists are marked in their own list below.
        Page::Liked | Page::Playlist(_) | Page::Track(_) | Page::Settings => None,
    };
    let height = 36.0;
    let top = ui.cursor().top();
    let width = ui.available_width();
    // The highlight slides to the active item.
    // The highlight slides to the active section; on a playlist's page it fades out
    // where it was.
    let shown = ctx.animate_bool_with_time(egui::Id::new("sidebar-highlight-shown"), active.is_some(), 0.15);
    let last_y = egui::Id::new("sidebar-highlight-y");
    let y = match active {
        Some(active) => ctx.animate_value_with_time(egui::Id::new("sidebar-highlight"), top + active as f32 * (height + 4.0), 0.22),
        None => ctx.data(|d| d.get_temp::<f32>(last_y)).unwrap_or(top),
    };
    ctx.data_mut(|d| d.insert_temp(last_y, y));
    if shown > 0.0 {
        let pill = Rect::from_min_size(Pos2::new(ui.cursor().left(), y), Vec2::new(width, height));
        ui.painter().rect_filled(pill, 8, P.surface_hi.gamma_multiply(shown));
        ui.painter().rect_filled(Rect::from_min_size(pill.min + Vec2::new(0.0, 9.0), Vec2::new(3.0, 18.0)), 2, P.accent.gamma_multiply(shown));
    }

    // Entries exactly `height + 4` apart, as the highlight assumes (egui's own spacing
    // between widgets is left out here).
    ui.spacing_mut().item_spacing.y = 0.0;
    for (n, (label, key, view)) in items.into_iter().enumerate() {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.12);
        let color = if Some(n) == active { P.text } else { widgets::lerp(P.muted, P.text, hover) };
        ui.painter().text(rect.left_center() + Vec2::new(16.0 + hover * 2.0, 0.0), Align2::LEFT_CENTER, label, font(14.0, medium()), color);
        ui.painter().text(rect.right_center() - Vec2::new(10.0, 0.0), Align2::RIGHT_CENTER, key, font(11.0, medium()), P.faint);
        if response.clicked() {
            shell.act(Action::Go(view));
        }
        ui.add_space(4.0);
    }
    ui.spacing_mut().item_spacing.y = 6.0;

    // Playlists, between the sections and the bottom (scrolls when there are many).
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("PLAYLISTS").font(font(11.0, semibold())).color(P.faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let plus = ui.add(egui::Button::new(RichText::new("+").font(font(16.0, medium())).color(P.muted)).frame(false));
            if plus.on_hover_text("New playlist").clicked() {
                shell.act(Action::NewPlaylist);
            }
        });
    });
    ui.add_space(4.0);
    let catalog = shell.catalog.value.clone();
    let playlists: Vec<(uuid::Uuid, String, bool)> =
        shell.playlists.list(catalog.as_deref()).iter().map(|p| (p.id, p.name.clone(), p.mine)).collect();
    let bottom_space = 86.0;
    egui::ScrollArea::vertical()
        .id_salt("sidebar-playlists")
        .max_height((ui.available_height() - bottom_space).max(40.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if playlist_item(ui, "Liked songs", true, page == Page::Liked, true) {
                shell.act(Action::Go(Page::Liked));
            }
            for (id, name, mine) in playlists {
                if playlist_item(ui, &name, mine, page == Page::Playlist(id), false) {
                    shell.act(Action::Go(Page::Playlist(id)));
                }
            }
        });

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.horizontal(|ui| {
            let settings = RichText::new("Settings").color(if page == Page::Settings { P.text } else { P.muted });
            if ui.add(egui::Button::new(settings).frame(false)).on_hover_text("Ctrl+,").clicked() {
                shell.act(Action::Go(Page::Settings));
            }
            if ui.add(egui::Button::new(RichText::new("Keys").color(P.muted)).frame(false)).on_hover_text("?").clicked() {
                shell.act(Action::ShowKeys);
            }
            if ui.add(egui::Button::new(RichText::new("Log out").color(P.muted)).frame(false)).clicked() {
                shell.act(Action::LogOut);
            }
        });
        ui.label(RichText::new(username).color(P.muted).font(font(13.0, medium())));
        let status = ui.label(RichText::new(shell.library_status()).color(P.faint).small());
        if status.on_hover_text("F5: check for changes now").clicked() {
            shell.act(Action::RefreshLibrary);
        }
    });
}

/// One playlist in the sidebar. `mine`: others' are shown dimmer. `liked`: with a star.
fn playlist_item(ui: &mut egui::Ui, name: &str, mine: bool, current: bool, liked: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    if current {
        ui.painter().rect_filled(rect, 7, P.surface_hi);
    }
    let base = if mine { P.muted } else { P.faint };
    let color = if current { P.text } else { widgets::lerp(base, P.text, hover) };
    let mut left = rect.left() + 12.0;
    if liked {
        widgets::draw_star(ui.painter(), Pos2::new(left + 5.0, rect.center().y), 6.0, P.accent);
        left += 18.0;
    }
    widgets::text_in(ui.painter(), Rect::from_min_max(Pos2::new(left, rect.top()), Pos2::new(rect.right() - 6.0, rect.bottom())), name, font(13.5, medium()), color);
    response.clicked()
}
