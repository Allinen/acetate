//! An album's tracks moved in its tracklist (edit mode): Alt+↑/↓ (or Alt+K/J) moves the
//! selected track one place, on its disc. The list shows it moved at once; each move goes
//! to the server (`PUT /api/albums/{id}/track-numbers`), in order, one request at a time,
//! and the list keeps the moves until the library has them.
//!
//! A move swaps the two tracks' places (disc and number), so nothing else changes. When
//! that can't order them (one has no number, or two on the disc share one), the disc's
//! tracks are numbered 1, 2, 3… as they're shown, with the move, and the notice says so.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use uuid::Uuid;

use crate::catalog::index::album_order;
use crate::catalog::model::Track;
use crate::catalog::Catalog;
use crate::ui::{Action, Shell};

/// A track's place on an album: its disc and number.
pub type Place = (Option<i16>, Option<i16>);

/// One move's request: the album, and the tracks' new places.
type Request = (Uuid, Vec<(Uuid, Place)>);

#[derive(Default)]
pub struct TrackOrder {
    /// The album whose moves are shown.
    album: Option<Uuid>,
    /// Moved tracks' places on it as they're to be (until the library has them).
    wanted: HashMap<Uuid, Place>,
    /// Sends the moves, in order, one at a time.
    queue: Option<UnboundedSender<Request>>,
    sent: Arc<Mutex<Sent>>,
    /// Answered, and the library not asked for the changes yet.
    unrefreshed: bool,
    /// The library's generation when it was last asked for them.
    refreshed_at: Option<u64>,
}

#[derive(Default)]
struct Sent {
    /// Requests sent, not answered yet.
    waiting: usize,
    /// One was answered since the last look.
    answered: bool,
    /// One failed (the moves after it aren't sent: they built on it).
    failed: Option<String>,
    /// Dropping the moves after the one that failed (until none is waiting).
    dropping: bool,
}

impl TrackOrder {
    /// Each frame, before the list: takes the answers, asks the library for the changes once
    /// they're all in, and lets go of the moves once it has them.
    pub fn update(&mut self, shell: &mut Shell, catalog: &Catalog, album: Uuid) {
        if self.album != Some(album) {
            self.album = Some(album);
            self.wanted.clear();
        }
        let (waiting, answered, failed) = {
            let mut sent = self.sent.lock().unwrap();
            (sent.waiting, std::mem::take(&mut sent.answered), sent.failed.take())
        };
        if let Some(err) = failed {
            shell.playlists.notice = Some((format!("Couldn't move the track: {err}"), Instant::now()));
            // The library as it is (the moves before it are in it).
            self.wanted.clear();
        }
        self.unrefreshed |= answered;
        if self.unrefreshed && waiting == 0 {
            self.unrefreshed = false;
            self.refreshed_at = Some(shell.generation);
            shell.act(Action::RefreshLibrary);
        }
        if waiting == 0 && !self.unrefreshed && !self.wanted.is_empty() {
            // A track gone from the library has nothing to wait for.
            let caught_up = self.wanted.iter().all(|(id, wanted)| catalog.track_index(*id).is_none_or(|t| place(&catalog.tracks[t as usize], album) == *wanted));
            // (Twice: a refresh started before the answer can bring the library as it was.)
            let refreshed_twice = self.refreshed_at.is_some_and(|at| shell.generation >= at + 2);
            if caught_up || refreshed_twice {
                self.wanted.clear();
            }
        }
    }

    /// The album's tracks (positions in the catalog) in the order they're to be: None while
    /// that's the library's.
    pub fn tracks(&self, catalog: &Catalog, album: Uuid, tracks: &[u32]) -> Option<Vec<u32>> {
        if self.wanted.is_empty() || self.album != Some(album) {
            return None;
        }
        let mut list = tracks.to_vec();
        list.sort_by(|&a, &b| {
            let (a, b) = (&catalog.tracks[a as usize], &catalog.tracks[b as usize]);
            album_order(self.place(a, album), &a.title, self.place(b, album), &b.title)
        });
        Some(list)
    }

    /// Moved tracks' places as they're to be (for their numbers), if any.
    pub fn places(&self) -> Option<&HashMap<Uuid, Place>> {
        (!self.wanted.is_empty()).then_some(&self.wanted)
    }

    /// Moves the track at `row` of `tracks` (the album's, as shown, in its own order) `by`
    /// one place up or down. Where it is then, if it moved.
    pub fn move_track(&mut self, shell: &mut Shell, catalog: &Catalog, album: Uuid, tracks: &[u32], row: usize, by: isize) -> Option<usize> {
        let to = row.checked_add_signed(by).filter(|&to| to < tracks.len())?;
        // A move failed and the ones after it are being dropped: not this one too.
        if self.sent.lock().unwrap().dropping {
            return None;
        }
        let shown: Vec<(Uuid, Place)> = tracks.iter().map(|&t| &catalog.tracks[t as usize]).map(|t| (t.id, self.place(t, album))).collect();
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
        self.album = Some(album);
        self.wanted.extend(changes.iter().copied());
        self.send(shell, album, changes);
        let id = shown[row].0;
        self.tracks(catalog, album, tracks)?.iter().position(|&t| catalog.tracks[t as usize].id == id)
    }

    /// The track's place on the album: as it's to be if moved, else the library's.
    fn place(&self, t: &Track, album: Uuid) -> Place {
        self.wanted.get(&t.id).copied().unwrap_or_else(|| place(t, album))
    }

    fn send(&mut self, shell: &Shell, album: Uuid, changes: Vec<(Uuid, Place)>) {
        if changes.is_empty() {
            return;
        }
        let queue = self.queue.get_or_insert_with(|| {
            let (sender, mut requests) = unbounded_channel::<Request>();
            let (api, sent, ctx) = (shell.api.clone(), self.sent.clone(), shell.ctx.clone());
            shell.runtime.spawn(async move {
                while let Some((album, places)) = requests.recv().await {
                    let result = if sent.lock().unwrap().dropping {
                        Ok(())
                    } else {
                        let tracks: Vec<Value> = places.iter().map(|(id, (disc, number))| json!({ "track_id": id, "disc_number": disc, "track_number": number })).collect();
                        api.send(reqwest::Method::PUT, &format!("/api/albums/{album}/track-numbers"), Some(json!({ "tracks": tracks }))).await.map_err(|err| err.message)
                    };
                    let mut sent = sent.lock().unwrap();
                    sent.waiting -= 1;
                    sent.answered = true;
                    if let Err(err) = result {
                        sent.failed = Some(err);
                        sent.dropping = true;
                    }
                    if sent.waiting == 0 {
                        sent.dropping = false;
                    }
                    ctx.request_repaint();
                }
            });
            sender
        });
        self.sent.lock().unwrap().waiting += 1;
        if queue.send((album, changes)).is_err() {
            self.sent.lock().unwrap().waiting -= 1;
        }
    }
}

/// The track's place on the album, as the library has it.
fn place(t: &Track, album: Uuid) -> Place {
    t.albums.iter().find(|m| m.0 == album).map_or((None, None), |m| (m.1, m.2))
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
