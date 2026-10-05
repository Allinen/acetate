//! An artist's page: the name and other names they go by, their releases (newest first,
//! a strip of covers), and every track they're credited on (featured and produced ones
//! too). Keys: arrows or jk move through the tracks, Enter plays from there.

use std::sync::Arc;

use eframe::egui::{self, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use super::album::play_button;
use super::artist_commands::{Act, ArtistCommands, Filters, Here, Reveal};
use super::track_table::{Options, TrackTable};
use crate::catalog::index::TrackRows;
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{thousands, widgets, Action, Page, Shell};

#[derive(Default)]
pub struct ArtistPage {
    /// Its notes (folded under their titles), and writing them in edit mode.
    notes: crate::ui::edit::notes::NotesView,
    /// Its extras, and attaching them in edit mode.
    extras: crate::ui::edit::extras::ExtrasView,
    /// Its tracks (as a primary artist), those it's featured on, those it produced.
    list: TrackTable,
    featured: TrackTable,
    produced: TrackTable,
    /// Which of the three takes the keys (the one clicked last; 0 to start with).
    active: usize,
    /// The previews of the three (their first few rows), apart from the full lists (a
    /// filter typed in a full one doesn't narrow its preview unseen).
    previews: [TrackTable; 3],
    /// One section shown alone, all of it (the rest of the page out of the way).
    expanded: Option<Section>,
    /// "Turn into a collective…" open (edit mode).
    to_collective: Option<super::to_collective::ToCollective>,
    /// This artist and another side by side, to merge (edit mode), in place of the rest.
    merge: Option<super::artist_merge::ArtistMergeView>,
    /// "Merge into another artist…" asked the palette for the other one.
    picking_merge: bool,
    /// Its sources (edit mode).
    sources: crate::ui::edit::sources::SourcesEditor,
    /// Only what's credited under the names chosen (none: all), only original files (og
    /// files, and releases with one): the chips.
    filters: Filters,
    /// The commandline under the header.
    commands: ArtistCommands,
    /// The release chosen in the Releases section on its own (the keys move it).
    release: usize,
    /// A part the commandline asked for: opened and scrolled to (true), or a folded one
    /// closed (false).
    reveal: Option<(Reveal, bool)>,
    /// The folded parts drawn last time, and whether each was open.
    folds: Vec<(Reveal, bool)>,
    /// Its history (restoring asks first).
    history: super::history::History,
}

/// The parts of an artist's page, each shown as a preview, or alone and whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Releases,
    Tracks,
    Featured,
    Produced,
    Extras,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Section::Releases => "Releases",
            Section::Tracks => "Tracks",
            Section::Featured => "Featured on",
            Section::Produced => "Produced",
            Section::Extras => "Extras",
        }
    }
}

/// Rows a track section's preview shows.
const PREVIEW_ROWS: usize = 5;
/// Extras the page shows before "All N".
pub(super) const PREVIEW_EXTRAS: usize = 3;
/// The releases' cards.
const CARD: f32 = 150.0;
const CARD_GAP: f32 = 18.0;

/// A section's heading, a link to see all of it: true when clicked.
pub(super) fn section_heading(ui: &mut egui::Ui, title: &str, count: usize, more: bool) -> bool {
    let text = if more { format!("{title}  {}  ›", thousands(count)) } else { format!("{title}  {}", thousands(count)) };
    let response = ui.add(egui::Label::new(RichText::new(text).font(font(18.0, bold()))).sense(Sense::click()));
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("All of it, on its own (Esc: back)");
    ui.add_space(8.0);
    response.clicked()
}

/// Release cards, in rows as wide as the room (all of `releases`: the caller picks).
/// `selected`: the one the keys are on, and whether it just moved (it's scrolled to).
pub(super) fn release_cards(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, releases: &[u32], selected: Option<(usize, bool)>) {
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(CARD_GAP, CARD_GAP);
        for (n, &index) in releases.iter().enumerate() {
            let album = &catalog.albums[index as usize];
            let (rect, response) = ui.allocate_exact_size(Vec2::new(CARD, CARD + 48.0), Sense::click());
            let hover = ctx.animate_bool_with_time(response.id, response.hovered(), 0.16);
            let cover = Rect::from_min_size(rect.min + Vec2::new(0.0, 6.0 - hover * 4.0), Vec2::splat(CARD));
            shell.cover(ui, album.cover(), album.id, cover, 8, hover);
            if let Some((at, moved)) = selected
                && at == n
            {
                ui.painter().rect_stroke(cover.expand(3.0), 10, egui::Stroke::new(2.0, P.accent), egui::StrokeKind::Outside);
                shell.list_target = Some(crate::links::Target::Album(album.id));
                if moved {
                    response.scroll_to_me(None);
                }
            }
            let top = cover.bottom() + 8.0 + hover * 4.0;
            widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(CARD, 17.0)), &album.title, font(13.0, semibold()), P.text);
            let sub = match album.year {
                Some(year) => format!("{year} · {}", album.kind),
                None => album.kind.clone(),
            };
            widgets::text_in(ui.painter(), Rect::from_min_size(Pos2::new(rect.left(), top + 18.0), Vec2::new(CARD, 15.0)), &sub, font(12.0, medium()), P.muted);
            widgets::share_menu(&response, shell, crate::links::Target::Album(album.id));
            if response.clicked() {
                shell.act(Action::Go(Page::Album(album.id)));
            }
        }
    });
}

/// All of `releases` on their own, as a grid the keys move through (arrows or hjkl;
/// Enter opens one, Shift+Enter plays it; s, +/-, q, p as in the Albums view).
pub(super) fn release_grid(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, releases: &[u32], selected: &mut usize, salt: impl std::hash::Hash + std::fmt::Debug) {
    let fit = (((ui.available_width() + CARD_GAP) / (CARD + CARD_GAP)) as usize).max(1);
    let page = ((ui.available_height() / (CARD + 48.0 + CARD_GAP)) as usize).max(1);
    let moved = widgets::move_selection(ui.ctx(), selected, releases.len(), fit, page);
    *selected = (*selected).min(releases.len().saturating_sub(1));
    if let Some(&a) = releases.get(*selected) {
        super::albums::album_keys(shell, catalog, &moved, catalog.albums[a as usize].id);
    }
    egui::ScrollArea::vertical().id_salt(salt).auto_shrink([false, false]).show(ui, |ui| {
        release_cards(ui, shell, catalog, releases, Some((*selected, moved.moved)));
    });
}

/// The names `artist` is credited under in `credits` (None: its own name).
fn names_in(credits: &[crate::catalog::model::Credit], artist: Uuid) -> impl Iterator<Item = Option<&str>> {
    credits.iter().filter(move |c| c.0 == artist).map(|c| c.2.as_deref())
}

/// Every name the artist is credited under on these tracks, with how many: its own name
/// first, then the others, most used first.
fn credited_names(catalog: &Catalog, tracks: &[u32], artist: Uuid) -> Vec<(Option<String>, usize)> {
    let mut counts: std::collections::HashMap<Option<String>, usize> = Default::default();
    for &t in tracks {
        let mut names: Vec<Option<&str>> = names_in(&catalog.tracks[t as usize].credits, artist).collect();
        names.dedup();
        for name in names {
            *counts.entry(name.map(str::to_string)).or_default() += 1;
        }
    }
    let mut names: Vec<(Option<String>, usize)> = counts.into_iter().collect();
    names.sort_by(|a, b| a.0.is_some().cmp(&b.0.is_some()).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    names
}

impl ArtistPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let ctx = ui.ctx().clone();
        // Ctrl+M: merge into another artist (again: close it). In a text field too. With
        // tracks marked, it merges those instead (the list takes it).
        let tables_take_it = [&self.list, &self.featured, &self.produced].into_iter().chain(&self.previews).any(|t| t.takes_ctrl_m(shell));
        if !shell.palette_open && !tables_take_it && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::M)) {
            if !shell.edit_mode {
                shell.playlists.notice = Some(("Merging is in edit mode: e, then Ctrl+M.".into(), std::time::Instant::now()));
            } else if self.merge.is_some() {
                self.merge = None;
            } else {
                self.picking_merge = true;
                shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: Some(id) });
            }
        }
        if let Some(popup) = self.to_collective.as_mut()
            && popup.show(shell)
        {
            self.to_collective = None;
        }
        let Some(artist) = catalog.artist(id) else {
            ui.label(RichText::new("This artist isn't in the library (anymore).").color(P.muted));
            return;
        };
        let all_tracks = catalog.artist_tracks(id);
        let all_albums = catalog.artist_albums(id);
        // Its extras (from its details), for the section at the bottom.
        let extras: Vec<crate::details::Extra> = shell.artist_details.get(id).value.as_ref().map(|d| d.extras.clone()).unwrap_or_default();
        // Only what's credited under the names chosen (if any are).
        let names = credited_names(catalog, all_tracks, id);
        if names.len() < 2 {
            self.filters.names.clear();
        }
        // Its original files (og files): a chip to see only those (and releases with one).
        let is_og = |t: u32| catalog.tracks[t as usize].original_filename.is_some();
        let og_count = all_tracks.iter().filter(|&&t| is_og(t)).count();
        if og_count == 0 {
            self.filters.og_only = false;
        }
        let details = shell.artist_details.get(id).value.clone();
        let (has_sources, has_notes) = details.as_ref().map_or((false, false), |d| (!d.sources.is_empty(), d.notes.iter().any(|n| !n.hidden)));
        let has_history = details.as_ref().is_some_and(|d| d.revisions.is_some());
        let folds_shown = self.folds.clone();
        let (edit_mode, may_collective) = (shell.edit_mode, shell.edit_mode && shell.account.can("edit_sources"));
        let may_edit = edit_mode && shell.account.can("edit_metadata");
        let (aliases, disambiguation, sources) = details.as_ref().map_or((&[][..], "", &[][..]), |d| (&d.aliases[..], d.disambiguation.as_str(), &d.sources[..]));
        let may_source = edit_mode && shell.account.can("edit_sources");
        let shapes: Vec<crate::account::SourceShape> = shell.account.vocab.as_ref().map(|v| v.source_shapes.clone()).unwrap_or_default();
        let may_access = edit_mode && shell.account.can("manage_access");
        let here = |sections| Here {
            id,
            own: &artist.name,
            names: &names,
            og_count,
            sections,
            has_sources,
            has_notes,
            has_history,
            folds: &folds_shown,
            edit_mode,
            may_edit,
            may_collective,
            aliases,
            disambiguation,
            sources,
            may_source,
            shapes: &shapes,
            extras: &extras,
            library_extras: &catalog.extras,
            may_access,
            notes: details.as_ref().map_or(&[][..], |d| &d.notes[..]),
        };
        // As the chips are, with what's being typed in the commandline on top.
        let filters = self.commands.preview(&here(&[]), &self.filters);
        let og_only = filters.og_only;
        // Every track it's credited on (under the names chosen, if any are), in any role.
        let tracks: Vec<u32> = all_tracks.iter().copied().filter(|&t| names_in(&catalog.tracks[t as usize].credits, id).any(|n| filters.shows(n)) && (!og_only || is_og(t))).collect();
        let tracks = &tracks[..];
        // What it's credited for, as what (and under the names chosen, if any are).
        use crate::catalog::model::Role;
        let credited = |credits: &[crate::catalog::model::Credit], role: Role| credits.iter().any(|c| c.0 == id && c.1 == role && filters.shows(c.2.as_deref()));
        let has_og = |a: u32| catalog.album_tracks(catalog.albums[a as usize].id).iter().any(|&t| is_og(t));
        let releases: Vec<u32> = all_albums.iter().copied().filter(|&a| credited(&catalog.albums[a as usize].credits, Role::Primary) && (!og_only || has_og(a))).collect();
        let by_role = |role: Role| -> Vec<u32> { all_tracks.iter().copied().filter(|&t| credited(&catalog.tracks[t as usize].credits, role) && (!og_only || is_og(t))).collect() };
        let (own, featured, produced) = (by_role(Role::Primary), by_role(Role::Featured), by_role(Role::Producer));
        let extras_shown = extras.iter().filter(|e| !e.hidden).count();
        let mut sections = vec![(Section::Releases, releases.len()), (Section::Tracks, own.len()), (Section::Featured, featured.len()), (Section::Produced, produced.len())];
        sections.retain(|(_, n)| *n > 0);
        if crate::ui::edit::extras::ExtrasView::worth_showing(shell, &extras) {
            sections.push((Section::Extras, extras_shown));
        }

        // Credited under several names: one chip each, on and off, to see only those
        // names'. And "Original files": only those (with or without names chosen).
        let name_chips = |ui: &mut egui::Ui, chosen: &mut Filters| {
            ui.horizontal_wrapped(|ui| {
                if names.len() >= 2 {
                    if widgets::chip(ui, &format!("All  {}", thousands(all_tracks.len())), filters.names.is_empty()) {
                        chosen.names.clear();
                    }
                    for (name, count) in &names {
                        let shown = name.as_deref().unwrap_or(&artist.name);
                        if widgets::chip(ui, &format!("as {shown}  {}", thousands(*count)), filters.names.contains(name)) {
                            chosen.toggle(name.clone());
                        }
                    }
                }
                if og_count > 0 {
                    if names.len() >= 2 {
                        ui.add_space(8.0);
                    }
                    if widgets::chip(ui, &format!("Original files  {}", thousands(og_count)), og_only) {
                        chosen.og_only = !chosen.og_only;
                    }
                }
            });
        };
        let chips_shown = names.len() >= 2 || og_count > 0;
        // Other names chosen are other rows (the version tells the tables to remake them).
        let version = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (shell.generation, &filters).hash(&mut hasher);
            hasher.finish()
        };

        // ---- One section alone, all of it ----
        if let Some(section) = self.expanded {
            // Esc or Backspace (outside a field): back to the whole page, not away from it.
            shell.page_takes_back = true;
            let back_key = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Backspace));
            let rows: &[u32] = match section {
                Section::Releases => &releases,
                Section::Tracks => &own,
                Section::Featured => &featured,
                Section::Produced => &produced,
                Section::Extras => &[],
            };
            let count = if section == Section::Extras { extras_shown } else { rows.len() };
            let mut back = back_key;
            ui.horizontal(|ui| {
                if ui.button(format!("‹ {}", artist.name)).on_hover_text("Back to the whole page (Esc)").clicked() {
                    back = true;
                }
                ui.label(RichText::new(format!("{}  {}", section.title(), thousands(count))).font(font(22.0, bold())));
            });
            // (The chips choose tracks: nothing to do with extras.)
            if chips_shown && section != Section::Extras {
                ui.add_space(4.0);
                name_chips(ui, &mut self.filters);
            }
            ui.add_space(10.0);
            if back {
                self.expanded = None;
                return;
            }
            match section {
                Section::Releases => release_grid(ui, shell, catalog, rows, &mut self.release, ("artist-releases-all", id)),
                Section::Extras => {
                    egui::ScrollArea::vertical().id_salt(("artist-extras-all", id)).auto_shrink([false, false]).show(ui, |ui| {
                        self.extras.show(ui, shell, crate::ui::edit::extras::Owner::Artist(id), &extras, false, true);
                    });
                }
                _ => {
                    let (table, key) = match section {
                        Section::Featured => (&mut self.featured, "artist-featured"),
                        Section::Produced => (&mut self.produced, "artist-produced"),
                        _ => (&mut self.list, "artist"),
                    };
                    let options = Options {
                        key,
                        columns: super::tracks::LIBRARY_COLUMNS,
                        added: None,
                        filter_hint: Some("Filter: words, or album:… year:… status:…"),
                        album: None,
                        places: None,
                        keys: true,
                        scroll: true,
                    };
                    let track_rows = TrackRows { tracks: Some(rows), added: None };
                    ui.push_id(key, |ui| table.show(ui, shell, catalog, &track_rows, version, player, &options));
                }
            }
            return;
        }

        // One scroll for the whole page (header, edit panels, notes, the previews as tall
        // as they are): nothing in it scrolls on its own.
        egui::ScrollArea::vertical().id_salt(("artist-page", id)).auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new("ARTIST").font(font(11.0, semibold())).color(P.accent));
            let name = artist.label();
            ui.label(RichText::new(name).font(font(40.0, bold())));
            if names.len() < 2 && !artist.aliases.is_empty() {
                ui.label(RichText::new(format!("Also credited as {}", artist.aliases.join(", "))).font(font(14.0, medium())).color(P.muted));
            }
            if chips_shown {
                ui.add_space(4.0);
                name_chips(ui, &mut self.filters);
                ui.add_space(4.0);
            }
            // The groupings its tracks are in (collectives it posts under, eras…), most first.
            let mut in_groupings: Vec<(Uuid, usize)> = Vec::new();
            for &t in all_tracks {
                for g in &catalog.tracks[t as usize].groupings {
                    match in_groupings.iter_mut().find(|(id, _)| id == g) {
                        Some((_, n)) => *n += 1,
                        None => in_groupings.push((*g, 1)),
                    }
                }
            }
            in_groupings.sort_by_key(|g| std::cmp::Reverse(g.1));
            // Member of: said by hand on the grouping (not worked out from credits).
            let member_of: Vec<&crate::catalog::model::Grouping> = catalog.groupings.iter().filter(|g| g.artists.contains(&id)).collect();
            if !member_of.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Member of ").font(font(14.0, medium())).color(P.faint));
                    for (n, g) in member_of.iter().enumerate() {
                        if n > 0 {
                            ui.label(RichText::new(", ").font(font(14.0, medium())).color(P.faint));
                        }
                        if widgets::link(ui, &g.name, font(14.0, medium()), P.text) {
                            shell.act(Action::Go(Page::Grouping(g.id)));
                        }
                        ui.label(RichText::new(format!(" ({})", g.kind)).font(font(14.0, medium())).color(P.faint));
                    }
                });
            }
            // Where its tracks are (an era, a collective it isn't a member of…).
            in_groupings.retain(|(g, _)| !member_of.iter().any(|m| m.id == *g));
            if !in_groupings.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Its tracks are in ").font(font(14.0, medium())).color(P.faint));
                    for (n, (g, count)) in in_groupings.iter().enumerate() {
                        let Some(grouping) = catalog.grouping(*g) else { continue };
                        if n > 0 {
                            ui.label(RichText::new(", ").font(font(14.0, medium())).color(P.faint));
                        }
                        if widgets::link(ui, &grouping.name, font(14.0, medium()), P.text) {
                            shell.act(Action::Go(Page::Grouping(*g)));
                        }
                        ui.label(RichText::new(format!(" ({}, {count})", grouping.kind)).font(font(14.0, medium())).color(P.faint));
                    }
                });
            }
            ui.label(
                RichText::new(format!("{} tracks · {} releases", thousands(own.len()), thousands(releases.len())))
                    .font(font(14.0, medium()))
                    .color(P.muted),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let (play_rect, _) = ui.allocate_exact_size(Vec2::new(116.0, 42.0), Sense::hover());
                // Its own tracks (or, with none, everything it's credited on).
                let to_play = if own.is_empty() { tracks } else { &own[..] };
                if play_button(ui, "artist-play", play_rect) && !to_play.is_empty() {
                    shell.act(Action::Play { tracks: to_play.to_vec(), start: 0 });
                }
                ui.add_space(6.0);
                super::album::marks(ui, shell, Kind::Artist, id);
                ui.add_space(8.0);
                widgets::share_chip(ui, shell, crate::links::Target::Artist(id));
                if shell.edit_mode && self.merge.is_none() {
                    ui.add_space(8.0);
                    let tip = "The same artist twice: choose the other one, then see both before merging";
                    if ui.button("Merge into another artist…  (Ctrl+M)").on_hover_text(tip).clicked() {
                        self.picking_merge = true;
                        shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: Some(id) });
                    }
                    let tip = "It's really an account several artists post under: make it a grouping (its tracks, albums and sources go there, its credits come off). Shows what it would do first.";
                    if shell.account.can("edit_sources") && ui.button("Turn into a collective, or merge into one…").on_hover_text(tip).clicked() {
                        self.to_collective = Some(super::to_collective::ToCollective::open(shell, id));
                    }
                }
            });
            ui.add_space(10.0);

            // The commandline (`/`): filters, the sections, and the rest by name.
            let to_play = if own.is_empty() { tracks.to_vec() } else { own.clone() };
            match self.commands.show(ui, shell, id, &here(&sections), &mut self.filters) {
                Some(Act::Expand(section, filter)) => {
                    self.expanded = Some(section);
                    let table = match section {
                        Section::Tracks => Some(&mut self.list),
                        Section::Featured => Some(&mut self.featured),
                        Section::Produced => Some(&mut self.produced),
                        _ => None,
                    };
                    if let Some(table) = table
                        && !filter.is_empty()
                    {
                        table.set_filter(&filter);
                    }
                }
                // A folded part that's open is closed; anything else opened and scrolled to.
                Some(Act::Reveal(reveal)) => self.reveal = Some((reveal, !self.folds.contains(&(reveal, true)))),
                Some(Act::Merge) => {
                    self.picking_merge = true;
                    shell.act(Action::Pick { kind: crate::ui::edit::PickKind::Artist, except: Some(id) });
                }
                Some(Act::Collective) => self.to_collective = Some(super::to_collective::ToCollective::open(shell, id)),
                Some(Act::ToggleEdit) => shell.act(Action::ToggleEditMode),
                Some(Act::Play) if !to_play.is_empty() => shell.act(Action::Play { tracks: to_play, start: 0 }),
                Some(Act::Note(note, title)) => {
                    let found = details.as_ref().and_then(|d| d.notes.iter().find(|n| Some(n.id) == note));
                    self.notes.open(crate::ui::edit::notes::Owner::Artist(id), found, &title);
                }
                Some(Act::PasteNotes) => {
                    let made = self.notes.add_files(crate::ui::edit::notes::Owner::Artist(id), crate::ui::files::clipboard_files());
                    if made == 0 {
                        shell.playlists.notice = Some(("No text files in the clipboard (copy .txt, .md or .nfo files in a file manager first).".into(), std::time::Instant::now()));
                    }
                }
                _ => {}
            }
            ui.add_space(18.0);

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
            // Listen mode: its sources, folded (as on an album's page). Edit mode has them
            // in its panel below. An id of their own (not this artist's): what's opened
            // stays open on the next artist.
            let mut folds = Vec::new();
            if !shell.edit_mode
                && let Some(d) = shell.artist_details.get(id).value.clone()
                && !d.sources.is_empty()
            {
                let set = self.reveal.take_if(|r| r.0 == Reveal::Sources).map(|r| r.1);
                let open = ui
                    .scope_builder(egui::UiBuilder::new().id(egui::Id::new("artist-sections")), |ui| {
                        super::track::section_set(ui, &format!("Sources ({})", d.sources.len()), false, set, |ui| super::track::sources(ui, &d.sources))
                    })
                    .inner;
                folds.push((Reveal::Sources, open));
            }

            // Edit mode: the artist's name, disambiguation and other names.
            if shell.edit_mode {
                edit_panel(ui, shell, id);
                ui.add_space(12.0);
                if let Some(d) = shell.artist_details.get(id).value.clone() {
                    let reveal = self.reveal.take_if(|r| r.0 == Reveal::Sources).is_some();
                    super::album::panel(ui, "artist-sources", |ui| {
                        let heading = ui.label(RichText::new("Sources").font(font(14.0, semibold())));
                        if reveal {
                            heading.scroll_to_me(Some(egui::Align::Min));
                        }
                        self.sources.show(ui, shell, crate::ui::edit::sources::Owner::Artist(id), &d.sources);
                    });
                    ui.add_space(12.0);
                }
            }

            // Its history, folded (both modes): every revision, what it changed, and why.
            if let Some(d) = shell.artist_details.get(id).value.clone()
                && let Some(revisions) = &d.revisions
            {
                let set = self.reveal.take_if(|r| r.0 == Reveal::History).map(|r| r.1);
                let names = std::collections::HashMap::new();
                let open = ui
                    .scope_builder(egui::UiBuilder::new().id(egui::Id::new("artist-history")), |ui| {
                        super::track::section_set(ui, &format!("History ({})", revisions.len()), false, set, |ui| {
                            self.history.show_revisions(ui, shell, catalog, super::history::Of::Artist(id), revisions, &names, |_| String::new());
                        })
                    })
                    .inner;
                folds.push((Reveal::History, open));
            }
            self.folds = folds;

            // Its notes, folded under their titles.
            if let Some(d) = shell.artist_details.get(id).value.clone()
                && crate::ui::edit::notes::NotesView::worth_showing(shell, &d.notes)
            {
                let heading = ui.label(RichText::new("Notes").font(font(16.0, semibold())));
                if self.reveal.take_if(|r| r.0 == Reveal::Notes).is_some() {
                    heading.scroll_to_me(Some(egui::Align::Min));
                }
                self.notes.show(ui, shell, crate::ui::edit::notes::Owner::Artist(id), &d.notes, true, true);
                ui.add_space(12.0);
            }

            if releases.is_empty() && own.is_empty() && featured.is_empty() && produced.is_empty() {
                let text = if !filters.names.is_empty() { "Nothing credited under these names." } else { "This artist has no tracks yet: go add some (Review, or a track's page in edit mode)." };
                ui.label(RichText::new(text).color(P.muted));
                return;
            }
            // The first shown takes the keys until another is clicked.
            let shown = [!own.is_empty(), !featured.is_empty(), !produced.is_empty()];
            if !shown[self.active] {
                self.active = shown.iter().position(|s| *s).unwrap_or(0);
            }

            // ---- Releases: one row of them (the heading shows all) ----
            if !releases.is_empty() {
                let fit = (((ui.available_width() + CARD_GAP) / (CARD + CARD_GAP)) as usize).max(1);
                if section_heading(ui, "Releases", releases.len(), releases.len() > fit) {
                    self.expanded = Some(Section::Releases);
                }
                release_cards(ui, shell, catalog, &releases[..releases.len().min(fit)], None);
                ui.add_space(18.0);
            }

            // ---- Its tracks, what it's featured on, what it produced: the first few ----
            let sections: [(Section, &[u32], &'static str); 3] =
                [(Section::Tracks, &own, "artist-preview"), (Section::Featured, &featured, "artist-featured-preview"), (Section::Produced, &produced, "artist-produced-preview")];
            for (n, (section, rows, key)) in sections.into_iter().enumerate() {
                if rows.is_empty() {
                    continue;
                }
                if section_heading(ui, section.title(), rows.len(), rows.len() > PREVIEW_ROWS) {
                    self.expanded = Some(section);
                }
                // As tall as its few rows: the page scrolls, never the list.
                let options = Options { key, columns: super::tracks::LIBRARY_COLUMNS, added: None, filter_hint: None, album: None, places: None, keys: self.active == n, scroll: false };
                let preview = &rows[..rows.len().min(PREVIEW_ROWS)];
                let track_rows = TrackRows { tracks: Some(preview), added: None };
                let shown = ui.push_id(key, |ui| self.previews[n].show(ui, shell, catalog, &track_rows, version, player, &options)).inner;
                if shown.clicked {
                    self.active = n;
                }
                if rows.len() > PREVIEW_ROWS && ui.small_button(format!("All {} ›", thousands(rows.len()))).clicked() {
                    self.expanded = Some(section);
                }
                ui.add_space(18.0);
            }

            // ---- Its extras (vocals, stems…): the first few, at the bottom ----
            if crate::ui::edit::extras::ExtrasView::worth_showing(shell, &extras) {
                let shown = extras.iter().filter(|e| !e.hidden).count();
                // (Always `›`: the section is there even with none, for attaching one.)
                if section_heading(ui, "Extras", shown, true) {
                    self.expanded = Some(Section::Extras);
                }
                // The first few shown (and those taken off, for edit mode's "Taken off").
                let preview: Vec<crate::details::Extra> = extras.iter().filter(|e| !e.hidden).take(PREVIEW_EXTRAS).chain(extras.iter().filter(|e| e.hidden)).cloned().collect();
                self.extras.show(ui, shell, crate::ui::edit::extras::Owner::Artist(id), &preview, false, true);
                if shown > PREVIEW_EXTRAS && ui.small_button(format!("All {} ›", thousands(shown))).clicked() {
                    self.expanded = Some(Section::Extras);
                }
                ui.add_space(18.0);
            }
        });
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
    super::album::panel(ui, "artist-edit", |ui| {
        // Taken out of the shell while it's shown (showing it needs the shell too).
        let Some(mut form) = shell.artist_forms.remove(&id).or_else(|| ArtistForm::from_detail(&d)) else {
            ui.label(RichText::new("This artist has no revision to build on, so it can't be edited here.").color(P.muted));
            return;
        };
        form.show(ui, shell, &d);
        shell.artist_forms.insert(id, form);
    });
}
