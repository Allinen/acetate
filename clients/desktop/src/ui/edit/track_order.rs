//! An album's tracks moved in its tracklist (edit mode): Alt+↑/↓ (or Alt+K/J) moves the
//! selected track one place, on its disc. The moves are a draft, kept per album (looking
//! elsewhere, or leaving edit mode, doesn't lose them): the list shows them, the moved
//! tracks' numbers in the accent colour, and a bar above it (there only once something
//! moved, and gone again if it all moved back) says how many tracks it renumbers, with
//! Save (Ctrl+Enter, the album's form too if it has changes) and Discard (Esc, asks
//! first). Saving sends them all in one request (`PUT /api/albums/{id}/track-numbers`):
//! a revision for each track whose place changed, however often it moved.
//!
//! A move swaps the two tracks' places (disc and number), so nothing else changes. When
//! that can't order them (one has no number, or two on the disc share one), the disc's
//! tracks are numbered 1, 2, 3… as they're shown, with the move, and the notice says so.

use std::collections::HashMap;
use std::time::Instant;

use eframe::egui::{self, Key, RichText};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::catalog::index::album_order;
use crate::catalog::Track;
use crate::catalog::Catalog;
use crate::ui::task::Task;
use crate::ui::theme::PALETTE as P;
use crate::ui::{Action, Shell};

/// A track's place on an album: its disc and number.
pub type Place = (Option<i16>, Option<i16>);

/// The drafts, by album.
#[derive(Default)]
pub struct TrackOrder {
    drafts: HashMap<Uuid, Draft>,
}

#[derive(Default)]
struct Draft {
    /// Moved tracks' places as they're to be (only ones that differ from the library's).
    wanted: HashMap<Uuid, Place>,
    saving: Saving,
    answer: Task<Result<(), String>>,
    /// The last save failed (the draft is kept, to try again).
    error: Option<String>,
    /// Esc was pressed once: again discards it.
    confirm_discard: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Saving {
    #[default]
    No,
    Sending,
    /// Saved; shown until the library has it. The library's generation then.
    Sent(u64),
}

impl Draft {
    /// Moves not saved (nor being saved).
    fn unsaved(&self) -> bool {
        !self.wanted.is_empty() && self.saving == Saving::No
    }
}

impl TrackOrder {
    /// Each frame, before the page's forms (Ctrl+Enter is theirs too): takes the answers to
    /// saves (any album's), lets go of saved drafts once the library has them, and saves
    /// this album's on Ctrl+Enter.
    pub fn update(&mut self, shell: &mut Shell, catalog: &Catalog, album: Uuid) {
        let mut refresh = false;
        for (&id, draft) in &mut self.drafts {
            if let Some(result) = draft.answer.poll() {
                match result {
                    Ok(()) => {
                        draft.saving = Saving::Sent(shell.generation);
                        refresh = true;
                    }
                    Err(err) => {
                        draft.saving = Saving::No;
                        draft.error = Some(err);
                    }
                }
            }
            match draft.saving {
                Saving::Sent(at) => {
                    // (Twice: a refresh started before the answer can bring the library as it was.)
                    if caught_up(catalog, id, &draft.wanted) || shell.generation >= at + 2 {
                        draft.wanted.clear();
                    }
                }
                // Moves the library has now (someone else's, say) aren't changes anymore.
                Saving::No => draft.wanted.retain(|track, wanted| catalog.track_index(*track).is_some_and(|t| place(catalog.track_at(t), id) != *wanted)),
                Saving::Sending => {}
            }
        }
        self.drafts.retain(|_, d| !d.wanted.is_empty() || d.saving == Saving::Sending);
        if refresh {
            shell.act(Action::RefreshLibrary);
        }
        let save = shell.edit_mode
            && !shell.palette_open
            && !shell.modal_was_open
            && self.drafts.get(&album).is_some_and(Draft::unsaved)
            && shell.ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
        if save {
            self.save(shell, album);
        }
    }

    /// The album's tracks (positions in the catalog) in the order they're to be: None while
    /// that's the library's. (A draft not saved shows in edit mode only.)
    pub fn tracks(&self, shell: &Shell, catalog: &Catalog, album: Uuid, tracks: &[u32]) -> Option<Vec<u32>> {
        let draft = self.shown(shell, album)?;
        let mut list = tracks.to_vec();
        list.sort_by(|&a, &b| {
            let (a, b) = (catalog.track_at(a), catalog.track_at(b));
            album_order(draft_place(draft, a, album), a.title(), draft_place(draft, b, album), b.title())
        });
        Some(list)
    }

    /// Moved tracks' places as they're to be (for their numbers), if any are shown.
    pub fn places(&self, shell: &Shell, album: Uuid) -> Option<&HashMap<Uuid, Place>> {
        self.shown(shell, album).map(|d| &d.wanted)
    }

    fn shown(&self, shell: &Shell, album: Uuid) -> Option<&Draft> {
        self.drafts.get(&album).filter(|d| !d.wanted.is_empty() && (shell.edit_mode || d.saving != Saving::No))
    }

    /// Moves the track at `row` of `tracks` (the album's, as shown, in its own order) `by`
    /// one place up or down. Where it is then, if it moved.
    pub fn move_track(&mut self, shell: &mut Shell, catalog: &Catalog, album: Uuid, tracks: &[u32], row: usize, by: isize) -> Option<usize> {
        let to = row.checked_add_signed(by).filter(|&to| to < tracks.len())?;
        let draft = self.drafts.entry(album).or_default();
        if draft.saving != Saving::No {
            shell.playlists.notice = Some(("Saving the new order: a moment".into(), Instant::now()));
            return None;
        }
        let shown: Vec<(Uuid, Place)> = tracks.iter().map(|&t| catalog.track_at(t)).map(|t| (t.id(), draft_place(draft, t, album))).collect();
        let changes = match plan(&shown, row, to) {
            Plan::OtherDisc(disc) => {
                let end = if by < 0 { "first" } else { "last" };
                shell.playlists.notice = Some((format!("It's the {end} track on disc {disc}: its disc is changed in its editor (disc:…)"), Instant::now()));
                return None;
            }
            Plan::Swap(changes) => changes,
            Plan::Numbered { disc, count, changes } => {
                shell.playlists.notice = Some((format!("Numbered disc {disc}'s tracks 1–{count} as shown (not every track had a number of its own)"), Instant::now()));
                changes
            }
        };
        // Only what differs from the library: a track moved back is no change.
        for (track, new) in changes {
            let theirs = catalog.track_index(track).map(|t| place(catalog.track_at(t), album));
            if theirs == Some(new) {
                draft.wanted.remove(&track);
            } else {
                draft.wanted.insert(track, new);
            }
        }
        draft.error = None;
        draft.confirm_discard = false;
        let id = shown[row].0;
        let order = self.tracks(shell, catalog, album, tracks).unwrap_or_else(|| tracks.to_vec());
        order.iter().position(|&t| catalog.track_at(t).id() == id)
    }

    /// Above the list (edit mode), once tracks have moved: how many it renumbers, Save and
    /// Discard. Esc discards too (asks first), unless `esc_taken` (it clears marks, say).
    pub fn bar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, album: Uuid, esc_taken: bool) {
        if !shell.edit_mode {
            return;
        }
        let Some(draft) = self.drafts.get_mut(&album).filter(|d| !d.wanted.is_empty() && !matches!(d.saving, Saving::Sent(_))) else { return };
        let unsaved = draft.unsaved();
        // Esc (and Backspace) mean this, not going back.
        shell.form_dirty |= unsaved;
        let ctx = ui.ctx().clone();
        let mut discard = false;
        if unsaved && !esc_taken && !shell.palette_open && !shell.modal_was_open && !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Escape)) {
            discard = draft.confirm_discard;
            draft.confirm_discard = true;
        }
        let mut save = false;
        crate::ui::views::album::panel(ui, "album-track-order", |ui| {
            ui.horizontal(|ui| {
                let count = draft.wanted.len();
                let tracks = crate::ui::count(count, "track");
                ui.label(RichText::new(format!("New order: {tracks} renumbered")).color(P.text));
                ui.add_space(8.0);
                save = ui.add_enabled(unsaved, egui::Button::new("Save order  (Ctrl+Enter)")).clicked();
                discard |= ui.add_enabled(unsaved, egui::Button::new("Discard  (Esc)")).clicked();
                let (text, color) = match (&draft.saving, &draft.error) {
                    (Saving::Sending, _) => ("Saving…".to_string(), P.muted),
                    (_, Some(err)) => (format!("Couldn't save it: {err}"), P.danger),
                    _ if draft.confirm_discard => ("Discard the new order? Esc again to discard it.".to_string(), P.danger),
                    _ => ("Not saved yet".to_string(), P.muted),
                };
                ui.label(RichText::new(text).color(color));
            });
        });
        ui.add_space(10.0);
        if discard {
            self.drafts.remove(&album);
        } else if save {
            self.save(shell, album);
        }
    }

    /// Sends the album's draft: every moved track's place, in one request.
    fn save(&mut self, shell: &Shell, album: Uuid) {
        let Some(draft) = self.drafts.get_mut(&album).filter(|d| d.unsaved()) else { return };
        draft.saving = Saving::Sending;
        draft.error = None;
        draft.confirm_discard = false;
        let tracks: Vec<Value> = draft.wanted.iter().map(|(id, (disc, number))| json!({ "track_id": id, "disc_number": disc, "track_number": number })).collect();
        draft.answer.send_with(shell, |api| async move { api.send(reqwest::Method::PUT, &format!("/api/albums/{album}/track-numbers"), Some(json!({ "tracks": tracks }))).await.map_err(|err| err.message) });
    }
}

/// The library has every place the draft wanted (a track gone from it has nothing to wait for).
fn caught_up(catalog: &Catalog, album: Uuid, wanted: &HashMap<Uuid, Place>) -> bool {
    wanted.iter().all(|(id, wanted)| catalog.track_index(*id).is_none_or(|t| place(catalog.track_at(t), album) == *wanted))
}

/// The track's place on the album: as the draft has it if moved, else the library's.
fn draft_place(draft: &Draft, t: Track<'_>, album: Uuid) -> Place {
    draft.wanted.get(&t.id()).copied().unwrap_or_else(|| place(t, album))
}

/// The track's place on the album, as the library has it.
fn place(t: Track<'_>, album: Uuid) -> Place {
    t.albums().iter().find(|m| m.0 == album).map_or((None, None), |m| (m.1, m.2))
}

/// What moving a track does to the places of the tracks shown.
#[derive(Debug, PartialEq)]
enum Plan {
    /// The other track is on another disc: nothing.
    OtherDisc(i16),
    /// The two swap places.
    Swap(Vec<(Uuid, Place)>),
    /// The disc's tracks (`count` of them) numbered as shown, with the move: the ones whose
    /// place changes.
    Numbered { disc: i16, count: usize, changes: Vec<(Uuid, Place)> },
}

/// Moving the track at `from` of `shown` (the album's tracks and their places, in order)
/// to `to`, the one next to it.
fn plan(shown: &[(Uuid, Place)], from: usize, to: usize) -> Plan {
    let disc = |place: Place| place.0.unwrap_or(1);
    let ((a, pa), (b, pb)) = (shown[from], shown[to]);
    if disc(pa) != disc(pb) {
        return Plan::OtherDisc(disc(pa));
    }
    let on_disc: Vec<(Uuid, Place)> = shown.iter().copied().filter(|(_, p)| disc(*p) == disc(pa)).collect();
    let mut numbers: Vec<Option<i16>> = on_disc.iter().map(|(_, p)| p.1).collect();
    numbers.sort();
    numbers.dedup();
    if numbers.len() == on_disc.len() && numbers.iter().all(Option::is_some) {
        return Plan::Swap(vec![(a, pb), (b, pa)]);
    }
    // Numbered as shown, with the move. On the disc they're on: theirs, when they all have
    // the same (none, say), else the one it's shown as.
    let mut order = on_disc.clone();
    let (i, j) = (order.iter().position(|t| t.0 == a).unwrap_or(0), order.iter().position(|t| t.0 == b).unwrap_or(0));
    order.swap(i, j);
    let disc_number = if on_disc.iter().all(|(_, p)| p.0 == pa.0) { pa.0 } else { Some(disc(pa)) };
    let changes = order
        .iter()
        .enumerate()
        .map(|(n, (id, place))| (*id, *place, (disc_number, Some(n as i16 + 1))))
        .filter(|(_, place, new)| place != new)
        .map(|(id, _, new)| (id, new))
        .collect();
    Plan::Numbered { disc: disc(pa), count: order.len(), changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn numbered_tracks_swap_places() {
        // Numbers with a gap (a track that's hidden, say): kept.
        let shown = [(id(1), (None, Some(1))), (id(2), (None, Some(2))), (id(3), (None, Some(4)))];
        assert_eq!(plan(&shown, 2, 1), Plan::Swap(vec![(id(3), (None, Some(2))), (id(2), (None, Some(4)))]));
        assert_eq!(plan(&shown, 0, 1), Plan::Swap(vec![(id(1), (None, Some(2))), (id(2), (None, Some(1)))]));
    }

    #[test]
    fn not_past_a_disc() {
        let shown = [(id(1), (Some(1), Some(1))), (id(2), (Some(2), Some(1)))];
        assert_eq!(plan(&shown, 1, 0), Plan::OtherDisc(2));
        assert_eq!(plan(&shown, 0, 1), Plan::OtherDisc(1));
    }

    #[test]
    fn without_numbers_of_their_own_the_disc_is_numbered_as_shown() {
        // Two share 2, one has none; disc 2 is left alone.
        let shown = [(id(1), (Some(1), Some(1))), (id(2), (Some(1), Some(2))), (id(3), (Some(1), Some(2))), (id(4), (Some(1), None)), (id(9), (Some(2), Some(1)))];
        assert_eq!(
            plan(&shown, 3, 2),
            Plan::Numbered { disc: 1, count: 4, changes: vec![(id(4), (Some(1), Some(3))), (id(3), (Some(1), Some(4)))] }
        );
        // None numbered, no disc: 1, 2, 3, still no disc.
        let shown = [(id(1), (None, None)), (id(2), (None, None)), (id(3), (None, None))];
        assert_eq!(
            plan(&shown, 0, 1),
            Plan::Numbered { disc: 1, count: 3, changes: vec![(id(2), (None, Some(1))), (id(1), (None, Some(2))), (id(3), (None, Some(3)))] }
        );
        // Disc 1 written for some, none for others: all of them on disc 1.
        let shown = [(id(1), (Some(1), Some(1))), (id(2), (None, None))];
        assert_eq!(plan(&shown, 1, 0), Plan::Numbered { disc: 1, count: 2, changes: vec![(id(2), (Some(1), Some(1))), (id(1), (Some(1), Some(2)))] });
    }
}
