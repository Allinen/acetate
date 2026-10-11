//! Merging two albums that are the same (split by ingest because the album artists or the
//! date differ, say), from the album page in edit mode; and undoing a merge.
//!
//! Merging: the two side by side, the one on the left merged into the one on the right
//! (`s` swaps). What happens is said before anything does: the left one's tracks (each
//! with a revision, their disc and track numbers kept), covers, sources and share links
//! move to the right one, which keeps its own title, kind, date and credits; the left one
//! then only redirects. If the tracklists are the same (the same numbers, and the same title
//! at each: see `matching_tracks`), the tracks merge too, pair by pair, the right album's
//! track kept with the better file (unless the "along" box is off, `a`; a setting shared
//! with track merges). Otherwise tracks with the same title on both are pointed out: the
//! album will have both copies, to merge afterwards. Ctrl+Enter merges
//! (`POST /api/albums/{left}/merge {into: right, merge_tracks}`).
//!
//! Undoing: the server's preview first (`GET /api/album-merges/{id}/undo`, which changes
//! nothing), all of it: what goes back, what stays and why, what's in the way. Only then
//! the button (`POST` the same).

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use serde::Deserialize;
use uuid::Uuid;

use crate::catalog::Catalog;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::ui::{widgets, Action, Page, Shell};

/// What happened, for the album page.
pub enum Outcome {
    /// Merged: go to the album that stays.
    Merged { kept: Uuid, gone: Uuid },
    Closed,
}

pub struct AlbumMergeView {
    /// Merged away.
    pub from: Uuid,
    /// Stays.
    pub into: Uuid,
    reason: String,
    merging: bool,
    error: Option<String>,
    answer: Arc<Mutex<Option<Result<(), String>>>>,
}

impl AlbumMergeView {
    pub fn new(from: Uuid, into: Uuid) -> AlbumMergeView {
        AlbumMergeView { from, into, reason: String::new(), merging: false, error: None, answer: Arc::default() }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog) -> Option<Outcome> {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.merging = false;
            match result {
                Ok(()) => return Some(Outcome::Merged { kept: self.into, gone: self.from }),
                Err(err) => self.error = Some(err),
            }
        }
        let (Some(from), Some(into)) = (catalog.album(self.from), catalog.album(self.into)) else {
            ui.label(RichText::new("One of the two albums isn't in the library (anymore).").color(P.muted));
            return ui.button("Close").clicked().then_some(Outcome::Closed);
        };

        // The tracks, if they merge along (only with the permission to change their files).
        let pairs = if shell.account.can("replace_file") { matching_tracks(catalog, from.id(), into.id()) } else { Vec::new() };

        // ---- Keys ----
        if !shell.palette_open {
            let ctx = ui.ctx().clone();
            if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter)) {
                self.merge(shell, !pairs.is_empty());
            }
            if !ctx.egui_wants_keyboard_input() {
                let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
                if key(Key::S) {
                    std::mem::swap(&mut self.from, &mut self.into);
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                } else if key(Key::A) && !pairs.is_empty() {
                    shell.settings.merge_along = !shell.settings.merge_along;
                    shell.settings.save(&shell.data_dir);
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                } else if key(Key::Escape) {
                    return Some(Outcome::Closed);
                }
            }
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Merge albums").font(font(20.0, bold())));
            ui.label(RichText::new("the left one is merged into the right one, which stays").color(P.muted));
            if ui.small_button("Swap  (s)").clicked() {
                std::mem::swap(&mut self.from, &mut self.into);
            }
        });
        ui.add_space(8.0);
        let width = ((ui.available_width() - 16.0) / 2.0).max(220.0);
        ui.horizontal_top(|ui| {
            for (album, label, color) in [(from, "Merged away", P.muted), (into, "Stays", P.accent)] {
                egui::Frame::NONE.fill(P.surface).corner_radius(10).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(RichText::new(label).font(font(12.0, semibold())).color(color));
                    ui.horizontal(|ui| {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
                        shell.cover(ui, album.cover(), album.id(), rect, 6, 0.1);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(album.title()).font(font(16.0, semibold())));
                            ui.label(RichText::new(album.artist_display()).color(P.muted));
                            let mut facts = vec![album.kind().to_string()];
                            if let Some(date) = album.release_date() {
                                facts.push(date.to_string());
                            }
                            let n = catalog.album_tracks(album.id()).len();
                            facts.push(crate::ui::count(n, "track"));
                            ui.label(RichText::new(facts.join(" · ")).color(P.faint));
                        });
                    });
                });
            }
        });

        // ---- What happens ----
        ui.add_space(8.0);
        let moving = catalog.album_tracks(from.id()).len();
        ui.label(
            RichText::new(format!(
                "“{}”'s {} (each with a revision, their disc and track numbers kept), covers, sources and share links move to “{}”, which keeps its own title, kind, date and credits. “{}” then only leads to it. Undo is on “{}”'s page.",
                from.title(),
                if moving == 1 { "track".to_string() } else { format!("{moving} tracks") },
                into.title(),
                from.title(),
                into.title()
            ))
            .color(P.text),
        );
        if !pairs.is_empty() {
            let text = format!(
                "Merge the tracks too: the tracklists are the same (numbers and titles), so each of “{}”'s tracks merges into the one in its place on “{}”  (a)",
                from.title(), into.title()
            );
            if ui.checkbox(&mut shell.settings.merge_along, text).changed() {
                shell.settings.save(&shell.data_dir);
            }
            if shell.settings.merge_along {
                let number = |i: u32, album: Uuid| {
                    let m = catalog.track_at(i).albums().iter().find(|m| m.0 == album);
                    match m.map(|m| (m.1, m.2)) {
                        Some((Some(disc), Some(n))) if disc > 1 => format!("{disc}-{n}"),
                        Some((_, Some(n))) => n.to_string(),
                        _ => "1".into(),
                    }
                };
                for &(a, b) in &pairs {
                    let (ta, tb) = (catalog.track_at(a), catalog.track_at(b));
                    let codec = |t: crate::catalog::Track<'_>| t.codec().map(str::to_string).unwrap_or_else(|| "?".into());
                    ui.label(RichText::new(format!("  {}. {} — {} + {}", number(b, into.id()), tb.title(), codec(tb), codec(ta))).color(P.muted));
                }
                ui.label(
                    RichText::new(format!(
                        "Each kept track is “{}”'s, with its title, artists and the rest; the better file of the two is kept (lossless first, then sample rate, bit depth, bitrate, size), the other stays a previous file. Each is a track merge of its own, undone on the kept track's page (after undoing the album merge).",
                        into.title()
                    ))
                    .color(P.faint)
                    .small(),
                );
            }
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional)").desired_width(420.0));
            return self.buttons(ui, shell, true);
        }
        // Same titles on both: those will be possible duplicates, to merge afterwards.
        let staying: Vec<String> = catalog.album_tracks(into.id()).iter().map(|&i| catalog.track_at(i).title().to_lowercase()).collect();
        let doubled: Vec<&str> = catalog
            .album_tracks(from.id())
            .iter()
            .map(|&i| catalog.track_at(i).title())
            .filter(|t| staying.contains(&t.to_lowercase()))
            .collect();
        if !doubled.is_empty() {
            ui.label(
                RichText::new(format!(
                    "On both: {}. After the merge the album has both copies of each: merge those tracks afterwards (from a track's page in edit mode, or Ctrl+M on Review where they're flagged as possible duplicates).",
                    doubled.join(", ")
                ))
                .color(P.muted),
            );
        }

        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional)").desired_width(420.0));
        self.buttons(ui, shell, false)
    }

    /// Merge and Close, and how it went. `pairs`: the tracks can merge along.
    fn buttons(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pairs: bool) -> Option<Outcome> {
        let mut outcome = None;
        ui.horizontal(|ui| {
            if ui.add_enabled(!self.merging, egui::Button::new("Merge  (Ctrl+Enter)")).clicked() {
                self.merge(shell, pairs);
            }
            if ui.button("Close  (Esc)").clicked() {
                outcome = Some(Outcome::Closed);
            }
            if self.merging {
                ui.label(RichText::new("Merging…").color(P.muted));
            }
        });
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(P.danger));
        }
        outcome
    }

    /// `pairs`: the tracklists are the same (merged along if the setting says so).
    fn merge(&mut self, shell: &Shell, pairs: bool) {
        if self.merging {
            return;
        }
        self.merging = true;
        self.error = None;
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        let merge_tracks = pairs && shell.settings.merge_along;
        let (from, body) = (self.from, serde_json::json!({ "into": self.into, "reason": self.reason.trim(), "merge_tracks": merge_tracks }));
        shell.runtime.spawn(async move {
            let result = api
                .send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/albums/{from}/merge"), Some(body))
                .await
                .map(|_| ())
                .map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// The tracks of albums `from` and `into` in pairs (from's, into's: places in the
/// catalog), if the tracklists are the same, as the server decides it
/// (`db::album_merges::pair_tracklists`): the same places (disc and track number; no disc
/// is disc 1; the one track of a one-track album is track 1 without a number), one track
/// in each, the same title in each place (letter case and spaces around aside). Empty
/// otherwise. Tracks the user can't see aren't known here: the server checks again.
pub fn matching_tracks(catalog: &Catalog, from: Uuid, into: Uuid) -> Vec<(u32, u32)> {
    use std::collections::BTreeMap;
    let places = |album: Uuid| -> Option<BTreeMap<(i16, i16), u32>> {
        let tracks = catalog.album_tracks(album);
        let mut places = BTreeMap::new();
        for &i in tracks {
            let m = catalog.track_at(i).albums().iter().find(|m| m.0 == album)?;
            let number = match (m.2, tracks.len()) {
                (Some(n), _) => n,
                (None, 1) => 1,
                (None, _) => return None,
            };
            if places.insert((m.1.unwrap_or(1), number), i).is_some() {
                return None;
            }
        }
        Some(places)
    };
    let (Some(a), Some(b)) = (places(from), places(into)) else { return Vec::new() };
    if a.is_empty() || a.len() != b.len() {
        return Vec::new();
    }
    let title = |i: u32| catalog.track_at(i).title().trim().to_lowercase();
    let mut pairs = Vec::new();
    for (place, &ta) in &a {
        let Some(&tb) = b.get(place) else { return Vec::new() };
        if title(ta) != title(tb) {
            return Vec::new();
        }
        if ta != tb {
            pairs.push((ta, tb));
        }
    }
    // A track on both, in different places: not the same tracklist after all.
    let on_both = |i: u32| a.values().any(|&x| x == i) && b.values().any(|&x| x == i);
    if pairs.iter().any(|&(x, y)| on_both(x) || on_both(y)) {
        return Vec::new();
    }
    pairs
}

/// What undoing an album merge would do (or did), as the server says it.
#[derive(Debug, Clone, Deserialize)]
pub struct UndoReport {
    pub from_album_id: Uuid,
    pub from_title: String,
    pub into_title: String,
    /// Why it can't be undone (empty: it can).
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub tracks: Vec<TrackUndo>,
    #[serde(default)]
    pub hidden_tracks_back: usize,
    #[serde(default)]
    pub hidden_tracks_staying: usize,
    #[serde(default)]
    pub goes_back: Vec<String>,
    #[serde(default)]
    pub stays: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackUndo {
    pub title: String,
    pub goes_back: bool,
    pub detail: String,
}

/// Undoing merges into an album: "Undo…" fetches the preview, shows it whole, and only
/// then offers to do it.
#[derive(Default)]
pub struct UndoPanel {
    /// The merge being looked at, and its preview (None: loading).
    open: Option<(Uuid, Option<Result<UndoReport, String>>)>,
    doing: bool,
    answer: Arc<Mutex<Option<(Uuid, bool, Result<UndoReport, String>)>>>,
    /// A finished undo: the album that came back.
    done: Option<(Uuid, String)>,
}

impl UndoPanel {
    /// Out of edit mode: the preview (and a finished undo's notice) closed.
    pub fn close(&mut self) {
        self.open = None;
        self.done = None;
    }

    /// The merges into album `album`, each with Undo… (`can`: allowed to).
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, album: Uuid, merges: &[crate::details::AlbumMerge], can: bool) {
        if let Some((merge, done, result)) = self.answer.lock().unwrap().take() {
            self.doing = false;
            match (done, result) {
                (true, Ok(report)) => {
                    self.open = None;
                    self.done = Some((report.from_album_id, report.from_title.clone()));
                    shell.album_details.refresh(album);
                    shell.act(Action::RefreshLibrary);
                }
                (_, result) => {
                    if self.open.as_ref().is_some_and(|(m, _)| *m == merge) {
                        self.open = Some((merge, Some(result)));
                    }
                }
            }
        }
        for m in merges {
            let title = m.from_title.clone().unwrap_or_else(|| "(an album you can't see)".into());
            let by = m.merged_by.as_deref().map(|b| format!(" by {b}")).unwrap_or_default();
            let why = Some(m.reason.trim()).filter(|r| !r.is_empty()).map(|r| format!(" ({r})")).unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{title} · {}{by}{why}", m.merged_at.get(..10).unwrap_or(&m.merged_at)));
                if m.undone_at.is_some() {
                    ui.label(RichText::new("· undone").color(P.faint));
                    if widgets::link(ui, "its page", font(13.0, egui::FontFamily::Proportional), P.muted) {
                        shell.act(Action::Go(Page::Album(m.from_album_id)));
                    }
                } else if can && self.open.as_ref().is_none_or(|(o, _)| *o != m.id) && ui.small_button("Undo…").clicked() {
                    self.open = Some((m.id, None));
                    self.done = None;
                    self.ask(shell, m.id, false);
                }
            });
            if let Some((open, preview)) = &self.open
                && *open == m.id
            {
                match preview {
                    None => {
                        ui.label(RichText::new("Asking the server what undoing it would do…").color(P.faint));
                    }
                    Some(Err(err)) => {
                        ui.label(RichText::new(format!("Couldn't get the preview: {err}")).color(P.danger));
                    }
                    Some(Ok(report)) => {
                        let report = report.clone();
                        if self.preview(ui, shell, m.id, &report) {
                            self.open = None;
                        }
                    }
                }
            }
        }
        if let Some((back, title)) = self.done.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Undone: “{title}” is an album of its own again.")).color(P.accent));
                if widgets::link(ui, "Go to it", font(13.0, egui::FontFamily::Proportional), P.text) {
                    shell.act(Action::Go(Page::Album(back)));
                }
            });
        }
    }

    /// The preview, all of it, and the button. True: cancelled.
    fn preview(&mut self, ui: &mut egui::Ui, shell: &Shell, merge: Uuid, r: &UndoReport) -> bool {
        let mut cancelled = false;
        egui::Frame::NONE.fill(P.surface).corner_radius(8).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.label(RichText::new(format!("Undoing gives “{}” back as its own album, out of “{}”:", r.from_title, r.into_title)).color(P.text));
            if !r.blockers.is_empty() {
                ui.label(RichText::new("It can't be undone:").color(P.danger));
                for b in &r.blockers {
                    ui.label(RichText::new(format!("• {b}")).color(P.danger));
                }
            }
            for t in &r.tracks {
                let (mark, color) = if t.goes_back { ("goes back", P.text) } else { ("stays", P.muted) };
                ui.label(RichText::new(format!("• {} — {mark}: {}", t.title, t.detail)).color(color));
            }
            if r.hidden_tracks_back + r.hidden_tracks_staying > 0 {
                ui.label(RichText::new(format!("Tracks you can't see: {} go back, {} stay.", r.hidden_tracks_back, r.hidden_tracks_staying)).color(P.muted));
            }
            for g in &r.goes_back {
                ui.label(RichText::new(format!("Goes back: {g}")).color(P.text));
            }
            for s in &r.stays {
                ui.label(RichText::new(format!("Stays: {s}")).color(P.muted));
            }
            ui.horizontal(|ui| {
                if r.blockers.is_empty() {
                    if self.doing {
                        ui.label(RichText::new("Undoing…").color(P.muted));
                    } else if ui.button("Undo the merge").clicked() {
                        self.doing = true;
                        self.ask(shell, merge, true);
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancelled = true;
                }
            });
        });
        cancelled
    }

    /// The preview (`doit` false) or the undo itself.
    fn ask(&self, shell: &Shell, merge: Uuid, doit: bool) {
        let (api, answer, ctx) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone());
        shell.runtime.spawn(async move {
            let path = format!("/api/album-merges/{merge}/undo");
            let result = if doit {
                api.send_json::<UndoReport>(reqwest::Method::POST, &path, Some(serde_json::json!({ "reason": "" }))).await
            } else {
                api.send_json::<UndoReport>(reqwest::Method::GET, &path, None).await
            };
            *answer.lock().unwrap() = Some((merge, doit, result.map_err(|err| err.message)));
            ctx.request_repaint();
        });
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, `ACETATE_TEST_ALBUM_FROM` and `ACETATE_TEST_ALBUM_INTO`. Merges the
/// first into the second, reads the undo preview, and undoes it: a throwaway database.
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;

    #[test]
    #[ignore]
    fn merging_and_undoing_albums() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_ALBUM_FROM", "ACETATE_TEST_ALBUM_INTO"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(from), Ok(into)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-album-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let body = serde_json::json!({ "into": into, "reason": "album merge test" });
            let merged: serde_json::Value = api.send_json(reqwest::Method::POST, &format!("/api/albums/{from}/merge"), Some(body)).await.unwrap();
            let merge = merged["merge_id"].as_str().unwrap().to_string();
            // The kept album lists it, as the page reads it.
            let kept: crate::details::AlbumDetail = api.send_json(reqwest::Method::GET, &format!("/api/albums/{into}"), None).await.unwrap();
            let entry = kept.merges.iter().find(|m| m.id.to_string() == merge).expect("listed on the kept album");
            assert_eq!(entry.from_album_id.to_string(), from);
            // The preview changes nothing, and reads.
            let path = format!("/api/album-merges/{merge}/undo");
            let preview: UndoReport = api.send_json(reqwest::Method::GET, &path, None).await.unwrap();
            assert!(preview.blockers.is_empty(), "{:?}", preview.blockers);
            let preview_again: UndoReport = api.send_json(reqwest::Method::GET, &path, None).await.unwrap();
            assert_eq!(preview.goes_back, preview_again.goes_back, "a preview changes nothing");
            // Undone: the album is back as itself.
            let done: UndoReport = api.send_json(reqwest::Method::POST, &path, Some(serde_json::json!({ "reason": "" }))).await.unwrap();
            assert_eq!(done.from_album_id.to_string(), from);
            let back: serde_json::Value = api.send_json(reqwest::Method::GET, &format!("/api/albums/{from}"), None).await.unwrap();
            assert_eq!(back["id"].as_str().unwrap(), from, "answers as itself again");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::model;

    /// Albums A and B with the same tracklist (B's titles in other letter case, B's only
    /// disc named), C with another title in place 2.
    fn catalog() -> Catalog {
        let (a, b, c) = ("00000000-0000-7000-8000-0000000000a1", "00000000-0000-7000-8000-0000000000a2", "00000000-0000-7000-8000-0000000000a3");
        let track = |n: u8, title: &str, album: &str, disc: Option<i16>, number: i16| {
            serde_json::json!([format!("00000000-0000-7000-8000-0000000001{n:02}"), title, "Ann", [], [[album, disc, number]], 1000,
                "00000000-0000-7000-8000-000000000f01", null, null, null, null, "released", [], "", false, null])
        };
        let album = |id: &str| serde_json::json!([id, "EP", "ep", "Ann", [], null, null, null, null, "", 2, 1]);
        let body = serde_json::json!({
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": [track(1, "One", a, None, 1), track(2, "Two", a, None, 2),
                                track(3, "two ", b, Some(1), 2), track(4, "ONE", b, Some(1), 1),
                                track(5, "One", c, None, 1), track(6, "Three", c, None, 2)]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [album(a), album(b), album(c)]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"], "rows": []},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(model::parse(body.to_string().as_bytes()).unwrap())
    }

    #[test]
    fn same_tracklists_pair_up_by_place() {
        let c = catalog();
        let id = |s: &str| s.parse::<Uuid>().unwrap();
        let title = |i: u32| c.track_at(i).title().to_string();
        let pairs = matching_tracks(&c, id("00000000-0000-7000-8000-0000000000a1"), id("00000000-0000-7000-8000-0000000000a2"));
        let named: Vec<(String, String)> = pairs.iter().map(|&(x, y)| (title(x), title(y))).collect();
        assert_eq!(named, [("One".to_string(), "ONE".to_string()), ("Two".to_string(), "two ".to_string())]);
        assert!(matching_tracks(&c, id("00000000-0000-7000-8000-0000000000a1"), id("00000000-0000-7000-8000-0000000000a3")).is_empty());
    }
}
