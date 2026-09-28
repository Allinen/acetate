//! An artist's page: the name and other names they go by, their releases (newest first,
//! a strip of covers), and every track they're credited on (featured and produced ones
//! too). Keys: arrows or jk move through the tracks, Enter plays from there.

use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use super::album::{play_button, Rows, TrackList};
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Default)]
pub struct ArtistPage {
    list: TrackList,
    /// The details panel is open.
    details: bool,
}

impl ArtistPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let ctx = ui.ctx().clone();
        let Some(artist) = catalog.artist(id) else {
            ui.label(RichText::new("This artist isn't in the library (anymore).").color(P.muted));
            return;
        };
        let tracks = catalog.artist_tracks(id);
        let albums = catalog.artist_albums(id);

        ui.label(RichText::new("ARTIST").font(font(11.0, semibold())).color(P.accent));
        let name = if artist.disambiguation.is_empty() { artist.name.clone() } else { format!("{} ({})", artist.name, artist.disambiguation) };
        ui.label(RichText::new(name).font(font(40.0, bold())));
        if !artist.aliases.is_empty() {
            ui.label(RichText::new(format!("Also credited as {}", artist.aliases.join(", "))).font(font(14.0, medium())).color(P.muted));
        }
        ui.label(
            RichText::new(format!("{} tracks · {} releases", thousands(tracks.len()), thousands(albums.len())))
                .font(font(14.0, medium()))
                .color(P.muted),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
            if play_button(ui, "artist-play", play_rect) && !tracks.is_empty() {
                shell.act(Action::Play { tracks: tracks.to_vec(), start: 0 });
            }
            ui.add_space(6.0);
            super::album::marks(ui, shell, Kind::Artist, id);
            ui.add_space(8.0);
            if widgets::chip(ui, "Details", self.details) {
                self.details = !self.details;
            }
        });
        ui.add_space(22.0);
        if self.details {
            super::album::details_panel(ui, "artist-details", |ui| {
                let cached = shell.artist_details.get(id);
                let Some(d) = cached.value.clone() else {
                    ui.label(RichText::new(if cached.refreshing { "Loading…" } else { "Couldn't load the details" }).color(P.muted));
                    return;
                };
                if d.sources.is_empty() {
                    ui.label(RichText::new("No sources").color(P.muted));
                } else {
                    ui.label(RichText::new(format!("Sources ({})", d.sources.len())).font(font(14.0, semibold())));
                    super::track::sources(ui, &d.sources);
                }
            });
            ui.add_space(12.0);
        }

        if !albums.is_empty() {
            ui.label(RichText::new("Releases").font(font(18.0, bold())));
            ui.add_space(8.0);
            let card = 150.0;
            egui::ScrollArea::horizontal().id_salt("artist-releases").auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for &index in albums {
                        let album = &catalog.albums[index as usize];
                        let (rect, response) = ui.allocate_exact_size(Vec2::new(card, card + 48.0), Sense::click());
                        let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.16);
                        let cover = Rect::from_min_size(rect.min + Vec2::new(0.0, 6.0 - hover * 4.0), Vec2::splat(card));
                        shell.cover(ui, album.cover(), album.id, cover, 8, hover);
                        let top = cover.bottom() + 8.0 + hover * 4.0;
                        widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(card, 17.0)), &album.title, font(13.0, semibold()), P.text);
                        let sub = match album.year {
                            Some(year) => format!("{year} · {}", album.kind),
                            None => album.kind.clone(),
                        };
                        widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), top + 18.0), Vec2::new(card, 15.0)), &sub, font(12.0, medium()), P.muted);
                        if response.clicked() {
                            shell.act(Action::Go(Page::Album(album.id)));
                        }
                    }
                });
            });
            ui.add_space(18.0);
        }

        ui.label(RichText::new("Tracks").font(font(18.0, bold())));
        ui.add_space(8.0);
        self.list.show(ui, shell, catalog, tracks, player, Rows::Mixed);
    }
}
