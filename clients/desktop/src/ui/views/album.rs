//! An album's page: its cover and facts, a Play button, and its tracks. Also the track
//! list used here and on artist pages. Keys: arrows or jk move, Enter plays from there.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::catalog::marks::Kind;
use super::track_table::{Column, Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Shell};

/// An album's columns (and which show to start with).
const ALBUM_COLUMNS: &[(Column, bool)] = &[
    (Column::Number, true),
    (Column::Cover, false),
    (Column::Title, true),
    (Column::Artist, true),
    (Column::Released, false),
    (Column::Plays, true),
    (Column::Format, false),
    (Column::Og, false),
    (Column::Length, true),
];

#[derive(Default)]
pub struct AlbumPage {
    list: TrackTable,
    /// The details panel is open.
    details: bool,
    /// This album and another side by side, to merge (edit mode), in place of the list.
    merge: Option<super::album_merge::AlbumMergeView>,
    /// "Merge into another album…" asked the palette for the other one.
    picking_merge: bool,
    /// Undoing merges into this album (in the details).
    undo: super::album_merge::UndoPanel,
    /// Its sources (edit mode).
    sources: crate::ui::edit::sources::SourcesEditor,
    /// Its covers: order, trash, copies, pasted ones (edit mode).
    covers: crate::ui::edit::cover_list::CoverList,
    /// The groupings it's in.
    part_of: crate::ui::edit::part_of::PartOf,
}

impl AlbumPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        // Ctrl+M: merge into another album (again: close it). In a text field too. With
        // tracks marked, it merges those instead (the list takes it).
        if !shell.palette_open && !self.list.takes_ctrl_m(shell) && ui.ctx().input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if !shell.edit_mode {
                shell.playlists.notice = Some(("Merging is in edit mode: e, then Ctrl+M.".into(), std::time::Instant::now()));
            } else if self.merge.is_some() {
                self.merge = None;
            } else {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
        }
        let Some(album) = catalog.album(id) else {
            ui.label(RichText::new("This album isn't in the library (anymore).").color(P.muted));
            return;
        };
        let tracks = catalog.album_tracks(id);

        // Header: a big cover, and what the album is.
        let header = ui.allocate_exact_size(Vec2::new(ui.available_width(), 232.0), Sense::hover()).0;
        let cover_rect = Rect::from_min_size(header.min, Vec2::splat(220.0));
        shell.cover_with_original(ui, album.cover(), album.id, cover_rect, 12, 0.6);
        let left = cover_rect.right() + 28.0;
        let painter = ui.painter();
        painter.text(Pos2::new(left, cover_rect.top() + 70.0), Align2::LEFT_BOTTOM, album.kind.to_uppercase(), font(11.0, semibold()), P.accent);
        widgets::text_in(painter, Rect::from_min_size(Pos2::new(left, cover_rect.top() + 76.0), Vec2::new(header.right() - left, 44.0)), &album.title, font(36.0, bold()), P.text);
        let mut facts = Vec::new();
        if let Some(date) = &album.release_date {
            facts.push(date.clone());
        }
        let count = tracks.len();
        facts.push(format!("{count} track{}", if count == 1 { "" } else { "s" }));
        let total: i64 = tracks.iter().filter_map(|&t| catalog.tracks[t as usize].duration_ms).sum();
        facts.push(widgets::clock(Duration::from_millis(total.max(0) as u64)));
        painter.text(Pos2::new(left, cover_rect.top() + 150.0), Align2::LEFT_TOP, facts.join("  ·  "), font(14.0, medium()), P.muted);

        // The album's artists, each a link.
        let artists_rect = Rect::from_min_size(Pos2::new(left, cover_rect.top() + 126.0), Vec2::new(header.right() - left, 20.0));
        let mut artists_ui = ui.new_child(egui::UiBuilder::new().max_rect(artists_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        super::artist_links(&mut artists_ui, shell, catalog, &album.credits, &album.artist_display, font(15.0, medium()), P.text);

        let play_rect = Rect::from_min_size(Pos2::new(left, cover_rect.bottom() - 44.0), Vec2::new(116.0, 42.0));
        if play_button(ui, "album-play", play_rect) && !tracks.is_empty() {
            shell.act(Action::Play { tracks: tracks.to_vec(), start: 0 });
        }
        let marks_rect = Rect::from_min_size(Pos2::new(play_rect.right() + 18.0, play_rect.top()), Vec2::new(460.0, play_rect.height()));
        let mut marks_ui = ui.new_child(egui::UiBuilder::new().max_rect(marks_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        marks(&mut marks_ui, shell, Kind::Album, id);
        marks_ui.add_space(8.0);
        if widgets::chip(&mut marks_ui, "Details", self.details) {
            self.details = !self.details;
        }
        marks_ui.add_space(4.0);
        widgets::share_chip(&mut marks_ui, shell, crate::links::Target::Album(id));
        // Its original files (og files): a chip that shows only those (`og:` in the filter).
        let og_count = tracks.iter().filter(|&&t| catalog.tracks[t as usize].original_filename.is_some()).count();
        if og_count > 0 {
            marks_ui.add_space(4.0);
            if widgets::chip(&mut marks_ui, &format!("Original files  {og_count}"), self.list.og_only()) {
                self.list.toggle_og();
            }
        }
        if shell.edit_mode && self.merge.is_none() {
            marks_ui.add_space(8.0);
            let tip = "The same album twice: choose the other one, then see both before merging";
            if marks_ui.button("Merge into another album…  (Ctrl+M)").on_hover_text(tip).clicked() {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Album, except: Some(id) });
            }
        }
        ui.add_space(10.0);
        let groupings = album.groupings.clone();
        self.part_of.show(ui, shell, catalog, crate::ui::edit::part_of::Member::Album(id), &groupings);
        ui.add_space(12.0);

        // The other album, picked in the palette (not a new one, not this one).
        if self.picking_merge
            && let Some(picked) = shell.picker.take()
        {
            self.picking_merge = false;
            if let (crate::ui::edit::PickKind::Album, Some(other)) = (picked.kind, picked.id)
                && other != id
            {
                self.merge = Some(super::album_merge::AlbumMergeView::new(id, other));
            }
        }
        if let Some(view) = self.merge.as_mut() {
            use super::album_merge::Outcome;
            shell.form_dirty = true;
            match view.show(ui, shell, catalog) {
                Some(Outcome::Merged { kept, gone }) => {
                    self.merge = None;
                    shell.album_details.refresh(kept);
                    shell.album_details.refresh(gone);
                    shell.act(Action::RefreshLibrary);
                    if kept != id {
                        shell.act(Action::Go(crate::ui::Page::Album(kept)));
                    }
                }
                Some(Outcome::Closed) => self.merge = None,
                None => {}
            }
            return;
        }
        if self.details {
            details_panel(ui, "album-details", |ui| {
                let cached = shell.album_details.get(id);
                let Some(d) = cached.value.clone() else {
                    ui.label(RichText::new(if cached.refreshing { "Loading…" } else { "Couldn't load the details" }).color(P.muted));
                    return;
                };
                if !d.credits.is_empty() {
                    ui.label(RichText::new("Credits").font(font(14.0, semibold())));
                    for c in &d.credits {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&c.role).color(P.faint));
                            let name = c.alias.clone().unwrap_or_else(|| c.artist_name.clone());
                            if widgets::link(ui, &name, font(14.0, medium()), P.text) {
                                shell.act(Action::Go(crate::ui::Page::Artist(c.artist_id)));
                            }
                        });
                    }
                    ui.add_space(8.0);
                }
                if !d.covers.is_empty() {
                    ui.label(RichText::new(format!("Covers ({})", d.covers.len())).font(font(14.0, semibold())));
                    ui.horizontal_wrapped(|ui| {
                        for c in &d.covers {
                            let (rect, response) = ui.allocate_exact_size(Vec2::splat(110.0), Sense::click());
                            shell.cover(ui, c.cover(), c.id, rect, 6, 0.1);
                            let size = match (c.width, c.height) {
                                (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                                _ => c.source.clone(),
                            };
                            let response = response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text(format!("{size} · click: the original"));
                            if response.clicked()
                                && let Some(cover) = c.cover()
                            {
                                shell.open_original(ui.ctx(), cover);
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                if !d.sources.is_empty() {
                    ui.label(RichText::new(format!("Sources ({})", d.sources.len())).font(font(14.0, semibold())));
                    super::track::sources(ui, &d.sources);
                }
                if !d.merges.is_empty() {
                    ui.label(RichText::new("Merged into this album").font(font(14.0, semibold())));
                    let can = shell.account.can("edit_metadata");
                    self.undo.show(ui, shell, id, &d.merges, can);
                }
            });
            ui.add_space(12.0);
        }

        // Edit mode: the album's own data (title, kind, date, artists).
        if shell.edit_mode {
            // A track or album picked to copy a cover from (before the form takes it).
            self.covers.take_pick(shell);
            edit_panel(ui, shell, catalog, id);
            ui.add_space(12.0);
            if let Some(d) = shell.album_details.get(id).value.clone() {
                details_panel(ui, "album-covers", |ui| {
                    ui.label(RichText::new("Covers").font(font(14.0, semibold())));
                    self.covers.show(ui, shell, crate::ui::edit::cover_list::Owner::Album(id), &d.covers, true);
                });
                ui.add_space(12.0);
                details_panel(ui, "album-sources", |ui| {
                    ui.label(RichText::new("Sources").font(font(14.0, semibold())));
                    self.sources.show(ui, shell, crate::ui::edit::sources::Owner::Album(id), &d.sources);
                });
                ui.add_space(12.0);
            }
        }

        if tracks.is_empty() {
            ui.label(RichText::new("No tracks you can see").color(P.muted));
            return;
        }
        let rows = TrackRows { tracks: Some(tracks), added: None };
        let options = Options { key: "album", columns: ALBUM_COLUMNS, added: None, filter_hint: None, album: Some((id, &album.artist_display)), keys: true, scroll: true };
        self.list.show(ui, shell, catalog, &rows, shell.generation, player, &options);
    }
}

/// The album's form (edit mode). Kept in the shell per album, so changes not saved yet
/// are still there after looking elsewhere.
fn edit_panel(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid) {
    use crate::ui::edit::album_form::AlbumForm;
    let cached = shell.album_details.get(id);
    let Some(d) = cached.value.clone() else {
        ui.label(RichText::new(if cached.refreshing { "Loading the album to edit…" } else { "Couldn't load the album to edit" }).color(P.muted));
        return;
    };
    details_panel(ui, "album-edit", |ui| {
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let Some(mut form) = shell.album_forms.remove(&id).or_else(|| AlbumForm::from_detail(&d)) else {
            ui.label(RichText::new("This album has no revision to build on, so it can't be edited here.").color(P.muted));
            return;
        };
        form.show(ui, shell, Some(catalog), &d);
        shell.album_forms.insert(id, form);
    });
}


/// A star button and a rating for a track, album or artist (the header's).
pub fn marks(ui: &mut egui::Ui, shell: &mut Shell, kind: Kind, id: Uuid) {
    let mark = shell.mark(id);
    ui.spacing_mut().item_spacing.x = 12.0;
    if widgets::star_button(ui, mark.starred, 26.0) {
        shell.act(Action::ToggleStar(kind, id));
    }
    if let Some(rating) = widgets::rating(ui, mark.rating, 20.0) {
        shell.act(Action::Rate(kind, id, rating));
    }
    if mark.play_count > 0 {
        let plays = if mark.play_count == 1 { "1 play".to_string() } else { format!("{} plays", crate::ui::thousands(mark.play_count as usize)) };
        ui.label(RichText::new(plays).font(font(12.5, medium())).color(P.faint));
    }
}

/// A panel for an album's or artist's details: framed, scrolling if long.
pub fn details_panel(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::ScrollArea::vertical().id_salt(id).max_height(300.0).show(ui, add);
    });
}

/// The big round-cornered Play button.
pub fn play_button(ui: &mut egui::Ui, id: &str, rect: Rect) -> bool {
    let response = ui.interact(rect, egui::Id::new(id), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    ui.painter().rect_filled(rect.expand(hover * 2.0), 21, P.accent);
    widgets::draw_icon(ui.painter(), widgets::Icon::Play, rect.left_center() + Vec2::new(28.0, 0.0), 7.0, P.bg);
    ui.painter().text(rect.left_center() + Vec2::new(44.0, 0.0), Align2::LEFT_CENTER, "Play", font(14.0, semibold()), P.bg);
    response.clicked()
}

