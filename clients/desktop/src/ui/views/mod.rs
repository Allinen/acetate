//! The pages, one module each, and the pieces around them (sidebar, player bar, login).
//! Each page keeps its own state (filter, sort, selection) here, so going back to a page
//! finds it as it was left.

pub mod album;
pub mod albums;
pub mod artist;
pub mod artists;
pub mod login;
pub mod now_playing;
pub mod palette;
pub mod player_bar;
pub mod playlist;
pub mod sidebar;
pub mod track_table;
pub mod tracks;

use eframe::egui::{self, RichText};

use super::theme::{self, PALETTE as P};
use super::{Page, Shell};
use crate::playback::spectrum::Spectrum;
use crate::playback::PlayerView;

/// The id of the current page's filter field (`/` focuses it).
pub const FILTER_ID: &str = "filter";

#[derive(Default)]
pub struct Pages {
    pub login: login::Login,
    pub palette: palette::Palette,
    albums: albums::Albums,
    artists: artists::Artists,
    tracks: tracks::Tracks,
    album: album::AlbumPage,
    artist: artist::ArtistPage,
    playlist: playlist::PlaylistPage,
}

impl Pages {
    /// A page was opened (not returned to): album and artist pages start at the top.
    pub fn opened(&mut self, page: Page) {
        match page {
            Page::Album(_) => self.album = Default::default(),
            Page::Artist(_) => self.artist = Default::default(),
            Page::Playlist(_) | Page::Liked => self.playlist = Default::default(),
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, page: Page, player: &PlayerView, spectrum: &mut Spectrum, dt: f32) {
        if page == Page::NowPlaying {
            now_playing::show(ui, shell, player, spectrum, dt);
            return;
        }
        let Some(catalog) = shell.catalog.value.clone() else {
            ui.label(RichText::new(shell.library_status()).color(P.muted));
            return;
        };
        match page {
            Page::Albums => self.albums.show(ui, shell, &catalog),
            Page::Artists => self.artists.show(ui, shell, &catalog),
            Page::Tracks => self.tracks.show(ui, shell, &catalog, player),
            Page::Album(id) => self.album.show(ui, shell, &catalog, id, player),
            Page::Artist(id) => self.artist.show(ui, shell, &catalog, id, player),
            Page::Liked => self.playlist.show_liked(ui, shell, &catalog, player),
            Page::Playlist(id) => self.playlist.show(ui, shell, &catalog, id, player),
            Page::NowPlaying => {}
        }
    }
}

/// A page's title with a count next to it.
fn heading(ui: &mut egui::Ui, title: &str, count: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(theme::font(30.0, theme::bold())));
        ui.label(RichText::new(super::thousands(count)).color(P.faint).font(theme::font(16.0, theme::medium())));
    });
}

/// The filter field. Enter, Esc or ↓ leave it (so the arrows move through the list
/// again). Returns whether the text changed.
fn filter(ui: &mut egui::Ui, text: &mut String, hint: &str) -> bool {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(FILTER_ID))
            .hint_text(format!("{hint}   /"))
            .desired_width(360.0)
            .margin(egui::Margin::symmetric(12, 8)),
    );
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::ArrowDown)) {
        response.surrender_focus();
    }
    response.changed()
}
