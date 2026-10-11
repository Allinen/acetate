//! The pages, one module each, and the pieces around them (sidebar, player bar, login).
//! Each page keeps its own state (filter, sort, selection) here, so going back to a page
//! finds it as it was left.

pub mod album;
pub mod album_commands;
pub mod album_merge;
pub mod albums;
pub mod artist;
pub mod artist_commands;
pub mod artist_merge;
pub mod artists;
pub mod grouping;
pub mod incompatible;
pub mod groupings;
pub mod history;
pub mod login;
pub mod now_playing;
pub mod page_commands;
pub mod palette;
pub mod player_bar;
pub mod playlist;
pub mod merge;
pub mod queue;
pub mod review;
pub mod review_bulk;
pub mod unlinked_extras;
pub mod credited_as;
pub mod extra;
pub mod extra_drafts;
pub mod mark_og;
pub mod marking;
pub mod not_imported;
pub mod settings;
pub mod settings_account;
pub mod settings_computer;
pub mod settings_general;
pub mod settings_mirror;
pub mod settings_server;
pub mod settings_sound;
pub mod settings_vocabulary;
pub mod sidebar;
pub mod track_table;
pub mod trash;
pub mod changes;
pub mod track;
pub mod track_commands;
pub mod to_collective;
pub mod to_extra;
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
    pub incompatible: incompatible::Incompatible,
    pub palette: palette::Palette,
    albums: albums::Albums,
    artists: artists::Artists,
    tracks: tracks::Tracks,
    album: album::AlbumPage,
    artist: artist::ArtistPage,
    playlist: playlist::PlaylistPage,
    track: track::TrackPage,
    review: review::Review,
    queue: queue::QueuePage,
    pub settings: settings::SettingsPage,
    pub trash: trash::TrashPage,
    pub changes: changes::ChangesPage,
    grouping: grouping::GroupingPage,
    groupings: groupings::GroupingsPage,
    extra: extra::ExtraPage,
}

impl Pages {
    /// A page was opened (not returned to): album and artist pages start at the top.
    pub fn opened(&mut self, page: Page) {
        match page {
            Page::Album(_) => self.album = Default::default(),
            Page::Artist(_) => self.artist = Default::default(),
            Page::Grouping(_) => self.grouping = Default::default(),
            Page::Extra(_) => self.extra = Default::default(),
            Page::Playlist(_) | Page::Liked => self.playlist = Default::default(),
            Page::Track(_) => self.track = Default::default(),
            Page::Queue => self.queue.opened(),
            Page::Review => self.review.opened(),
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, page: Page, player: &PlayerView, spectrum: &mut Spectrum, dt: f32) {
        if page == Page::NowPlaying {
            now_playing::show(ui, shell, player, spectrum, dt);
            return;
        }
        if page == Page::Queue {
            self.queue.show(ui, shell, player);
            return;
        }
        if page == Page::Settings {
            self.settings.show(ui, shell, player);
            return;
        }
        if page == Page::Trash {
            if shell.connection.is_offline() {
                offline(ui, "The trash");
                return;
            }
            self.trash.show(ui, shell);
            return;
        }
        if page == Page::Changes && shell.connection.is_offline() {
            offline(ui, "Changes");
            return;
        }
        if page == Page::Review && shell.connection.is_offline() {
            offline(ui, "Review");
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
            Page::Track(id) => self.track.show(ui, shell, &catalog, id, player),
            Page::Review => self.review.show(ui, shell, &catalog, player),
            Page::Grouping(id) => self.grouping.show(ui, shell, &catalog, id, player),
            Page::Groupings => self.groupings.show(ui, shell, &catalog),
            Page::Extra(id) => self.extra.show(ui, shell, id),
            Page::Changes => self.changes.show(ui, shell, &catalog),
            Page::NowPlaying | Page::Queue | Page::Settings | Page::Trash => {}
        }
    }
}

/// A page that needs the server, while it's away.
pub fn offline(ui: &mut egui::Ui, what: &str) {
    ui.add_space(24.0);
    ui.label(RichText::new("Offline").font(theme::font(24.0, theme::bold())));
    ui.add_space(6.0);
    ui.label(RichText::new(format!("{what} needs the server, which can't be reached. What's on this computer still plays; the server is asked again now and then (or Try now, in the sidebar).")).color(P.muted));
}

/// Credited artists (producers left out) as links to their pages, each as credited
/// ("credited as" names as written), or `fallback` if there are no credits.
pub fn artist_links(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    catalog: &crate::catalog::Catalog,
    credits: &[crate::catalog::model::Credit],
    fallback: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    use crate::catalog::model::Role;
    ui.spacing_mut().item_spacing.x = 0.0;
    let shown: Vec<_> = credits.iter().filter(|c| c.1 != Role::Producer).collect();
    if shown.is_empty() {
        ui.add(egui::Label::new(RichText::new(fallback).font(font).color(color)).truncate());
        return;
    }
    for (i, credit) in shown.iter().enumerate() {
        if i > 0 {
            // "feat." before the first featured artist, else a comma.
            let joiner = if credit.1 == Role::Featured && shown[i - 1].1 != Role::Featured { " feat. " } else { ", " };
            ui.label(RichText::new(joiner).font(font.clone()).color(P.faint));
        }
        let name = credit.2.as_deref().map(str::to_string).or_else(|| catalog.artist(credit.0).map(|a| a.name.clone())).unwrap_or_default();
        if super::widgets::link(ui, &name, font.clone(), color) {
            shell.act(super::Action::Go(Page::Artist(credit.0)));
        }
    }
}

/// The original filename of a track's file, if it's an original file (an og file).
pub fn original_filename(shell: &Shell, track: uuid::Uuid) -> Option<String> {
    let catalog = shell.catalog.value.as_ref()?;
    catalog.track_index(track).and_then(|i| catalog.track_at(i).original_filename().map(str::to_string))
}

/// What plays, as links: the title to its page, the artists to theirs (from the
/// catalog's credits; plain text for a track the catalog doesn't have).
#[allow(clippy::too_many_arguments)]
pub fn playing_links(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    track: uuid::Uuid,
    title: &str,
    artist: &str,
    title_rect: egui::Rect,
    artist_rect: egui::Rect,
    title_font: egui::FontId,
    artist_font: egui::FontId,
) {
    let catalog = shell.catalog.value.clone();
    let t = catalog.as_deref().and_then(|c| c.track(track));
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let mut title_ui = ui.new_child(egui::UiBuilder::new().max_rect(title_rect).layout(layout));
    match t {
        Some(t) => {
            // An original file's track: its title as in track lists (amber, a wave on hover).
            let clicked = match &t.original_filename() {
                Some(name) => super::widgets::og_link(&mut title_ui, title, title_font, name),
                None => super::widgets::link(&mut title_ui, title, title_font, P.text),
            };
            if clicked {
                shell.act(super::Action::Go(Page::Track(t.id())));
            }
        }
        None => {
            title_ui.add(egui::Label::new(RichText::new(title).font(title_font).color(P.text)).truncate());
        }
    }
    let mut artist_ui = ui.new_child(egui::UiBuilder::new().max_rect(artist_rect).layout(layout));
    match (t, catalog.as_deref()) {
        (Some(t), Some(c)) => artist_links(&mut artist_ui, shell, c, t.credits(), artist, artist_font, P.muted),
        _ => {
            artist_ui.add(egui::Label::new(RichText::new(artist).font(artist_font).color(P.muted)).truncate());
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
/// again). Arriving on a page puts the cursor here. Returns whether the text changed.
fn filter(ui: &mut egui::Ui, text: &mut String, hint: &str) -> bool {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .id(egui::Id::new(FILTER_ID))
            .hint_text(format!("{hint}   /"))
            .desired_width(360.0)
            .margin(egui::Margin::symmetric(12, 8)),
    );
    // Not drawn last pass means the page was just opened. (Keyed by the hint: every page
    // shares the field's id, so going from one page to another has no gap in it.)
    let pass = ui.ctx().cumulative_pass_nr();
    let seen = egui::Id::new(("filter_seen_pass", hint));
    let last: Option<u64> = ui.ctx().data_mut(|d| d.get_temp(seen));
    ui.ctx().data_mut(|d| d.insert_temp(seen, pass));
    if last.is_none_or(|l| l + 1 < pass) {
        response.request_focus();
    }
    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::ArrowDown)) {
        response.surrender_focus();
    }
    response.changed()
}
