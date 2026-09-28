//! Every track, as a table (see `track_table`), newest first.

use std::sync::Arc;

use eframe::egui;

use super::track_table::{Column, Options, TrackTable};
use crate::catalog::index::{TrackRows, TrackSort};
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::Shell;

/// The columns of a list of any tracks (and which show to start with).
pub const LIBRARY_COLUMNS: &[(Column, bool)] = &[
    (Column::Cover, true),
    (Column::Title, true),
    (Column::Artist, true),
    (Column::Album, true),
    (Column::Released, true),
    (Column::Plays, true),
    (Column::Format, true),
    (Column::Length, true),
];

pub struct Tracks {
    table: TrackTable,
}

impl Default for Tracks {
    fn default() -> Self {
        Tracks { table: TrackTable::new(TrackSort::Added) }
    }
}

impl Tracks {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, player: &PlayerView) {
        super::heading(ui, "Tracks", self.table.shown());
        ui.add_space(10.0);
        let rows = TrackRows { tracks: None, added: None };
        let options = Options { key: "tracks", columns: LIBRARY_COLUMNS, added: None, filter_hint: Some("Filter by title, artist or album"), album: None };
        self.table.show(ui, shell, catalog, &rows, shell.generation, player, &options);
    }
}
