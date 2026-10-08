//! The queue, in two parts:
//!
//! - **the list** (automatic): what was played from (an album, a playlist, a table),
//!   and where in it playback is. Playing something from a page replaces it. Shuffle
//!   mixes this part.
//! - **the queue** (manual): tracks queued by hand (`q` at its end, Shift+Q at its
//!   start). They play before the list goes on, and stay when another list is played.
//!
//! Nothing here plays anything: the controller asks what comes next, hands it to the
//! audio thread, and tells the queue when it plays.

use std::sync::Arc;

use super::QueueItem;

/// What plays, and whether it came from the queue (else from the list, at `list_index`).
#[derive(Debug, Clone)]
pub struct Current {
    pub item: QueueItem,
    pub queued: bool,
}

/// What comes after the current track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upcoming {
    /// The same track again (looping it).
    Again,
    /// The first track in the queue.
    Queued,
    /// This track of the list.
    List(usize),
}

/// A place in the queue or the list (for the Queue page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Queued(usize),
    List(usize),
}

#[derive(Default)]
pub struct Queue {
    pub list: Arc<Vec<QueueItem>>,
    /// Where in the list playback is: its current track, or (while a queued track plays)
    /// the list's last one played.
    pub list_index: usize,
    /// While shuffled: the list in its own order, and each shuffled item's place in it.
    original: Option<(Vec<usize>, Arc<Vec<QueueItem>>)>,
    pub queued: Arc<Vec<QueueItem>>,
    pub current: Option<Current>,
}

impl Queue {
    /// Plays from a list: `start` first, then the rest (shuffled: the others in a random
    /// order). The queue stays. Returns the track to play.
    pub fn play_list(&mut self, items: Vec<QueueItem>, start: usize, shuffled: bool) -> Option<QueueItem> {
        let start = start.min(items.len().checked_sub(1)?);
        if shuffled {
            let mut order: Vec<usize> = (0..items.len()).filter(|&i| i != start).collect();
            super::shuffle(&mut order);
            order.insert(0, start);
            self.list = Arc::new(order.iter().map(|&i| items[i].clone()).collect());
            self.original = Some((order, Arc::new(items)));
            self.list_index = 0;
        } else {
            self.list = Arc::new(items);
            self.original = None;
            self.list_index = start;
        }
        self.play(Place::List(self.list_index))
    }

    /// What comes after the current track.
    pub fn upcoming(&self, repeat: bool) -> Option<Upcoming> {
        if repeat && self.current.is_some() {
            Some(Upcoming::Again)
        } else if !self.queued.is_empty() {
            Some(Upcoming::Queued)
        } else if self.list_index + 1 < self.list.len() {
            Some(Upcoming::List(self.list_index + 1))
        } else {
            None
        }
    }

    pub fn item(&self, upcoming: Upcoming) -> Option<&QueueItem> {
        match upcoming {
            Upcoming::Again => self.current.as_ref().map(|c| &c.item),
            Upcoming::Queued => self.queued.first(),
            Upcoming::List(i) => self.list.get(i),
        }
    }

    /// The upcoming track plays now.
    pub fn advance(&mut self, upcoming: Upcoming) {
        match upcoming {
            Upcoming::Again => {}
            Upcoming::Queued => {
                self.play(Place::Queued(0));
            }
            Upcoming::List(i) => {
                self.play(Place::List(i));
            }
        }
    }

    /// Plays a place: a list track (the list goes on from there), or a queued one (it
    /// and the ones before it leave the queue). Returns the track.
    pub fn play(&mut self, place: Place) -> Option<QueueItem> {
        let item = match place {
            Place::List(i) => {
                let item = self.list.get(i)?.clone();
                self.list_index = i;
                self.current = Some(Current { item: item.clone(), queued: false });
                item
            }
            Place::Queued(i) => {
                if i >= self.queued.len() {
                    return None;
                }
                let queued = Arc::make_mut(&mut self.queued);
                let item = queued.drain(..=i).next_back()?;
                self.current = Some(Current { item: item.clone(), queued: true });
                item
            }
        };
        Some(item)
    }

    /// Where Previous goes: from a queued track, back to the list's (it played before);
    /// else the list's one before (None: the start of the list: play it again).
    pub fn previous(&self) -> Option<Place> {
        match &self.current {
            Some(Current { queued: true, .. }) if !self.list.is_empty() => Some(Place::List(self.list_index)),
            _ if self.list_index > 0 => Some(Place::List(self.list_index - 1)),
            _ => None,
        }
    }

    /// Queues tracks by hand: at the end of the queue, or (`first`) at its start (in
    /// their order). With nothing playing, the first of them plays: returned.
    pub fn enqueue(&mut self, items: Vec<QueueItem>, first: bool) -> Option<QueueItem> {
        let queued = Arc::make_mut(&mut self.queued);
        if first {
            queued.splice(0..0, items);
        } else {
            queued.extend(items);
        }
        if self.current.is_none() { self.play(Place::Queued(0)) } else { None }
    }

    /// Takes a place out (not the one playing). Returns whether it did.
    pub fn remove(&mut self, place: Place) -> bool {
        match place {
            Place::Queued(i) if i < self.queued.len() => {
                Arc::make_mut(&mut self.queued).remove(i);
                true
            }
            Place::List(i) if i < self.list.len() && !(i == self.list_index && self.playing_from_list()) => {
                Arc::make_mut(&mut self.list).remove(i);
                if let Some((order, original)) = self.original.as_mut() {
                    let gone = order.remove(i);
                    Arc::make_mut(original).remove(gone);
                    for o in order.iter_mut() {
                        if *o > gone {
                            *o -= 1;
                        }
                    }
                }
                if i < self.list_index {
                    self.list_index -= 1;
                }
                true
            }
            _ => false,
        }
    }

    /// Empties the queue (the list stays).
    pub fn clear_queued(&mut self) -> bool {
        let had = !self.queued.is_empty();
        self.queued = Arc::default();
        had
    }

    /// Moves a queued track to another place in the queue. Returns whether it did.
    pub fn move_queued(&mut self, from: usize, to: usize) -> bool {
        let len = self.queued.len();
        if from == to || from >= len || to >= len {
            return false;
        }
        let queued = Arc::make_mut(&mut self.queued);
        let item = queued.remove(from);
        queued.insert(to, item);
        true
    }

    /// Shuffles the rest of the list (what's played and playing stays), or puts it back
    /// in its own order, from where playback is in it.
    pub fn set_shuffled(&mut self, on: bool) {
        if on {
            let fixed = (self.list_index + 1).min(self.list.len());
            let mut order: Vec<usize> = (0..self.list.len()).collect();
            super::shuffle(&mut order[fixed..]);
            let list = self.list.clone();
            self.list = Arc::new(order.iter().map(|&i| list[i].clone()).collect());
            self.original = Some((order, list));
        } else if let Some((order, original)) = self.original.take() {
            self.list_index = order.get(self.list_index).copied().unwrap_or(0);
            self.list = original;
        }
    }

    fn playing_from_list(&self) -> bool {
        self.current.as_ref().is_some_and(|c| !c.queued)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn item(n: u128) -> QueueItem {
        QueueItem {
            track_id: Uuid::from_u128(n),
            file_id: Uuid::from_u128(n),
            source: Default::default(),
            title: n.to_string(),
            artist: String::new(),
            album: None,
            duration_ms: None,
            cover: None,
            codec: None,
        }
    }

    fn items(ns: &[u128]) -> Vec<QueueItem> {
        ns.iter().map(|&n| item(n)).collect()
    }

    fn titles(items: &[QueueItem]) -> Vec<String> {
        items.iter().map(|i| i.title.clone()).collect()
    }

    /// Plays on to the end; the titles in the order they played.
    fn play_out(q: &mut Queue) -> Vec<String> {
        let mut played = vec![q.current.as_ref().unwrap().item.title.clone()];
        while let Some(next) = q.upcoming(false) {
            q.advance(next);
            played.push(q.current.as_ref().unwrap().item.title.clone());
        }
        played
    }

    #[test]
    fn queued_tracks_play_before_the_list_goes_on() {
        let mut q = Queue::default();
        q.play_list(items(&[1, 2, 3]), 0, false);
        q.enqueue(items(&[10]), false); // q
        q.enqueue(items(&[11]), false); // q: after 10
        q.enqueue(items(&[20, 21]), true); // Shift+Q: before 10, in their order
        assert_eq!(play_out(&mut q), ["1", "20", "21", "10", "11", "2", "3"]);
    }

    #[test]
    fn playing_another_list_keeps_the_queue() {
        let mut q = Queue::default();
        q.play_list(items(&[1, 2]), 0, false);
        q.enqueue(items(&[10]), false);
        q.play_list(items(&[5, 6, 7]), 1, false);
        assert_eq!(play_out(&mut q), ["6", "10", "7"]);
    }

    #[test]
    fn previous_from_a_queued_track_goes_back_to_the_lists() {
        let mut q = Queue::default();
        q.play_list(items(&[1, 2, 3]), 1, false);
        q.enqueue(items(&[10]), false);
        q.advance(q.upcoming(false).unwrap()); // 10 plays
        assert_eq!(q.previous(), Some(Place::List(1)), "2, the list's last one");
        q.play(Place::List(1));
        assert_eq!(q.previous(), Some(Place::List(0)));
    }

    #[test]
    fn with_nothing_playing_queueing_plays() {
        let mut q = Queue::default();
        assert_eq!(q.enqueue(items(&[10, 11]), false).map(|i| i.title), Some("10".into()));
        assert_eq!(titles(&q.queued), ["11"]);
    }

    #[test]
    fn the_queue_page_plays_removes_and_moves() {
        let mut q = Queue::default();
        q.play_list(items(&[1, 2, 3, 4]), 0, false);
        q.enqueue(items(&[10, 11, 12]), false);
        assert!(q.move_queued(2, 0)); // 12, 10, 11
        assert!(q.remove(Place::Queued(1))); // 12, 11
        assert!(q.remove(Place::List(2))); // the list: 1, 2, 4
        assert!(!q.remove(Place::List(0)), "not the one playing");
        // Playing a queued track skips the ones before it.
        q.play(Place::Queued(1));
        assert!(q.queued.is_empty());
        assert_eq!(play_out(&mut q), ["11", "2", "4"]);
    }

    #[test]
    fn shuffle_mixes_the_rest_of_the_list_only_and_unshuffles_from_where_it_is() {
        let mut q = Queue::default();
        q.play_list(items(&(1..=50).collect::<Vec<_>>()), 9, false); // "10" plays
        q.enqueue(items(&[100]), false);
        q.set_shuffled(true);
        assert_eq!(q.list[9].title, "10", "what played stays");
        assert_eq!(titles(&q.queued), ["100"], "the queue isn't shuffled");
        let mut rest = titles(&q.list[10..]);
        rest.sort_by_key(|t| t.parse::<u32>().unwrap());
        assert_eq!(rest, (11..=50).map(|n| n.to_string()).collect::<Vec<_>>());
        // Two more list tracks play, then shuffle goes off: the own order, from there.
        q.play(Place::List(11));
        let playing = q.current.as_ref().unwrap().item.title.clone();
        q.set_shuffled(false);
        assert_eq!(q.list[q.list_index].title, playing);
    }
}
