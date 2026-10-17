//! Listening together: shared sessions, and what other people are listening to.
//!
//! A session is a room a user starts and others join with its invite code. Everyone in it
//! hears the same thing at the same place, and anyone may pause, seek, skip or change the
//! queue. The room holds the player's state (the list, where in it, the hand-made queue,
//! paused or not, the position); clients apply it and send their own changes. A change is
//! sent with the version it was made on top of, and one made on an older version loses
//! (the client gets the current state back and follows it): two people skipping at once,
//! or every client noticing a track ended, settle on one answer.
//!
//! Rooms live in memory, per node (like the rate limits): a restart ends them. A room
//! ends when its members have all gone quiet. Clients follow a room by long polling
//! (`GET /api/together/{id}?version=`), which returns when the version changes.
//!
//! Besides rooms, a client that lets others see what it plays reports it
//! (`PUT /api/now-playing`) and others list it (`GET /api/now-playing`). Reports expire.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;

/// A member who hasn't asked for this long has left (a client polls at least every ~25 s).
const MEMBER_TIMEOUT: Duration = Duration::from_secs(75);
/// What a user reported playing stays listed this long without a new report.
const NOW_PLAYING_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_MEMBERS: usize = 20;
const MAX_TRACKS: usize = 10_000;
/// The longest a poll waits for a change.
const MAX_WAIT: u64 = 25;

/// The player's state, as clients send it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Playing {
    /// The list played from, and the place in it.
    pub list: Vec<Uuid>,
    pub index: usize,
    /// Tracks queued by hand, next up.
    pub queued: Vec<Uuid>,
    /// The track playing, if it came from the hand-made queue (else the list's `index` is).
    #[serde(default)]
    pub current_queued: Option<Uuid>,
    pub paused: bool,
    /// Where the current track is, in milliseconds, when this is sent.
    pub position_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub user_id: Uuid,
    pub username: String,
}

/// A room as a member sees it.
#[derive(Debug, Clone, Serialize)]
pub struct RoomView {
    pub id: Uuid,
    /// What others join with.
    pub code: String,
    pub version: u64,
    pub members: Vec<Member>,
    pub playing: Playing,
    /// Who made the last change.
    pub changed_by: Option<String>,
}

struct Room {
    code: String,
    version: u64,
    members: Vec<(Member, Instant)>,
    playing: Playing,
    /// When `playing.position_ms` was true.
    set_at: Instant,
    changed_by: Option<String>,
    tx: watch::Sender<u64>,
}

impl Room {
    fn view(&self, id: Uuid) -> RoomView {
        let mut playing = self.playing.clone();
        // The position as of now, so a client needs no clock of ours.
        if !playing.paused && !playing.list.is_empty() {
            playing.position_ms += self.set_at.elapsed().as_millis() as u64;
        }
        RoomView {
            id,
            code: self.code.clone(),
            version: self.version,
            members: self.members.iter().map(|(m, _)| m.clone()).collect(),
            playing,
            changed_by: self.changed_by.clone(),
        }
    }

    /// Drops members who've gone quiet. True if the room is empty then.
    fn forget_quiet(&mut self, now: Instant) -> bool {
        let before = self.members.len();
        self.members.retain(|(_, seen)| now.duration_since(*seen) < MEMBER_TIMEOUT);
        if self.members.len() != before {
            self.bump();
        }
        self.members.is_empty()
    }

    fn bump(&mut self) {
        self.version += 1;
        self.tx.send_replace(self.version);
    }
}

struct Reported {
    track: Uuid,
    paused: bool,
    at: Instant,
    username: String,
}

#[derive(Default)]
pub struct Together {
    rooms: Mutex<HashMap<Uuid, Room>>,
    now_playing: Mutex<HashMap<Uuid, Reported>>,
}

/// What a request for a room can come to.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// No such room, or the user isn't in it.
    NotFound,
    Full,
    TooLong,
}

impl From<Refused> for AppError {
    fn from(refused: Refused) -> AppError {
        match refused {
            Refused::NotFound => AppError::NotFound,
            Refused::Full => AppError::Conflict(format!("the session is full ({MAX_MEMBERS} people)")),
            Refused::TooLong => AppError::BadRequest(format!("a session's list holds at most {MAX_TRACKS} tracks")),
        }
    }
}

fn member_of(user: &CurrentUser) -> Result<Member, AppError> {
    let login = user.login.as_ref().ok_or(AppError::Unauthorized("log in to listen together"))?;
    Ok(Member { user_id: login.id, username: login.username.clone() })
}

/// A code to read out loud: no 0/O or 1/I.
fn new_code() -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    (0..8).map(|_| LETTERS[rand::random::<u8>() as usize % LETTERS.len()] as char).collect()
}

impl Together {
    fn expire(rooms: &mut HashMap<Uuid, Room>, now: Instant) {
        rooms.retain(|_, room| !room.forget_quiet(now));
    }

    /// Starts a room with `member` in it.
    pub fn create(&self, member: Member) -> RoomView {
        let id = Uuid::now_v7();
        let (tx, _) = watch::channel(0);
        let room = Room {
            code: new_code(),
            version: 0,
            members: vec![(member, Instant::now())],
            playing: Playing::default(),
            set_at: Instant::now(),
            changed_by: None,
            tx,
        };
        let view = room.view(id);
        let mut rooms = self.rooms.lock().unwrap();
        Self::expire(&mut rooms, Instant::now());
        rooms.insert(id, room);
        view
    }

    /// Adds `member` to the room with this code (again, if they're in it already).
    pub fn join(&self, code: &str, member: Member) -> Result<RoomView, Refused> {
        let now = Instant::now();
        let mut rooms = self.rooms.lock().unwrap();
        Self::expire(&mut rooms, now);
        let code = code.trim().to_ascii_uppercase();
        let (id, room) = rooms.iter_mut().find(|(_, r)| r.code == code).ok_or(Refused::NotFound)?;
        match room.members.iter_mut().find(|(m, _)| m.user_id == member.user_id) {
            Some((_, seen)) => *seen = now,
            None => {
                if room.members.len() >= MAX_MEMBERS {
                    return Err(Refused::Full);
                }
                room.members.push((member, now));
                room.bump();
            }
        }
        Ok(room.view(*id))
    }

    /// The room as `user` sees it (they're marked present). With `known` (the version they
    /// have), waits for a different one.
    pub async fn get(&self, id: Uuid, user: Uuid, known: Option<u64>, wait: Duration) -> Result<RoomView, Refused> {
        let mut changes = {
            let mut rooms = self.rooms.lock().unwrap();
            let room = Self::present(&mut rooms, id, user)?;
            if known != Some(room.version) {
                return Ok(room.view(id));
            }
            room.tx.subscribe()
        };
        let _ = tokio::time::timeout(wait, changes.wait_for(|version| Some(*version) != known)).await;
        let mut rooms = self.rooms.lock().unwrap();
        Ok(Self::present(&mut rooms, id, user)?.view(id))
    }

    fn present(rooms: &mut HashMap<Uuid, Room>, id: Uuid, user: Uuid) -> Result<&mut Room, Refused> {
        let now = Instant::now();
        Self::expire(rooms, now);
        let room = rooms.get_mut(&id).ok_or(Refused::NotFound)?;
        let (_, seen) = room.members.iter_mut().find(|(m, _)| m.user_id == user).ok_or(Refused::NotFound)?;
        *seen = now;
        Ok(room)
    }

    /// Sets the room's state, if `base` is its current version. Returns whether it was taken,
    /// and the room as it is then.
    pub fn set(&self, id: Uuid, user: &Member, base: u64, playing: Playing) -> Result<(bool, RoomView), Refused> {
        if playing.list.len() + playing.queued.len() > MAX_TRACKS {
            return Err(Refused::TooLong);
        }
        let mut rooms = self.rooms.lock().unwrap();
        let room = Self::present(&mut rooms, id, user.user_id)?;
        if room.version != base {
            return Ok((false, room.view(id)));
        }
        let mut playing = playing;
        playing.index = playing.index.min(playing.list.len().saturating_sub(1));
        room.playing = playing;
        room.set_at = Instant::now();
        room.changed_by = Some(user.username.clone());
        room.bump();
        Ok((true, room.view(id)))
    }

    /// `user` leaves; the last one out ends the room.
    pub fn leave(&self, id: Uuid, user: Uuid) {
        let mut rooms = self.rooms.lock().unwrap();
        if let Some(room) = rooms.get_mut(&id) {
            let before = room.members.len();
            room.members.retain(|(m, _)| m.user_id != user);
            if room.members.is_empty() {
                rooms.remove(&id);
            } else if room.members.len() != before {
                room.bump();
            }
        }
    }

    /// Records what `user` plays (None: nothing, which clears it).
    pub fn report(&self, user: &Member, playing: Option<(Uuid, bool)>) {
        let mut reports = self.now_playing.lock().unwrap();
        match playing {
            Some((track, paused)) => {
                reports.insert(user.user_id, Reported { track, paused, at: Instant::now(), username: user.username.clone() });
            }
            None => {
                reports.remove(&user.user_id);
            }
        }
    }

    /// What others are listening to: (user, name, track, paused, seconds since reported).
    fn others(&self, me: Uuid) -> Vec<Listening> {
        let mut reports = self.now_playing.lock().unwrap();
        reports.retain(|_, r| r.at.elapsed() < NOW_PLAYING_TIMEOUT);
        let mut list: Vec<Listening> = reports
            .iter()
            .filter(|(user, _)| **user != me)
            .map(|(user, r)| Listening { user_id: *user, username: r.username.clone(), track_id: r.track, paused: r.paused, seconds_ago: r.at.elapsed().as_secs() })
            .collect();
        list.sort_by_key(|l| l.username.to_lowercase());
        list
    }
}

#[derive(Debug, Serialize)]
pub struct Listening {
    pub user_id: Uuid,
    pub username: String,
    pub track_id: Uuid,
    pub paused: bool,
    pub seconds_ago: u64,
}

// ---- Routes ----

/// `POST /api/together`: starts a session; the caller is in it.
pub async fn create(State(state): State<AppState>, user: CurrentUser) -> Result<Json<RoomView>, AppError> {
    Ok(Json(state.together.create(member_of(&user)?)))
}

#[derive(Deserialize)]
pub struct Join {
    code: String,
}

/// `POST /api/together/join {code}`: joins the session with this invite code.
pub async fn join(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Join>) -> Result<Json<RoomView>, AppError> {
    Ok(Json(state.together.join(&body.code, member_of(&user)?)?))
}

#[derive(Deserialize)]
pub struct Poll {
    /// The version the client has: the answer waits for another.
    version: Option<u64>,
    /// Seconds to wait at most (default 0: answer now).
    wait: Option<u64>,
}

/// `GET /api/together/{id}?version=&wait=`: the session, as a member sees it.
pub async fn get(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Query(poll): Query<Poll>) -> Result<Json<RoomView>, AppError> {
    let member = member_of(&user)?;
    let wait = Duration::from_secs(poll.wait.unwrap_or(0).min(MAX_WAIT));
    Ok(Json(state.together.get(id, member.user_id, poll.version, wait).await?))
}

#[derive(Deserialize)]
pub struct Set {
    /// The version this change is made on top of.
    base: u64,
    #[serde(flatten)]
    playing: Playing,
}

#[derive(Serialize)]
pub struct SetAnswer {
    /// The change was taken. If not, the room had moved on: follow `room`.
    accepted: bool,
    room: RoomView,
}

/// `PUT /api/together/{id}/state`: a change to what plays, on top of version `base`.
pub async fn set(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, Json(body): Json<Set>) -> Result<Json<SetAnswer>, AppError> {
    let (accepted, room) = state.together.set(id, &member_of(&user)?, body.base, body.playing)?;
    Ok(Json(SetAnswer { accepted, room }))
}

/// `DELETE /api/together/{id}`: leaves the session.
pub async fn leave(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>) -> Result<(), AppError> {
    state.together.leave(id, member_of(&user)?.user_id);
    Ok(())
}

#[derive(Deserialize)]
pub struct Report {
    /// What plays; none: nothing (stopped, or no longer shared).
    track_id: Option<Uuid>,
    #[serde(default)]
    paused: bool,
}

/// `PUT /api/now-playing`: what the caller is listening to, for others to see. Clients send
/// it only when the user lets others see it, and again now and then (a report expires).
pub async fn report(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Report>) -> Result<(), AppError> {
    state.together.report(&member_of(&user)?, body.track_id.map(|t| (t, body.paused)));
    Ok(())
}

/// `GET /api/now-playing`: what others are listening to, of the tracks the caller can see.
pub async fn listening(State(state): State<AppState>, user: CurrentUser) -> Result<Json<Vec<Listening>>, AppError> {
    let mut list = state.together.others(member_of(&user)?.user_id);
    if !list.is_empty() {
        let ids: Vec<Uuid> = list.iter().map(|l| l.track_id).collect();
        let mut tx = state.db(&user).await?;
        let visible: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tracks WHERE id = ANY($1)").bind(&ids).fetch_all(&mut *tx).await?;
        list.retain(|l| visible.contains(&l.track_id));
    }
    Ok(Json(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: u128, name: &str) -> Member {
        Member { user_id: Uuid::from_u128(n), username: name.into() }
    }

    fn playing(tracks: &[u128], index: usize) -> Playing {
        Playing { list: tracks.iter().map(|t| Uuid::from_u128(*t)).collect(), index, ..Playing::default() }
    }

    #[test]
    fn joining_needs_the_code() {
        let together = Together::default();
        let room = together.create(member(1, "ann"));
        assert_eq!(together.join("WRONG", member(2, "bo")).unwrap_err(), Refused::NotFound);
        let joined = together.join(&room.code.to_lowercase(), member(2, "bo")).unwrap();
        assert_eq!(joined.id, room.id);
        assert_eq!(joined.members.len(), 2);
        // Joining again doesn't add them twice.
        assert_eq!(together.join(&room.code, member(2, "bo")).unwrap().members.len(), 2);
    }

    #[test]
    fn only_members_see_a_room() {
        let together = Together::default();
        let room = together.create(member(1, "ann"));
        let outsider = member(9, "eve");
        assert_eq!(together.set(room.id, &outsider, 0, playing(&[1], 0)).unwrap_err(), Refused::NotFound);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let seen = runtime.block_on(together.get(room.id, outsider.user_id, None, Duration::ZERO));
        assert_eq!(seen.unwrap_err(), Refused::NotFound);
    }

    #[test]
    fn a_change_on_an_old_version_loses() {
        let together = Together::default();
        let room = together.create(member(1, "ann"));
        together.join(&room.code, member(2, "bo")).unwrap();
        let (ann, bo) = (member(1, "ann"), member(2, "bo"));
        let version = together.join(&room.code, bo.clone()).unwrap().version;

        let (taken, now) = together.set(room.id, &ann, version, playing(&[1, 2, 3], 1)).unwrap();
        assert!(taken);
        assert_eq!(now.changed_by.as_deref(), Some("ann"));
        // Bo made a change on the version before Ann's: it's not taken, and Bo is shown Ann's.
        let (taken, now) = together.set(room.id, &bo, version, playing(&[7], 0)).unwrap();
        assert!(!taken);
        assert_eq!(now.playing.index, 1);
        assert_eq!(now.playing.list.len(), 3);
    }

    #[test]
    fn the_index_stays_inside_the_list() {
        let together = Together::default();
        let ann = member(1, "ann");
        let room = together.create(ann.clone());
        let (_, now) = together.set(room.id, &ann, room.version, playing(&[1, 2], 9)).unwrap();
        assert_eq!(now.playing.index, 1);
    }

    #[test]
    fn the_last_one_out_ends_the_room() {
        let together = Together::default();
        let room = together.create(member(1, "ann"));
        together.join(&room.code, member(2, "bo")).unwrap();
        together.leave(room.id, Uuid::from_u128(1));
        assert_eq!(together.join(&room.code, member(3, "cy")).unwrap().members.len(), 2);
        together.leave(room.id, Uuid::from_u128(2));
        together.leave(room.id, Uuid::from_u128(3));
        assert_eq!(together.join(&room.code, member(4, "di")).unwrap_err(), Refused::NotFound);
    }

    #[test]
    fn a_poll_waits_for_a_change() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        runtime.block_on(async {
            let together = std::sync::Arc::new(Together::default());
            let ann = member(1, "ann");
            let room = together.create(ann.clone());
            let waiting = {
                let together = together.clone();
                tokio::spawn(async move { together.get(room.id, ann.user_id, Some(room.version), Duration::from_secs(5)).await })
            };
            tokio::time::sleep(Duration::from_millis(50)).await;
            together.join(&room.code, member(2, "bo")).unwrap();
            let seen = waiting.await.unwrap().unwrap();
            assert_eq!(seen.members.len(), 2);

            // Nothing changes: the poll ends with the same version when the wait is up.
            let again = together.get(room.id, Uuid::from_u128(1), Some(seen.version), Duration::from_millis(50)).await.unwrap();
            assert_eq!(again.version, seen.version);
        });
    }

    #[test]
    fn the_position_moves_on_while_playing_but_not_while_paused() {
        let together = Together::default();
        let ann = member(1, "ann");
        let room = together.create(ann.clone());
        let mut state = playing(&[1], 0);
        state.position_ms = 5_000;
        together.set(room.id, &ann, room.version, state.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let seen = runtime.block_on(together.get(room.id, ann.user_id, None, Duration::ZERO)).unwrap();
        assert!(seen.playing.position_ms >= 5_030);

        state.paused = true;
        together.set(room.id, &ann, seen.version, state).unwrap();
        std::thread::sleep(Duration::from_millis(30));
        let seen = runtime.block_on(together.get(room.id, ann.user_id, None, Duration::ZERO)).unwrap();
        assert_eq!(seen.playing.position_ms, 5_000);
    }

    #[test]
    fn others_see_what_is_reported_but_not_their_own() {
        let together = Together::default();
        let (ann, bo) = (member(1, "ann"), member(2, "bo"));
        together.report(&ann, Some((Uuid::from_u128(10), false)));
        together.report(&bo, Some((Uuid::from_u128(11), true)));
        let seen = together.others(ann.user_id);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].username, "bo");
        assert!(seen[0].paused);
        together.report(&bo, None);
        assert!(together.others(ann.user_id).is_empty());
    }
}
