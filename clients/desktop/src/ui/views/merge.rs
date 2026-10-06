//! Merging two tracks that are the same song (a "possible duplicate" flag, from the
//! Review page): side by side, left is kept and right is merged into it. One pick per
//! row (title, artists, date, genres, comment, status, file, covers) says whose it is;
//! rows where both are the same say so. The file row shows format and quality, with a
//! play button per side, so the wrong file isn't kept by accident. Albums, relations,
//! sources, lyrics and the right track's versions are combined without asking; the
//! view lists them. Nothing happens until Ctrl+Enter (`POST /api/tracks/{left}/merge`,
//! with both tracks' revisions as the base: a change meanwhile is refused, not undone).
//! Undoing is on the kept track's page.
//!
//! Two tracks each alone on a single (of kind single, nothing else on it): the singles are
//! merged along, the right one's into the left one's, unless the "along" box is off (it's
//! a setting, shared with album merges). The server checks again, counting tracks the
//! user can't see.
//!
//! Keys: ↑/↓ (or j/k) a row (passing over rows where both are the same), ←/→ (or h/l)
//! its side, s picks everything from the right, and again from the left, Space plays the left file and Shift+Space the right (from
//! the middle; switching goes on from the same moment, to hear the difference), a merges
//! the singles along or not, Ctrl+Enter merges, n says they're not the same, Esc closes
//! it.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Key, Modifiers, RichText, Sense, Vec2};
use uuid::Uuid;

use crate::api::ApiError;
use crate::catalog::Catalog;
use crate::details::TrackDetail;
use crate::ui::theme::{bold, font, semibold, PALETTE as P};
use crate::playback::PlayerView;
use crate::ui::{playing_track, widgets, Action, Shell};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// The rows, as the server names their picks.
const FIELDS: [(&str, &str); 8] = [
    ("title", "Title"),
    ("credits", "Artists"),
    ("release_date", "Released"),
    ("genres", "Genres"),
    ("comment", "Comment"),
    ("status", "Status"),
    ("file", "File"),
    ("covers", "Main cover"),
];
const FILE: usize = 6;
const COVERS: usize = 7;

/// What happened, for the page that shows the view.
pub enum Outcome {
    /// Merged: `kept` is the track that stays.
    Merged { kept: Uuid, gone: Uuid },
    /// "Not the same": the pair's flag can be settled.
    NotSame,
    /// Both stay, playing the same file (the server settles their flags about each
    /// other).
    Shared,
    Closed,
}

pub struct MergeView {
    pub left: Uuid,
    pub right: Uuid,
    picks: [Side; 8],
    row: usize,
    reason: String,
    merging: bool,
    error: Option<String>,
    answer: Arc<Mutex<Option<Result<(), ApiError>>>>,
    /// A "share a file instead" on its way, and its answer.
    sharing: bool,
    share_answer: Arc<Mutex<Option<Result<(), ApiError>>>>,
    /// The file pick was set from the files' quality (once both details are there).
    file_suggested: bool,
    /// Both files are being downloaded ahead (for switching between them at once); let
    /// go when the view goes.
    kept_by: Option<(Arc<crate::playback::Controller>, tokio::runtime::Handle)>,
    /// Shown in a popup, which has its own title (so the view's is left out).
    in_popup: bool,
    /// Who may see each (left, right), for `manage_access`: asked once, when either is
    /// restricted.
    access: Arc<Mutex<Option<(Access, Access)>>>,
    access_asked: bool,
}

/// A track's access (`GET /api/tracks/{id}/access`), as far as the merge view needs it.
#[derive(Clone, serde::Deserialize)]
struct Access {
    restricted: bool,
    groups: Vec<GroupRef>,
}

#[derive(Clone, serde::Deserialize)]
struct GroupRef {
    id: Uuid,
    name: String,
}

impl Access {
    /// Who may see it, in words.
    fn audience(&self) -> String {
        match (self.restricted, self.groups.is_empty()) {
            (false, _) => "everyone".to_string(),
            (true, true) => "only admins".to_string(),
            (true, false) => self.groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", "),
        }
    }

    fn same_as(&self, other: &Access) -> bool {
        let ids = |a: &Access| a.groups.iter().map(|g| g.id).collect::<std::collections::BTreeSet<_>>();
        self.restricted == other.restricted && (!self.restricted || ids(self) == ids(other))
    }
}

impl Drop for MergeView {
    fn drop(&mut self) {
        if let Some((controller, runtime)) = self.kept_by.take() {
            runtime.spawn(async move { controller.prepare(Vec::new()).await });
        }
    }
}

impl MergeView {
    /// `left` is kept, `right` merged into it.
    pub fn new(left: Uuid, right: Uuid) -> MergeView {
        MergeView {
            left,
            right,
            picks: [Side::Left; 8],
            row: 0,
            reason: String::new(),
            merging: false,
            error: None,
            answer: Arc::default(),
            sharing: false,
            share_answer: Arc::default(),
            file_suggested: false,
            kept_by: None,
            in_popup: false,
            access: Arc::default(),
            access_asked: false,
        }
    }

    /// For showing in a popup (`ui::modal`): the popup gives the title.
    pub fn in_popup(mut self) -> MergeView {
        self.in_popup = true;
        self
    }

    /// Picks everything from the right, or from the left if that's already so. The sides
    /// stay where they are; only the picks change.
    fn flip_all(&mut self) {
        let to = if self.picks.iter().all(|p| *p == Side::Right) { Side::Left } else { Side::Right };
        self.picks = [to; 8];
    }

    /// The view; returns what happened when it's over.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, catalog: &Catalog, player: &PlayerView) -> Option<Outcome> {
        if let Some(result) = self.answer.lock().unwrap().take() {
            self.merging = false;
            match result {
                Ok(()) => return Some(Outcome::Merged { kept: self.left, gone: self.right }),
                Err(err) => self.error = Some(err.message),
            }
        }
        if let Some(result) = self.share_answer.lock().unwrap().take() {
            self.sharing = false;
            match result {
                Ok(()) => return Some(Outcome::Shared),
                Err(err) => self.error = Some(err.message),
            }
        }
        let left = shell.track_details.get(self.left).value.clone();
        let right = shell.track_details.get(self.right).value.clone();
        let (Some(l), Some(r)) = (left, right) else {
            ui.label(RichText::new("Loading both tracks…").color(P.faint));
            return None;
        };
        let lt = catalog.track_index(self.left).map(|i| &catalog.tracks[i as usize]);
        let rt = catalog.track_index(self.right).map(|i| &catalog.tracks[i as usize]);
        if self.kept_by.is_none() {
            if let (Some(a), Some(b)) = (catalog.track_index(self.left), catalog.track_index(self.right)) {
                shell.act(Action::Warm { a, b, at: middle(catalog, self.left, self.right) });
            }
            self.kept_by = Some((shell.controller.clone(), shell.runtime.handle().clone()));
        }
        let singles = lone_singles(catalog, self.left, self.right);
        // Rows where both are the same: nothing to pick, so ↑/↓ pass over them.
        let same: Vec<bool> = FIELDS.iter().map(|(field, _)| same(&l, &r, catalog, field)).collect();
        if !self.file_suggested {
            self.file_suggested = true;
            if quality(&r) > quality(&l) {
                self.picks[FILE] = Side::Right;
            }
            // Starting on the first row with something to pick.
            self.row = same.iter().position(|s| !s).unwrap_or(0);
        }

        // ---- Keys ----
        if !shell.palette_open {
            let ctx = ui.ctx().clone();
            let merge = ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            if merge {
                self.merge(shell, &l, &r, singles.is_some());
            }
            if !ctx.egui_wants_keyboard_input() {
                let key = |k: Key| ctx.input(|i| i.key_pressed(k) && i.modifiers.is_none());
                let shift_space = ctx.input(|i| i.key_pressed(Key::Space) && i.modifiers.shift);
                let mut acted = true;
                if key(Key::ArrowDown) || key(Key::J) {
                    self.row = (self.row + 1..FIELDS.len()).find(|&n| !same[n]).unwrap_or(self.row);
                } else if key(Key::ArrowUp) || key(Key::K) {
                    self.row = (0..self.row).rev().find(|&n| !same[n]).unwrap_or(self.row);
                } else if key(Key::ArrowLeft) || key(Key::H) {
                    self.picks[self.row] = Side::Left;
                } else if key(Key::ArrowRight) || key(Key::L) {
                    self.picks[self.row] = Side::Right;
                } else if key(Key::S) {
                    self.flip_all();
                } else if key(Key::A) && singles.is_some() {
                    shell.settings.merge_along = !shell.settings.merge_along;
                    shell.settings.save(&shell.data_dir);
                } else if key(Key::N) {
                    return Some(Outcome::NotSame);
                } else if key(Key::Escape) {
                    return Some(Outcome::Closed);
                } else if shift_space {
                    play(shell, catalog, player, self.right, self.left);
                } else if key(Key::Space) {
                    play(shell, catalog, player, self.left, self.right);
                } else {
                    acted = false;
                }
                if acted {
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                }
            }
        }

        // ---- The two sides ----
        ui.horizontal(|ui| {
            if !self.in_popup {
                ui.label(RichText::new("Merge").font(font(20.0, bold())));
            }
            ui.label(RichText::new("left is kept; right is merged into it").color(P.muted));
            if ui.small_button("Pick all from the other side  (s)").clicked() {
                self.flip_all();
            }
        });
        // Who may see them: with `manage_access`, which groups (asked once); else only
        // whether each is restricted.
        let any_restricted = lt.is_some_and(|t| t.is_restricted) || rt.is_some_and(|t| t.is_restricted);
        if any_restricted && shell.account.can("manage_access") && !self.access_asked {
            self.access_asked = true;
            let (api, access, ctx, left, right) = (shell.api.clone(), self.access.clone(), shell.ctx.clone(), self.left, self.right);
            shell.runtime.spawn(async move {
                let (l_path, r_path) = (format!("/api/tracks/{left}/access"), format!("/api/tracks/{right}/access"));
                let l = api.send_json::<Access>(reqwest::Method::GET, &l_path, None).await;
                let r = api.send_json::<Access>(reqwest::Method::GET, &r_path, None).await;
                if let (Ok(l), Ok(r)) = (l, r) {
                    *access.lock().unwrap() = Some((l, r));
                    ctx.request_repaint();
                }
            });
        }
        let access = self.access.lock().unwrap().clone();
        match access {
            Some((l, r)) if !l.same_as(&r) => {
                let text = format!(
                    "Who may see them differs (kept: {}; merged: {}). The kept track keeps its own access.",
                    l.audience(),
                    r.audience()
                );
                ui.label(RichText::new(text).color(P.danger));
            }
            Some(_) => {}
            None => {
                if lt.map(|t| t.is_restricted) != rt.map(|t| t.is_restricted) {
                    ui.label(RichText::new("Who may see them differs: the kept track keeps its own access.").color(P.danger));
                }
            }
        }
        ui.add_space(6.0);
        let values = |d: &TrackDetail, field: &str| -> String { value(d, catalog, field) };
        let width = ((ui.available_width() - 140.0) / 2.0).max(160.0);
        egui::Grid::new(("merge-grid", self.left, self.right)).num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("Left · kept").font(font(13.0, semibold())).color(P.accent));
            ui.label(RichText::new("Right · merged into it").font(font(13.0, semibold())).color(P.muted));
            ui.end_row();
            for (n, (field, name)) in FIELDS.iter().enumerate() {
                let (lv, rv) = (values(&l, field), values(&r, field));
                let same = same[n];
                let current = n == self.row;
                let label = if same { format!("{name}  (same)") } else { name.to_string() };
                ui.label(RichText::new(label).color(if current { P.text } else { P.faint }));
                for (side, text, detail) in [(Side::Left, &lv, &l), (Side::Right, &rv, &r)] {
                    let chosen = self.picks[n] == side;
                    let frame = egui::Frame::NONE
                        .fill(if chosen && !same { P.accent_soft } else { P.surface })
                        .stroke(egui::Stroke::new(1.0, if current && chosen { P.accent } else { P.line }))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::same(8));
                    let response = frame
                        .show(ui, |ui| {
                            ui.set_width(width);
                            if n == COVERS {
                                let cover = detail.covers.first().and_then(|c| c.cover());
                                let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
                                shell.cover(ui, cover, detail.id, rect, 4, 0.0);
                            }
                            ui.add(egui::Label::new(RichText::new(if text.is_empty() { "—" } else { text }).color(P.text)).wrap());
                            if n == FILE {
                                let (id, other) = if side == Side::Left { (self.left, self.right) } else { (self.right, self.left) };
                                if ui.small_button(if side == Side::Left { "▶ Play  (Space)" } else { "▶ Play  (Shift+Space)" }).clicked() {
                                    play(shell, catalog, player, id, other);
                                }
                            }
                        })
                        .response
                        .interact(Sense::click());
                    if response.clicked() {
                        self.picks[n] = side;
                        self.row = n;
                    }
                }
                ui.end_row();
            }
        });

        // ---- What's combined without asking ----
        ui.add_space(8.0);
        let both = |f: fn(&TrackDetail) -> usize| f(&l) + f(&r);
        let combined = [
            (both(|d| d.albums.len()), "album places"),
            (both(|d| d.relations.len()), "relations"),
            (both(|d| d.sources.len()), "sources"),
            (both(|d| d.lyrics.len()), "lyrics"),
            (r.variants.len(), "versions of the right track (they become versions of the kept one)"),
            (both(|d| d.covers.len()), "covers (all kept; the pick says whose come first)"),
        ];
        let listed: Vec<String> = combined.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect();
        if !listed.is_empty() {
            ui.label(RichText::new(format!("Combined: {}.", listed.join(", "))).color(P.muted));
        }
        if let Some((from, into)) = singles {
            let title = |id: Uuid| catalog.album(id).map(|a| a.title.clone()).unwrap_or_default();
            let text = format!("Merge their singles too: “{}” into “{}” (each has only this track)  (a)", title(from), title(into));
            if ui.checkbox(&mut shell.settings.merge_along, text).changed() {
                shell.settings.save(&shell.data_dir);
            }
        }
        ui.label(RichText::new("The right file stays as a previous file of the kept track. Undo is on the kept track's page (the singles' merge, if any, first: on the single's page).").color(P.faint).small());

        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut self.reason).hint_text("Why (optional)").desired_width(420.0));
        let mut outcome = None;
        ui.horizontal(|ui| {
            let can = shell.account.can("replace_file");
            if ui.add_enabled(can && !self.merging, egui::Button::new("Merge  (Ctrl+Enter)")).clicked() {
                self.merge(shell, &l, &r, singles.is_some());
            }
            if ui.button("Not the same  (n)").clicked() {
                outcome = Some(Outcome::NotSame);
            }
            if ui.button("Close  (Esc)").clicked() {
                outcome = Some(Outcome::Closed);
            }
            if !can {
                ui.label(RichText::new("Merging changes which file a track plays: it needs the replace_file permission.").color(P.muted));
            } else if self.merging {
                ui.label(RichText::new("Merging…").color(P.muted));
            }
        });
        // Both stay (different versions, the same audio): one gives up its file and
        // plays the other's. Its own file stays as a previous file of it.
        if can_share(shell) {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Or keep both, sharing one file:").color(P.muted));
                let busy = self.sharing || self.merging;
                if ui.add_enabled(!busy, egui::Button::new("Both play the left file").small()).clicked() {
                    self.share(shell, self.right, self.left);
                }
                if ui.add_enabled(!busy, egui::Button::new("Both play the right file").small()).clicked() {
                    self.share(shell, self.left, self.right);
                }
                if self.sharing {
                    ui.label(RichText::new("…").color(P.muted));
                }
            });
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(P.danger));
        }
        ui.label(RichText::new("↑/↓ row · ←/→ side · s all right / all left · Space / Shift+Space play left / right · a singles along · n not the same · Esc close").color(P.faint).small());
        outcome
    }

    /// The request: the picks, and both tracks' latest revisions as the base.
    /// `merge_singles`: their singles too (see `lone_singles`).
    fn body(&self, l: &TrackDetail, r: &TrackDetail, merge_singles: bool) -> serde_json::Value {
        let base = |d: &TrackDetail| d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)).map(|v| v.revision_id);
        let mut pick = serde_json::Map::new();
        for (n, (field, _)) in FIELDS.iter().enumerate() {
            pick.insert(field.to_string(), serde_json::json!(self.picks[n].name()));
        }
        serde_json::json!({
            "from": self.right,
            "base": { "into_revision_id": base(l), "from_revision_id": base(r) },
            "pick": pick,
            "reason": self.reason.trim(),
            "merge_singles": merge_singles,
        })
    }

    /// `track` plays `other`'s file from now on (its own becomes a previous file).
    fn share(&mut self, shell: &Shell, track: Uuid, other: Uuid) {
        self.sharing = true;
        self.error = None;
        let (api, answer, ctx) = (shell.api.clone(), self.share_answer.clone(), shell.ctx.clone());
        let body = serde_json::json!({ "file_of": other, "reason": self.reason.trim() });
        shell.runtime.spawn(async move {
            let result = api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/tracks/{track}/use-file"), Some(body)).await.map(|_| ());
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// `singles`: they're each alone on a single (merged along if the setting says so).
    fn merge(&mut self, shell: &Shell, l: &TrackDetail, r: &TrackDetail, singles: bool) {
        if self.merging || !shell.account.can("replace_file") {
            return;
        }
        let body = self.body(l, r, singles && shell.settings.merge_along);
        self.merging = true;
        self.error = None;
        let (api, answer, ctx, into) = (shell.api.clone(), self.answer.clone(), shell.ctx.clone(), self.left);
        shell.runtime.spawn(async move {
            let result = api.send_json::<serde_json::Value>(reqwest::Method::POST, &format!("/api/tracks/{into}/merge"), Some(body)).await.map(|_| ());
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
}

/// The singles the two tracks are each alone on (of kind single, no other track on it),
/// if both are on exactly one: (right's, left's). As far as the library kept here knows:
/// the server checks again, tracks the user can't see counted.
pub fn lone_singles(catalog: &Catalog, left: Uuid, right: Uuid) -> Option<(Uuid, Uuid)> {
    let alone_on = |track: Uuid| -> Option<Uuid> {
        let t = &catalog.tracks[catalog.track_index(track)? as usize];
        let mut singles = t.albums.iter().map(|m| m.0).filter(|&album| {
            catalog.album(album).is_some_and(|a| a.kind == "single") && catalog.album_tracks(album).len() == 1
        });
        match (singles.next(), singles.next()) {
            (Some(album), None) => Some(album),
            _ => None,
        }
    };
    let (from, into) = (alone_on(right)?, alone_on(left)?);
    (from != into).then_some((from, into))
}

fn can_share(shell: &Shell) -> bool {
    shell.account.can("replace_file")
}

/// Where comparing starts (when the other side isn't playing): the middle of the
/// shorter, the same for both sides (see `Action::Warm`).
fn middle(catalog: &Catalog, a: Uuid, b: Uuid) -> Duration {
    let length = |id: Uuid| catalog.track_index(id).and_then(|i| catalog.tracks[i as usize].duration_ms).map(|ms| Duration::from_millis(ms.max(0) as u64));
    let shorter = match (length(a), length(b)) {
        (Some(x), Some(y)) => x.min(y),
        (Some(x), None) | (None, Some(x)) => x,
        (None, None) => return Duration::ZERO,
    };
    // Not too near the end.
    (shorter / 2).min(shorter.saturating_sub(Duration::from_secs(1)))
}

/// Plays one side, for comparing: from where the other side is when that one's playing
/// (switching sides hears the same moment of both), else from the middle (where the
/// song is, rather than an intro). Playing again the side that plays pauses it.
fn play(shell: &mut Shell, catalog: &Catalog, player: &PlayerView, track: Uuid, other: Uuid) {
    let (Some(index), Some(other_index)) = (catalog.track_index(track), catalog.track_index(other)) else { return };
    let playing = playing_track(player);
    if playing == Some(track) {
        shell.act(Action::TogglePause);
        return;
    }
    let length = catalog.tracks[index as usize].duration_ms.map(|ms| Duration::from_millis(ms.max(0) as u64));
    let follow = playing == Some(other);
    let at = if follow { player.position_now() } else { middle(catalog, track, other) };
    // Not past the end (the other one may be longer).
    let at = length.map_or(at, |l| at.min(l.saturating_sub(Duration::from_secs(1))));
    shell.act(Action::Compare { track: index, other: other_index, at, follow });
}

/// Whether a row is the same on both sides: the same text, except covers, whose text is
/// only a count: the same images in the same order.
fn same(l: &TrackDetail, r: &TrackDetail, catalog: &Catalog, field: &str) -> bool {
    if field == "covers" {
        return l.covers.iter().map(|c| &c.blob_sha256).eq(r.covers.iter().map(|c| &c.blob_sha256));
    }
    value(l, catalog, field) == value(r, catalog, field)
}

/// A field as the row shows it.
fn value(d: &TrackDetail, catalog: &Catalog, field: &str) -> String {
    let track = catalog.track_index(d.id).map(|i| &catalog.tracks[i as usize]);
    let variant = d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)).map(|v| &v.data);
    let data = |key: &str| variant.and_then(|v| v.get(key)).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match field {
        "title" => d.title.clone(),
        "credits" => {
            let credits: Vec<String> = d
                .credits
                .iter()
                .map(|c| {
                    let name = c.alias.clone().unwrap_or_else(|| c.artist_name.clone());
                    if c.role == "primary" { name } else { format!("{name} ({})", c.role) }
                })
                .collect();
            credits.join(", ")
        }
        "release_date" => d.release_date.clone().unwrap_or_default(),
        "genres" => track.map(|t| t.genres.join(", ")).unwrap_or_default(),
        "comment" => data("comment"),
        "status" => d.status.clone(),
        "file" => {
            let f = &d.file;
            let mut parts = vec![f.codec.clone().unwrap_or_else(|| "?".into())];
            if let Some(b) = f.bitrate {
                parts.push(format!("{b} kbps"));
            }
            if let Some(s) = f.sample_rate {
                parts.push(format!("{:.1} kHz", s as f32 / 1000.0));
            }
            if let Some(b) = f.bit_depth {
                parts.push(format!("{b}-bit"));
            }
            parts.push(widgets::length(track.and_then(|t| t.duration_ms)));
            parts.push(format!("{:.1} MB", f.size_bytes as f64 / 1_000_000.0));
            format!("{}\n{}", parts.join(" · "), f.original_filename.clone().unwrap_or_else(|| f.uploaded_filename.clone()))
        }
        "covers" => match d.covers.len() {
            0 => "no cover of its own".into(),
            1 => "1 cover".into(),
            n => format!("{n} covers"),
        },
        _ => String::new(),
    }
}

/// How good a file is, to suggest which to keep (shown, and changeable): its format first,
/// uncompressed (WAV, AIFF) over lossless (FLAC, ALAC…) over lossy (MP3, AAC…) over
/// unknown; then the bigger file. As the server's bulk merge picks (`format_rank` there).
fn quality(d: &TrackDetail) -> (u8, i64) {
    (format_rank(d.file.codec.as_deref()), d.file.size_bytes)
}

fn format_rank(codec: Option<&str>) -> u8 {
    match codec.map(str::to_lowercase) {
        None => 0,
        Some(c) if c.starts_with("pcm") || c == "wav" || c == "aiff" => 3,
        Some(c) if matches!(c.as_str(), "flac" | "alac" | "ape" | "wavpack") => 2,
        Some(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::format_rank;

    #[test]
    fn wav_over_flac_over_mp3() {
        let order = [None, Some("mp3"), Some("aac"), Some("flac"), Some("pcm_s16le"), Some("pcm_s24le")].map(format_rank);
        assert_eq!(order, [0, 1, 1, 2, 3, 3]);
    }
}

/// Against a running server (ignored by default): `ACETATE_TEST_SERVER`,
/// `ACETATE_TEST_TOKEN`, and a flagged pair: `ACETATE_TEST_DUPLICATE` (the flagged track)
/// and `ACETATE_TEST_ORIGINAL` (the one it duplicates). Merges them: a throwaway database.
#[cfg(test)]
mod against_a_server {
    use super::*;
    use crate::api::Api;

    #[test]
    #[ignore]
    fn merging_a_flagged_pair() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_DUPLICATE", "ACETATE_TEST_ORIGINAL"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(duplicate), Ok(original)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let detail = |id: String| {
                let api = &api;
                async move { api.send_json::<TrackDetail>(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await.unwrap() }
            };
            let (l, r) = (detail(original.clone()).await, detail(duplicate.clone()).await);
            let mut view = MergeView::new(l.id, r.id);
            // The suggestion: the better file (the original FLAC over its MP3 copy) is kept.
            assert!(quality(&l) > quality(&r));
            view.file_suggested = true;
            view.reason = "merge test".into();
            let merged: serde_json::Value =
                api.send_json(reqwest::Method::POST, &format!("/api/tracks/{}/merge", l.id), Some(view.body(&l, &r, false))).await.unwrap();
            assert_eq!(merged["track_id"].as_str().unwrap(), l.id.to_string());

            // The duplicate now answers as the kept track, with the FLAC as its file.
            let after = detail(duplicate.clone()).await;
            assert_eq!(after.id, l.id);
            assert_eq!(after.file.codec.as_deref(), Some("flac"));
            // And its flag is settled.
            let open: Vec<crate::review::Flag> = api.send_json(reqwest::Method::GET, "/api/review", None).await.unwrap();
            assert!(!open.iter().any(|f| f.track_id == r.id), "{open:?}");
        });
    }

    /// Undoes the newest merge into `ACETATE_TEST_ORIGINAL`, as the track page's button
    /// does, and checks the merged track is back as itself.
    #[test]
    #[ignore]
    fn undoing_a_merge() {
        let vars = ["ACETATE_TEST_SERVER", "ACETATE_TEST_TOKEN", "ACETATE_TEST_ORIGINAL"].map(std::env::var);
        let [Ok(server), Ok(token), Ok(original)] = vars else { return };
        let dir = std::env::temp_dir().join(format!("acetate-undo-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let kept: TrackDetail = api.send_json(reqwest::Method::GET, &format!("/api/tracks/{original}"), None).await.unwrap();
            let merge = kept.merges.iter().find(|m| m.undone_at.is_none() && m.undo_blockers.is_empty()).expect("a merge to undo");
            let undone: serde_json::Value = api
                .send_json(reqwest::Method::POST, &format!("/api/track-merges/{}/undo", merge.id), Some(serde_json::json!({ "reason": "" })))
                .await
                .unwrap();
            assert_eq!(undone["restored_track_id"].as_str().unwrap(), merge.from_track_id.to_string());
            let back: TrackDetail = api.send_json(reqwest::Method::GET, &format!("/api/tracks/{}", merge.from_track_id), None).await.unwrap();
            assert_eq!(back.id, merge.from_track_id, "it answers as itself again");
            let kept: TrackDetail = api.send_json(reqwest::Method::GET, &format!("/api/tracks/{original}"), None).await.unwrap();
            assert!(kept.merges.iter().any(|m| m.id == merge.id && m.undone_at.is_some()));
        });
    }
}
