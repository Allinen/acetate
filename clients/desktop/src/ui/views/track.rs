//! A track's page: everything about it, calm by default (listen mode), the rest a click
//! away. The header says what it is (title, artists, the albums it's on, the release),
//! then sections: lyrics (synced ones follow playback), credits, relations, the file (and
//! where it came from, and earlier files), versions, sources, merges, covers and the
//! file's own tags. What's here comes from the catalog at once, and from the track's
//! details (kept on disk, refreshed in the background) as they arrive.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Vec2};
use uuid::Uuid;

use super::album::{marks, play_button};
use crate::catalog::marks::Kind;
use crate::catalog::Catalog;
use crate::details::{Source, TrackDetail};
use crate::ui::edit::track_form::TrackForm;
use crate::lyrics;
use crate::playback::PlayerView;
use crate::ui::theme::{bold, font, medium, semibold, PALETTE as P};
use crate::ui::{playing_track, widgets, Action, Page, Shell};

#[derive(Default)]
pub struct TrackPage {
    /// Which lyrics entry is shown (a track may have several: languages, official).
    lyrics: usize,
    /// The synced line last scrolled to (so it scrolls when the line changes, not always).
    followed_line: Option<usize>,
}

impl TrackPage {
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, id: Uuid, player: &PlayerView) {
        let cached = shell.track_details.get(id);
        let (detail, refreshing, error) = (cached.value.clone(), cached.refreshing, cached.error.clone());
        // A merged track answers with the one it went into: go there instead.
        if let Some(d) = &detail {
            if d.id != id {
                shell.act(Action::Back);
                shell.act(Action::Go(Page::Track(d.id)));
            }
        }
        let track = catalog.track_index(id).map(|i| &catalog.tracks[i as usize]);
        if track.is_none() && detail.is_none() {
            let text = error.unwrap_or_else(|| if refreshing { "Loading…".into() } else { "This track isn't in the library (anymore).".into() });
            ui.label(RichText::new(text).color(P.muted));
            return;
        }
        let is_playing = playing_track(player) == Some(id);

        egui::ScrollArea::vertical().id_salt("track-page").auto_shrink([false, false]).show(ui, |ui| {
            // ---- Header ----
            let header = ui.allocate_exact_size(Vec2::new(ui.available_width(), 232.0), Sense::hover()).0;
            let cover_rect = Rect::from_min_size(header.min, Vec2::splat(220.0));
            let cover = track.and_then(|t| t.cover()).or_else(|| detail.as_ref().and_then(|d| d.covers.first().and_then(|c| c.cover())));
            shell.cover(ui, cover, id, cover_rect, 12, 0.6);
            let left = cover_rect.right() + 28.0;
            let width = header.right() - left;
            let title = track.map(|t| t.title.clone()).or_else(|| detail.as_ref().map(|d| d.title.clone())).unwrap_or_default();
            let painter = ui.painter().clone();
            painter.text(Pos2::new(left, cover_rect.top() + 40.0), Align2::LEFT_BOTTOM, "TRACK", font(11.0, semibold()), P.accent);
            widgets::text_in(&painter, Rect::from_min_size(Pos2::new(left, cover_rect.top() + 44.0), Vec2::new(width, 44.0)), &title, font(34.0, bold()), P.text);

            // Artists, then the albums it's on, each a link.
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            let mut line = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(left, cover_rect.top() + 94.0), Vec2::new(width, 22.0))).layout(layout));
            if let Some(t) = track {
                super::artist_links(&mut line, shell, catalog, &t.credits, &t.artist_display, font(15.0, medium()), P.text);
            }
            let mut line = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(Pos2::new(left, cover_rect.top() + 120.0), Vec2::new(width, 20.0))).layout(layout));
            line.spacing_mut().item_spacing.x = 0.0;
            if let Some(t) = track {
                for (n, membership) in t.albums.iter().enumerate() {
                    let Some(album) = catalog.album(membership.0) else { continue };
                    line.label(RichText::new(if n == 0 { "on " } else { ", " }).font(font(13.5, medium())).color(P.faint));
                    if widgets::link(&mut line, &album.title, font(13.5, medium()), P.muted) {
                        shell.act(Action::Go(Page::Album(album.id)));
                    }
                    if let Some(number) = membership.2 {
                        let disc = membership.1.filter(|d| *d > 1).map(|d| format!("{d}-")).unwrap_or_default();
                        line.label(RichText::new(format!(" (#{disc}{number})")).font(font(13.5, medium())).color(P.faint));
                    }
                }
            }
            // Facts: the release date as written, length, status, genres.
            let mut facts = Vec::new();
            if let Some(date) = track.and_then(|t| t.release_date.clone()).or_else(|| detail.as_ref().and_then(|d| d.release_date.clone())) {
                facts.push(date);
            }
            facts.push(widgets::length(track.and_then(|t| t.duration_ms)));
            if let Some(status) = track.map(|t| t.status.as_str()).filter(|s| *s != "released") {
                facts.push(status.to_string());
            }
            if track.is_some_and(|t| t.is_restricted) {
                facts.push("restricted".into());
            }
            if let Some(genres) = track.map(|t| t.genres.join(", ")).filter(|g| !g.is_empty()) {
                facts.push(genres);
            }
            painter.text(Pos2::new(left, cover_rect.top() + 146.0), Align2::LEFT_TOP, facts.join("  ·  "), font(13.5, medium()), P.muted);

            let play_rect = Rect::from_min_size(Pos2::new(left, cover_rect.bottom() - 44.0), Vec2::new(116.0, 42.0));
            if play_button(ui, "track-play", play_rect) {
                // Play it, then the rest of its album (or just it).
                if let Some(index) = catalog.track_index(id) {
                    match track.and_then(|t| t.albums.first()) {
                        Some(album) => {
                            let tracks = catalog.album_tracks(album.0).to_vec();
                            let start = tracks.iter().position(|&t| t == index).unwrap_or(0);
                            shell.act(Action::Play { tracks, start });
                        }
                        None => shell.act(Action::Play { tracks: vec![index], start: 0 }),
                    }
                }
            }
            let marks_rect = Rect::from_min_size(Pos2::new(play_rect.right() + 18.0, play_rect.top()), Vec2::new(260.0, play_rect.height()));
            let mut marks_ui = ui.new_child(egui::UiBuilder::new().max_rect(marks_rect).layout(layout));
            marks(&mut marks_ui, shell, Kind::Track, id);
            ui.add_space(20.0);

            let Some(d) = detail else {
                let text = error.map(|e| format!("Couldn't load the details: {e}")).unwrap_or_else(|| "Loading the details…".into());
                ui.label(RichText::new(text).color(P.faint));
                return;
            };
            if let Some(err) = &error {
                ui.label(RichText::new(format!("Showing the copy kept here: couldn't refresh it ({err})")).color(P.faint).small());
            }

            // ---- Sections ----
            if shell.edit_mode {
                section(ui, "Edit", true, |ui| edit(ui, shell, catalog, &d));
            }
            if !d.lyrics.is_empty() {
                section(ui, "Lyrics", true, |ui| self.lyrics(ui, shell, &d, is_playing, player));
            }
            section(ui, "Credits", true, |ui| credits(ui, shell, &d));
            if !d.relations.is_empty() {
                section(ui, "Related tracks", true, |ui| relations(ui, shell, &d));
            }
            section(ui, "File", true, |ui| file(ui, shell, &d));
            if d.variants.len() > 1 {
                section(ui, &format!("Versions ({})", d.variants.len()), false, |ui| {
                    for v in &d.variants {
                        let primary = if Some(v.id) == d.primary_variant_id { "  · shown" } else { "" };
                        let title = v.data.get("title").and_then(|t| t.as_str()).unwrap_or("");
                        ui.label(RichText::new(format!("{}{primary}", v.label)).font(font(13.5, semibold())));
                        if !title.is_empty() {
                            ui.label(RichText::new(title).color(P.muted));
                        }
                        ui.add_space(4.0);
                    }
                });
            }
            if !d.sources.is_empty() {
                section(ui, &format!("Sources ({})", d.sources.len()), false, |ui| sources(ui, &d.sources));
            }
            if !d.merges.is_empty() {
                section(ui, "Merged into this track", false, |ui| {
                    for m in &d.merges {
                        let by = m.merged_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
                        let undone = if m.undone_at.is_some() { " · undone" } else { "" };
                        ui.label(format!("{} · {}{by}{undone}", m.from_title, day(&m.merged_at)));
                    }
                });
            }
            if d.covers.len() > 1 {
                section(ui, &format!("Covers ({})", d.covers.len()), false, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for c in &d.covers {
                            let (rect, response) = ui.allocate_exact_size(Vec2::splat(140.0), Sense::hover());
                            shell.cover(ui, c.cover(), c.id, rect, 8, 0.2);
                            let size = match (c.width, c.height) {
                                (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
                                _ => c.source.clone(),
                            };
                            response.on_hover_text(size);
                        }
                    });
                });
            }
            if d.file.embedded_tags.as_object().is_some_and(|t| !t.is_empty()) {
                section(ui, "Tags in the file", false, |ui| json_grid(ui, "embedded-tags", &d.file.embedded_tags));
            }
            ui.add_space(24.0);
        });
    }

    /// The lyrics: synced ones as lines, the current one lit (and kept in view) while
    /// this track plays; a click on a line jumps there. Several entries: chips to choose.
    fn lyrics(&mut self, ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail, is_playing: bool, player: &PlayerView) {
        if d.lyrics.len() > 1 {
            ui.horizontal(|ui| {
                for (n, l) in d.lyrics.iter().enumerate() {
                    let mut name = l.language.clone().unwrap_or_else(|| format!("Lyrics {}", n + 1));
                    if l.is_official {
                        name.push_str(" · official");
                    }
                    if widgets::chip(ui, &name, self.lyrics == n) {
                        self.lyrics = n;
                        self.followed_line = None;
                    }
                }
            });
            ui.add_space(8.0);
        }
        let Some(entry) = d.lyrics.get(self.lyrics.min(d.lyrics.len() - 1)) else { return };
        lyrics_lines(ui, shell, entry, is_playing, player, &mut self.followed_line, 15.0);
        if !is_playing && entry.is_synced {
            ui.add_space(4.0);
            ui.label(RichText::new("Play the track to follow along").color(P.faint).small());
        }
    }
}

/// Lyrics: synced ones as lines, the current one lit (and scrolled to the middle when it
/// changes; `followed` remembers the last) while the track plays, a click on a line
/// jumping there; others as text. `size`: the lines' font size.
pub fn lyrics_lines(
    ui: &mut egui::Ui,
    shell: &mut Shell,
    entry: &crate::details::Lyrics,
    is_playing: bool,
    player: &PlayerView,
    followed: &mut Option<usize>,
    size: f32,
) {
    let lines = if entry.is_synced { lyrics::parse(&entry.body) } else { Vec::new() };
    if lines.is_empty() {
        ui.add(egui::Label::new(RichText::new(&entry.body).font(font(size, medium())).color(P.text)).wrap());
        return;
    }
    let current = if is_playing { lyrics::current(&lines, player.position_now()) } else { None };
    if is_playing && !player.paused {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    for (n, line) in lines.iter().enumerate() {
        let now = current == Some(n);
        let color = if now { P.accent } else if current.is_some_and(|c| n < c) { P.faint } else { P.muted };
        let text = if line.text.is_empty() { "♪" } else { &line.text };
        let label = egui::Label::new(RichText::new(text).font(font(if now { size * 1.2 } else { size }, if now { semibold() } else { medium() })).color(color));
        let response = ui.add(label.wrap().sense(Sense::click()));
        if now && *followed != Some(n) {
            *followed = Some(n);
            response.scroll_to_me(Some(egui::Align::Center));
        }
        if is_playing && response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            shell.act(Action::SeekTo(line.at));
        }
    }
}

/// A section with a heading; closed ones open on a click.
fn section(ui: &mut egui::Ui, title: &str, open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).font(font(17.0, bold())))
        .id_salt(title.split(" (").next().unwrap_or(title))
        .default_open(open)
        .show(ui, |ui| {
            ui.add_space(4.0);
            add(ui);
            ui.add_space(8.0);
        });
    ui.add_space(6.0);
}

/// The track's form (edit mode). Kept in the shell per track, so changes not saved yet
/// are still there after looking elsewhere.
fn edit(ui: &mut egui::Ui, shell: &mut Shell, catalog: &Arc<Catalog>, d: &TrackDetail) {
    // Taken out of the shell while it's shown (showing it needs the shell too: Rust
    // allows only one mutable borrow of it at a time), then put back.
    let Some(mut form) = shell.track_forms.remove(&d.id).or_else(|| TrackForm::from_detail(d, Some(catalog))) else {
        ui.label(RichText::new("This track has no version to edit.").color(P.muted));
        return;
    };
    form.show(ui, shell, Some(catalog), d);
    shell.track_forms.insert(d.id, form);
}

/// Every credit, by role: the artist (a link) and the name it's credited under.
fn credits(ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail) {
    let roles: Vec<&str> = {
        let mut roles: Vec<&str> = d.credits.iter().map(|c| c.role.as_str()).collect();
        roles.dedup();
        roles
    };
    if roles.is_empty() {
        ui.label(RichText::new(format!("No credits (the artist text is “{}”)", d.artist_display)).color(P.muted));
    }
    egui::Grid::new("credits").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        for role in roles {
            ui.label(RichText::new(capitalized(role)).color(P.faint));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (n, c) in d.credits.iter().filter(|c| c.role == role).enumerate() {
                    if n > 0 {
                        ui.label(RichText::new(", ").color(P.faint));
                    }
                    let name = c.alias.clone().unwrap_or_else(|| c.artist_name.clone());
                    if widgets::link(ui, &name, font(14.0, medium()), P.text) {
                        shell.act(Action::Go(Page::Artist(c.artist_id)));
                    }
                    if c.alias.is_some() {
                        ui.label(RichText::new(format!(" (credited as, for {})", c.artist_name)).color(P.faint).small());
                    }
                }
            });
            ui.end_row();
        }
    });
}

/// Relations as sentences: "%A is a remix of %B", the other track a link.
fn relations(ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail) {
    for r in &d.relations {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let sentence = r.template.replace("%A", "This track");
            let (before, after) = sentence.split_once("%B").unwrap_or((&sentence, ""));
            ui.label(RichText::new(before).color(P.muted));
            if widgets::link(ui, &format!("{} — {}", r.title, r.artist_display), font(14.0, medium()), P.text) {
                shell.act(Action::Go(Page::Track(r.track_id)));
            }
            ui.label(RichText::new(after).color(P.muted));
        });
    }
}

/// The file: its format and quality, whether it's an original file (and its name), the
/// name it was uploaded as, where it came from, other tracks playing it, earlier files.
fn file(ui: &mut egui::Ui, shell: &mut Shell, d: &TrackDetail) {
    let f = &d.file;
    if let Some(name) = &f.original_filename {
        egui::Frame::NONE.fill(P.accent_soft).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.label(RichText::new(format!("Original file · {name}")).font(font(13.5, semibold())).color(P.accent));
        });
        ui.add_space(6.0);
    }
    let mut quality = Vec::new();
    if let Some(codec) = &f.codec {
        quality.push(codec.to_uppercase());
    }
    if let Some(rate) = f.sample_rate {
        quality.push(format!("{} kHz", rate as f32 / 1000.0));
    }
    if let Some(depth) = f.bit_depth {
        quality.push(format!("{depth}-bit"));
    }
    if let Some(bitrate) = f.bitrate {
        quality.push(format!("{} kbps", bitrate / 1000));
    }
    match f.channels {
        Some(1) => quality.push("mono".into()),
        Some(2) | None => {}
        Some(n) => quality.push(format!("{n} channels")),
    }
    quality.push(format!("{:.1} MB", f.size_bytes as f64 / 1_000_000.0));
    egui::Grid::new("file").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Format").color(P.faint));
        ui.label(quality.join("  ·  "));
        ui.end_row();
        ui.label(RichText::new("Uploaded as").color(P.faint));
        ui.label(format!("{} · {}", f.uploaded_filename, day(&f.uploaded_at)));
        ui.end_row();
        if let Some(origin) = &f.origin {
            ui.label(RichText::new("From").color(P.faint));
            ui.vertical(|ui| {
                let id = origin.external_id.as_deref().map(|i| format!(" · {i}")).unwrap_or_default();
                ui.label(format!("{}{id}", origin.platform));
                links_in(ui, &origin.data);
            });
            ui.end_row();
        }
        if !d.file_shared_with.is_empty() {
            ui.label(RichText::new("Also played by").color(P.faint));
            ui.vertical(|ui| {
                for s in &d.file_shared_with {
                    if widgets::link(ui, &format!("{} — {}", s.title, s.artist_display), font(14.0, medium()), P.text) {
                        shell.act(Action::Go(Page::Track(s.id)));
                    }
                }
            });
            ui.end_row();
        }
        ui.label(RichText::new("SHA-256").color(P.faint));
        ui.add(egui::Label::new(RichText::new(&f.sha256).monospace().color(P.faint).small()).selectable(true));
        ui.end_row();
    });
    if let Some(history) = d.file_history.as_ref().filter(|h| !h.is_empty()) {
        ui.add_space(8.0);
        ui.label(RichText::new(format!("Earlier files ({})", history.len())).font(font(14.0, semibold())));
        for h in history {
            let name = h.old_original_filename.clone().map(|n| format!("original file “{n}”")).unwrap_or_else(|| h.old_uploaded_filename.clone());
            let state = if h.old_purged_at.is_some() {
                " · purged"
            } else if h.old_trashed_at.is_some() {
                " · in the trash"
            } else {
                ""
            };
            let codec = h.old_codec.clone().unwrap_or_default().to_uppercase();
            let bitrate = h.old_bitrate.map(|b| format!(" {} kbps", b / 1000)).unwrap_or_default();
            ui.label(format!(
                "{name} · {codec}{bitrate} · {:.1} MB · replaced {}{state}",
                h.old_size_bytes as f64 / 1_000_000.0,
                day(&h.replaced_at)
            ));
            if !h.reason.is_empty() {
                ui.label(RichText::new(&h.reason).color(P.faint).small());
            }
        }
    }
}

/// Sources: the platform and id, and the newest snapshot's data (links clickable).
pub fn sources(ui: &mut egui::Ui, sources: &[Source]) {
    for s in sources {
        let id = s.external_id.as_deref().map(|i| format!(" · {i}")).unwrap_or_default();
        ui.label(RichText::new(format!("{}{id}", s.platform)).font(font(14.0, semibold())));
        if let Some(snapshot) = s.snapshots.first() {
            ui.label(RichText::new(format!("as of {}", day(&snapshot.captured_at))).color(P.faint).small());
            json_grid(ui, &format!("source-{}-{id}", s.platform), &snapshot.data);
        }
        ui.add_space(6.0);
    }
}

/// The links in some JSON data (a source's `url`, say), clickable.
fn links_in(ui: &mut egui::Ui, data: &serde_json::Value) {
    if let Some(object) = data.as_object() {
        for value in object.values() {
            if let Some(url) = value.as_str().filter(|v| v.starts_with("http")) {
                ui.hyperlink(url);
            }
        }
    }
}

/// A JSON object as a key–value grid: lists joined, links clickable, long values
/// wrapped, nested objects (like a tagged copy's own tags) as grids of their own.
fn json_grid(ui: &mut egui::Ui, id: &str, data: &serde_json::Value) {
    let Some(object) = data.as_object() else {
        ui.add(egui::Label::new(data.to_string()).wrap());
        return;
    };
    // The values' column gets what's left, so long ones wrap instead of running off.
    let values_width = (ui.available_width() - 190.0).max(120.0);
    egui::Grid::new(id).num_columns(2).spacing([24.0, 4.0]).max_col_width(values_width).show(ui, |ui| {
        for (key, value) in object {
            ui.label(RichText::new(key).color(P.faint));
            match value {
                serde_json::Value::String(text) if text.starts_with("http") => {
                    ui.hyperlink(text);
                }
                serde_json::Value::String(text) => {
                    ui.add(egui::Label::new(text).wrap());
                }
                serde_json::Value::Array(items) if items.iter().all(|v| !v.is_object()) => {
                    let joined: Vec<String> = items.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect();
                    ui.add(egui::Label::new(joined.join(" · ")).wrap());
                }
                serde_json::Value::Object(_) => {
                    ui.vertical(|ui| {
                        ui.set_max_width(values_width);
                        json_grid(ui, &format!("{id}/{key}"), value);
                    });
                }
                other => {
                    ui.add(egui::Label::new(other.to_string()).wrap());
                }
            }
            ui.end_row();
        }
    });
}

/// The day of a timestamp ("2026-09-27T12:50:48Z" → "2026-09-27").
fn day(timestamp: &str) -> &str {
    timestamp.get(..10).unwrap_or(timestamp)
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
