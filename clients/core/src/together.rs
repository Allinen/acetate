//! Listening together: a session several people hear in step, and what others listen to.
//!
//! The server keeps a session's state (the list, the place in it, the hand-made queue,
//! paused or not, the position) and a version that rises with every change. A client
//! follows by long polling, and sends its own changes on top of the version it has; a
//! change on an old version is refused and the client follows what the server answers.
//!
//! This is the part that decides, with no player and no UI: `compare` says what a player
//! has to change to be where the session is. It is used both ways. A session update that
//! arrives is applied by doing what `compare(session, player)` says; and when no update
//! has come but `compare(session, player)` says something, the player was changed by its
//! user, and that is sent.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{Api, ApiError};

/// A position this far from where the session says it should be is a seek (smaller gaps are
/// the players' own jitter and the time a message took).
pub const SEEK_TOLERANCE: Duration = Duration::from_secs(3);

/// What plays, as the server keeps it for a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Playing {
    pub list: Vec<Uuid>,
    pub index: usize,
    pub queued: Vec<Uuid>,
    /// The track playing, if it came from the hand-made queue (else `list[index]` is).
    #[serde(default)]
    pub current_queued: Option<Uuid>,
    pub paused: bool,
    pub position_ms: u64,
}

impl Playing {
    /// The track that plays.
    pub fn current(&self) -> Option<Uuid> {
        self.current_queued.or_else(|| self.list.get(self.index).copied())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Member {
    pub user_id: Uuid,
    pub username: String,
}

/// A session as the server tells it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Room {
    pub id: Uuid,
    /// What others join with.
    pub code: String,
    pub version: u64,
    pub members: Vec<Member>,
    pub playing: Playing,
    /// Who made the last change.
    pub changed_by: Option<String>,
}

/// What someone else is listening to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Listening {
    pub user_id: Uuid,
    pub username: String,
    pub track_id: Uuid,
    pub paused: bool,
    pub seconds_ago: u64,
}

#[derive(Debug, Deserialize)]
struct Answer {
    accepted: bool,
    room: Room,
}

/// A session, as last agreed with the server, and when it was heard.
#[derive(Debug, Clone)]
pub struct Synced {
    pub room: Room,
    pub heard: Instant,
}

impl Synced {
    pub fn new(room: Room) -> Synced {
        Synced { room, heard: Instant::now() }
    }

    /// Where the session's track is now: the position it came with, moved on while it plays.
    pub fn position_now(&self) -> Duration {
        let moved = if self.room.playing.paused { Duration::ZERO } else { self.heard.elapsed() };
        Duration::from_millis(self.room.playing.position_ms) + moved
    }
}

/// A player's state, in the terms of a session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Local {
    pub list: Vec<Uuid>,
    pub index: usize,
    pub queued: Vec<Uuid>,
    pub current_queued: Option<Uuid>,
    /// Something is loaded (else the rest means nothing).
    pub loaded: bool,
    pub paused: bool,
    pub position: Duration,
}

impl Local {
    pub fn current(&self) -> Option<Uuid> {
        if !self.loaded {
            return None;
        }
        self.current_queued.or_else(|| self.list.get(self.index).copied())
    }

    /// What to send to the server to make the session this.
    pub fn playing(&self) -> Playing {
        Playing {
            list: self.list.clone(),
            index: self.index,
            queued: self.queued.clone(),
            current_queued: self.current_queued,
            paused: self.paused,
            position_ms: self.position.as_millis() as u64,
        }
    }
}

/// What a player has to change to be where a session is.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// Nothing: they agree.
    Nothing,
    /// A different track, list or place in it: load the session's, at `position`, and pause
    /// or play as it does.
    Replace { position: Duration },
    /// Only the hand-made queue differs.
    Queue,
    Pause(bool),
    Seek(Duration),
}

/// What `local` has to change to be where `session` is (`position`: where the session's
/// track is now). `known`: whether this client has a track (a track it can't see isn't
/// held against the player, which can never have it).
pub fn compare(session: &Playing, position: Duration, local: &Local, known: &dyn Fn(Uuid) -> bool) -> Change {
    let only_known = |ids: &[Uuid]| -> Vec<Uuid> { ids.iter().copied().filter(|id| known(*id)).collect() };
    let Some(wanted) = session.current() else {
        return Change::Nothing;
    };
    // A session on a track this client can't play is left alone.
    if !known(wanted) {
        return Change::Nothing;
    }
    if !local.loaded || local.current() != Some(wanted) || only_known(&session.list) != only_known(&local.list) {
        return Change::Replace { position };
    }
    if only_known(&session.queued) != only_known(&local.queued) {
        return Change::Queue;
    }
    if session.paused != local.paused {
        return Change::Pause(session.paused);
    }
    if local.position.abs_diff(position) > SEEK_TOLERANCE {
        return Change::Seek(position);
    }
    Change::Nothing
}

// ---- Talking to the server ----

pub async fn start(api: &Api) -> Result<Room, ApiError> {
    api.send_json(reqwest::Method::POST, "/api/together", None).await
}

pub async fn join(api: &Api, code: &str) -> Result<Room, ApiError> {
    api.send_json(reqwest::Method::POST, "/api/together/join", Some(serde_json::json!({ "code": code }))).await
}

/// The session; with `known` (the version we have), waits up to `wait` seconds for another.
pub async fn poll(api: &Api, id: Uuid, known: Option<u64>, wait: u64) -> Result<Room, ApiError> {
    let mut path = format!("/api/together/{id}?wait={wait}");
    if let Some(version) = known {
        path.push_str(&format!("&version={version}"));
    }
    api.send_json(reqwest::Method::GET, &path, None).await
}

/// Sends a change on top of version `base`. Returns the session as it is then, and whether
/// the change was taken (if not, the session had moved on and is what to follow).
pub async fn send(api: &Api, id: Uuid, base: u64, playing: &Playing) -> Result<(bool, Room), ApiError> {
    let mut body = serde_json::to_value(playing).expect("a state is JSON");
    body["base"] = base.into();
    let answer: Answer = api.send_json(reqwest::Method::PUT, &format!("/api/together/{id}/state"), Some(body)).await?;
    Ok((answer.accepted, answer.room))
}

pub async fn leave(api: &Api, id: Uuid) -> Result<(), ApiError> {
    api.send(reqwest::Method::DELETE, &format!("/api/together/{id}"), None).await
}

/// Tells the server what plays, for others to see (None: nothing).
pub async fn report(api: &Api, playing: Option<(Uuid, bool)>) -> Result<(), ApiError> {
    let body = serde_json::json!({ "track_id": playing.map(|p| p.0), "paused": playing.is_some_and(|p| p.1) });
    api.send(reqwest::Method::PUT, "/api/now-playing", Some(body)).await
}

/// What others are listening to.
pub async fn listening(api: &Api) -> Result<Vec<Listening>, ApiError> {
    api.send_json(reqwest::Method::GET, "/api/now-playing", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn session(list: &[u128], index: usize) -> Playing {
        Playing { list: list.iter().map(|n| id(*n)).collect(), index, ..Playing::default() }
    }

    fn local(list: &[u128], index: usize) -> Local {
        Local { list: list.iter().map(|n| id(*n)).collect(), index, loaded: true, ..Local::default() }
    }

    const ALL: &dyn Fn(Uuid) -> bool = &|_| true;

    #[test]
    fn the_same_state_changes_nothing() {
        let at = Duration::from_secs(10);
        let mut l = local(&[1, 2, 3], 1);
        l.position = at;
        assert_eq!(compare(&session(&[1, 2, 3], 1), at, &l, ALL), Change::Nothing);
    }

    #[test]
    fn another_track_or_list_replaces() {
        let at = Duration::ZERO;
        assert_eq!(compare(&session(&[1, 2, 3], 2), at, &local(&[1, 2, 3], 1), ALL), Change::Replace { position: at });
        assert_eq!(compare(&session(&[4, 5], 0), at, &local(&[1, 2, 3], 0), ALL), Change::Replace { position: at });
        assert_eq!(compare(&session(&[1], 0), at, &Local::default(), ALL), Change::Replace { position: at }, "nothing loaded");
    }

    #[test]
    fn a_queued_track_playing_is_the_current() {
        let mut s = session(&[1, 2], 0);
        s.current_queued = Some(id(9));
        let mut l = local(&[1, 2], 0);
        assert_eq!(compare(&s, Duration::ZERO, &l, ALL), Change::Replace { position: Duration::ZERO });
        l.current_queued = Some(id(9));
        assert_eq!(compare(&s, Duration::ZERO, &l, ALL), Change::Nothing);
    }

    #[test]
    fn queue_pause_and_seek_are_told_apart() {
        let at = Duration::from_secs(30);
        let mut s = session(&[1, 2], 0);
        let mut l = local(&[1, 2], 0);
        l.position = at;

        s.queued = vec![id(7)];
        assert_eq!(compare(&s, at, &l, ALL), Change::Queue);
        l.queued = vec![id(7)];

        s.paused = true;
        assert_eq!(compare(&s, at, &l, ALL), Change::Pause(true));
        l.paused = true;

        // A little off is the players' jitter; a lot is a seek.
        assert_eq!(compare(&s, at + Duration::from_secs(2), &l, ALL), Change::Nothing);
        assert_eq!(compare(&s, at + Duration::from_secs(20), &l, ALL), Change::Seek(at + Duration::from_secs(20)));
    }

    #[test]
    fn what_this_client_cant_see_isnt_held_against_it() {
        // Track 2 is one this client doesn't have: its list is the session's without it.
        let known: &dyn Fn(Uuid) -> bool = &|t| t != id(2);
        let l = local(&[1, 3], 0);
        assert_eq!(compare(&session(&[1, 2, 3], 0), Duration::ZERO, &l, known), Change::Nothing);
        // A session on a track it can't have is left alone.
        assert_eq!(compare(&session(&[1, 2, 3], 1), Duration::ZERO, &l, known), Change::Nothing);
    }

    #[test]
    fn the_session_position_moves_on_unless_paused() {
        let mut room = Room { id: id(1), code: "X".into(), version: 1, members: vec![], playing: session(&[1], 0), changed_by: None };
        room.playing.position_ms = 5_000;
        let mut synced = Synced::new(room);
        synced.heard -= Duration::from_secs(2);
        assert!(synced.position_now() >= Duration::from_secs(7));
        synced.room.playing.paused = true;
        assert_eq!(synced.position_now(), Duration::from_secs(5));
    }
}
