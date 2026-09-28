//! An artist's page: the name and other names they go by, their releases (newest first,
//! a strip of covers), and every track they're credited on (featured and produced ones
//! too). Keys: arrows or jk move through the tracks, Enter plays from there.

use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use super::album::play_button;
use super::track_table::{Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Default)]
pub struct ArtistPage {
    list: TrackTable,
    /// The details panel is open.
    details: bool,
    /// This artist and another side by side, to merge (edit mode), in place of the rest.
    merge: Option<super::artist_merge::ArtistMergeView>,
    /// "Merge into another artist…" asked the palette for the other one.
    picking_merge: bool,
}

impl ArtistPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let ctx = ui.ctx().clone();
        // Ctrl+M: merge into another artist (again: close it). In a text field too.
        if !shell.palette_open && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if !shell.edit_mode {
                shell.playlists.notice = Some(("Merging is in edit mode: e, then Ctrl+M.".into(), std::time::Instant::now()));
            } else if self.merge.is_some() {
                self.merge = None;
            } else {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: Some(id) });
            }
        }
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
            if shell.edit_mode && self.merge.is_none() {
                ui.add_space(8.0);
                let tip = "The same artist twice: choose the other one, then see both before merging";
                if ui.button("Merge into another artist…  (Ctrl+M)").on_hover_text(tip).clicked() {
                    self.picking_merge = true;
                    shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: Some(id) });
                }
            }
        });
        ui.add_space(22.0);

        // The other artist, picked in the palette (not a new one, not this one).
        if self.picking_merge
            && let Some(picked) = shell.picker.take()
        {
            self.picking_merge = false;
            if let (crate::ui::edit::PickKind::Artist, Some(other)) = (picked.kind, picked.id)
                && other != id
            {
                self.merge = Some(super::artist_merge::ArtistMergeView::new(id, other));
            }
        }
        if let Some(view) = self.merge.as_mut() {
            use super::artist_merge::Outcome;
            shell.form_dirty = true;
            match view.show(ui, shell, catalog) {
                Some(Outcome::Merged { kept, gone }) => {
                    self.merge = None;
                    shell.artist_details.refresh(kept);
                    shell.artist_details.refresh(gone);
                    shell.flags.refresh(&shell.api, shell.runtime.handle(), &shell.ctx);
                    shell.act(Action::RefreshLibrary);
                    if kept != id {
                        shell.act(Action::Go(Page::Artist(kept)));
                    }
                }
                Some(Outcome::Closed) => self.merge = None,
                None => {}
            }
            return;
        }
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

        // Edit mode: the artist's name, disambiguation and other names.
        if shell.edit_mode {
            edit_panel(ui, shell, id);
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
        // Newest release first (the artist's own order).
        let rows = TrackRows { tracks: Some(tracks), added: None };
        let options = Options { key: "artist", columns: super::tracks::LIBRARY_COLUMNS, added: None, filter_hint: Some("Filter: words, or album:… year:… status:…"), album: None };
        self.list.show(ui, shell, catalog, &rows, shell.generation, player, &options);
    }
}

/// The artist's form (edit mode). Kept in the shell per artist, so changes not saved yet
/// are still there after looking elsewhere.
fn edit_panel(ui: &mut egui::Ui, shell: &mut Shell, id: Uuid) {
    use crate::ui::edit::artist_form::ArtistForm;
    let cached = shell.artist_details.get(id);
    let Some(d) = cached.value.clone() else {
        ui.label(RichText::new(if cached.refreshing { "Loading the artist to edit…" } else { "Couldn't load the artist to edit" }).color(P.muted));
        return;
    };
    super::album::details_panel(ui, "artist-edit", |ui| {
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let Some(mut form) = shell.artist_forms.remove(&id).or_else(|| ArtistForm::from_detail(&d)) else {
            ui.label(RichText::new("This artist has no revision to build on, so it can't be edited here.").color(P.muted));
            return;
        };
        form.show(ui, shell, &d);
        shell.artist_forms.insert(id, form);
    });
}
