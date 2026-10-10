//! Changes made here that the server has to hear about: stars, ratings, plays and tracks
//! added to playlists. Each goes into the outbox (kept on disk at once, so a crash or a
//! restart loses nothing) and is sent from there, in order: right away when the server
//! is there, else when it's back. The app shows the change at once, as if it were sent.
//!
//! - A star or rating replaces one of the same thing still waiting (only the latest is
//!   sent), and remembers the value it changed from. If the server had another value by
//!   the time it's sent (changed elsewhere meanwhile), the change still wins, but the user
//!   is told it replaced that one.
//! - A play is sent with when it was played and a key of its own, so one sent twice (the
//!   answer was lost) counts once.
//! - The server refusing a change (the track is gone, no permission) drops it, saying so.
//!   Not reaching the server keeps it, for the next time.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{Api, ApiError};

/// A change to send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum Change {
    /// `kind`: the API's word for it ("tracks", "albums", "artists"). `name`: what to call
    /// it when telling the user something about it.
    Star { kind: String, id: Uuid, name: String, starred: bool, from: bool },
    Rate { kind: String, id: Uuid, name: String, rating: i16, from: i16 },
    /// `at`: seconds since 1970.
    Play { track: Uuid, at: u64, key: Uuid },
    /// `at`: before the playlist's `at`-th entry (None: at the end).
    Add {
        playlist: Uuid,
        name: String,
        tracks: Vec<Uuid>,
        #[serde(default)]
        at: Option<usize>,
    },
}

impl Change {
    /// A play of `track`, now.
    pub fn played(track: Uuid) -> Change {
        Change::Play { track, at: now(), key: Uuid::new_v4() }
    }

    /// What it stars or rates.
    fn marks(&self) -> Option<Uuid> {
        match self {
            Change::Star { id, .. } | Change::Rate { id, .. } => Some(*id),
            Change::Play { track, .. } => Some(*track),
            Change::Add { .. } => None,
        }
    }
}

/// What came of the changes, for the app to take (`take`).
#[derive(Debug, Clone, PartialEq)]
pub enum Sent {
    /// A play was counted here (the play count shown goes up at once).
    Played(Uuid),
    /// A star, rating or play reached the server: what's shown stays until a catalog
    /// asked for after now (it has the change).
    Marked(Uuid),
    /// The server refused a star (or a rating): it's shown as it was before.
    Unstar { id: Uuid, starred: bool },
    Unrate { id: Uuid, rating: i16 },
    /// A playlist changed (or a change to it was refused): fetch it again.
    Playlist(Uuid),
    /// Something to tell the user.
    Notice(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Waiting {
    /// When it was made (seconds since 1970).
    made: u64,
    change: Change,
}

#[derive(Default)]
struct Inner {
    waiting: VecDeque<Waiting>,
    /// Being sent now (out of `waiting`, so nothing replaces it meanwhile; kept on disk).
    sending: Option<Waiting>,
    sent: Vec<Sent>,
}

pub struct Outbox {
    file: PathBuf,
    api: Api,
    runtime: tokio::runtime::Handle,
    waker: crate::Waker,
    inner: Mutex<Inner>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Outbox {
    /// The outbox kept in `file` (`outbox-<key>.json`); what's in it is sent now.
    pub fn open(file: PathBuf, api: Api, runtime: tokio::runtime::Handle, waker: crate::Waker) -> Arc<Outbox> {
        let waiting: VecDeque<Waiting> = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        let outbox = Arc::new(Outbox { file, api, runtime, waker, inner: Mutex::new(Inner { waiting, ..Inner::default() }) });
        outbox.kick();
        outbox
    }

    /// Puts a change in (replacing a star or rating of the same thing still waiting), and
    /// sends it if it can.
    pub fn push(self: &Arc<Self>, change: Change) {
        {
            let mut inner = self.inner.lock().unwrap();
            let mut change = change;
            // A star or rating waiting for the same thing: this one replaces it, and changed
            // from what that one changed from. Back to that: nothing to send.
            let same = inner.waiting.iter().position(|w| match (&w.change, &change) {
                (Change::Star { id: a, .. }, Change::Star { id: b, .. }) | (Change::Rate { id: a, .. }, Change::Rate { id: b, .. }) => a == b,
                _ => false,
            });
            let mut cancelled = false;
            if let Some(at) = same {
                let earlier = inner.waiting.remove(at).expect("found").change;
                match (&earlier, &mut change) {
                    (Change::Star { from, .. }, Change::Star { from: new_from, starred, .. }) => {
                        *new_from = *from;
                        cancelled = starred == from;
                    }
                    (Change::Rate { from, .. }, Change::Rate { from: new_from, rating, .. }) => {
                        *new_from = *from;
                        cancelled = rating == from;
                    }
                    _ => {}
                }
            }
            if let Change::Play { track, .. } = change {
                inner.sent.push(Sent::Played(track));
            }
            if !cancelled {
                inner.waiting.push_back(Waiting { made: now(), change });
            }
            self.save(&inner);
        }
        // (Later, not from inside this call: whoever pushed may hold locks the app's
        // waker takes.)
        let waker = self.waker.clone();
        self.runtime.spawn(async move { waker.changed() });
        self.kick();
    }

    /// How many changes wait to be sent.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.waiting.len() + usize::from(inner.sending.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What stars, ratings and plays wait for (the catalog doesn't have them yet).
    pub fn waiting_marks(&self) -> HashSet<Uuid> {
        let inner = self.inner.lock().unwrap();
        inner.sending.iter().chain(&inner.waiting).filter_map(|w| w.change.marks()).collect()
    }

    /// What happened since last asked.
    pub fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut self.inner.lock().unwrap().sent)
    }

    /// Sends what waits, in order, unless that's already going on. Stops at the first that
    /// doesn't reach the server (call again when it's back).
    pub fn kick(self: &Arc<Self>) {
        // (Offline, it would fail at once: it's sent when the server is back.)
        if self.api.connection().is_offline() {
            return;
        }
        let first = {
            let mut inner = self.inner.lock().unwrap();
            if inner.sending.is_some() {
                return;
            }
            let Some(first) = inner.waiting.pop_front() else { return };
            inner.sending = Some(first.clone());
            first
        };
        let this = self.clone();
        self.runtime.spawn(async move {
            let mut next = Some(first);
            while let Some(waiting) = next {
                let result = this.send(&waiting.change).await;
                let mut inner = this.inner.lock().unwrap();
                inner.sending = None;
                match result {
                    Ok(sent) => inner.sent.extend(sent),
                    // Not there (or the session ended): kept, for the next time.
                    Err(err) if err.status.is_none() || err.status == Some(401) => {
                        inner.waiting.push_front(waiting);
                        this.save(&inner);
                        break;
                    }
                    Err(err) => inner.sent.extend(refused(&waiting.change, &err)),
                }
                next = inner.waiting.pop_front();
                inner.sending = next.clone();
                this.save(&inner);
                drop(inner);
                this.waker.changed();
            }
            this.waker.changed();
        });
    }

    async fn send(&self, change: &Change) -> Result<Vec<Sent>, ApiError> {
        Ok(match change {
            Change::Star { kind, id, name, starred, from } => {
                let changed = self.api.star(kind, *id, *starred).await?;
                let mut sent = vec![Sent::Marked(*id)];
                if changed.was_starred != *from && changed.was_starred != *starred {
                    let what = if changed.was_starred { "starred" } else { "unstarred" };
                    sent.push(Sent::Notice(format!("“{name}” had been {what} elsewhere meanwhile; your change replaced it.")));
                }
                sent
            }
            Change::Rate { kind, id, name, rating, from } => {
                let changed = self.api.rate(kind, *id, *rating).await?;
                let mut sent = vec![Sent::Marked(*id)];
                if changed.was_rating != *from && changed.was_rating != *rating {
                    let what = match changed.was_rating {
                        0 => "unrated".to_string(),
                        r => format!("rated {}", stars(r)),
                    };
                    sent.push(Sent::Notice(format!("“{name}” had been {what} elsewhere meanwhile; your rating replaced it.")));
                }
                sent
            }
            Change::Play { track, at, key } => {
                self.api.played(*track, Some((*at, *key))).await?;
                vec![Sent::Marked(*track)]
            }
            Change::Add { playlist, name, tracks, at } => {
                let body = serde_json::json!({ "tracks": tracks, "at": at });
                self.api.send(Method::POST, &format!("/api/playlists/{playlist}/entries"), Some(body)).await?;
                vec![Sent::Notice(format!("Added {} to “{name}”", count(tracks.len()))), Sent::Playlist(*playlist)]
            }
        })
    }

    /// Writes what waits (best effort: it's in memory all the same).
    fn save(&self, inner: &Inner) {
        let all: Vec<&Waiting> = inner.sending.iter().chain(&inner.waiting).collect();
        let temp = self.file.with_extension("json.part");
        let saved = std::fs::write(&temp, serde_json::to_vec(&all).unwrap_or_default()).and_then(|_| std::fs::rename(&temp, &self.file));
        if let Err(err) = saved {
            eprintln!("couldn't keep the changes waiting to be sent: {err}");
        }
    }
}

/// What a refused change comes to: it's shown as before, and the user is told why.
fn refused(change: &Change, err: &ApiError) -> Vec<Sent> {
    match change {
        Change::Star { id, name, from, .. } => {
            vec![Sent::Unstar { id: *id, starred: *from }, Sent::Notice(format!("Couldn't change the star on “{name}”: {err}"))]
        }
        Change::Rate { id, name, from, .. } => {
            vec![Sent::Unrate { id: *id, rating: *from }, Sent::Notice(format!("Couldn't rate “{name}”: {err}"))]
        }
        Change::Play { .. } => vec![Sent::Notice(format!("Couldn't count a play: {err}"))],
        Change::Add { playlist, name, tracks, .. } => {
            let why = match err.status {
                Some(404) => format!("“{name}” isn't there anymore, so {} couldn't be added to it.", count(tracks.len())),
                Some(403) => format!("“{name}” isn't yours to change anymore, so {} couldn't be added to it.", count(tracks.len())),
                _ => format!("Couldn't add to “{name}”: {err}"),
            };
            vec![Sent::Notice(why), Sent::Playlist(*playlist)]
        }
    }
}

fn count(n: usize) -> String {
    if n == 1 { "1 track".into() } else { format!("{n} tracks") }
}

/// 7 → "3½ stars".
fn stars(rating: i16) -> String {
    let half = if rating % 2 == 1 { "½" } else { "" };
    match rating {
        2 => "1 star".into(),
        1 => "½ star".into(),
        r => format!("{}{half} stars", r / 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server answering every request with `status` and `body`, counting them.
    fn server(status: &'static str, body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut line, mut first, mut length) = (String::new(), String::new(), 0usize);
                while reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                    if first.is_empty() {
                        first = line.trim().to_string();
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let mut sent = vec![0; length];
                let _ = reader.read_exact(&mut sent);
                log.lock().unwrap().push(format!("{first} {}", String::from_utf8_lossy(&sent)));
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (address, asked)
    }

    struct Rig {
        dir: tempfile::TempDir,
        runtime: tokio::runtime::Runtime,
        api: Api,
    }

    fn rig(server: &str) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let session = crate::api::Session { server: server.into(), token: "t".into(), username: "ann".into(), instance: None };
        std::fs::write(dir.path().join("session.json"), serde_json::to_vec(&session).unwrap()).unwrap();
        let api = Api::new(dir.path().to_path_buf());
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        Rig { dir, runtime, api }
    }

    impl Rig {
        fn outbox(&self) -> Arc<Outbox> {
            Outbox::open(self.dir.path().join("outbox-x.json"), self.api.clone(), self.runtime.handle().clone(), Arc::new(crate::NoWake))
        }
    }

    /// Waits until nothing is being sent.
    fn settle(outbox: &Outbox) {
        for _ in 0..200 {
            if outbox.inner.lock().unwrap().sending.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("still sending");
    }

    fn star(id: u128, starred: bool, from: bool) -> Change {
        Change::Star { kind: "tracks".into(), id: Uuid::from_u128(id), name: format!("T{id}"), starred, from }
    }

    fn away() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    #[test]
    fn kept_while_the_server_is_away_and_collapsed() {
        let r = rig(&away());
        let outbox = r.outbox();
        outbox.push(star(1, true, false));
        settle(&outbox);
        // Offline now: the rest waits without trying.
        outbox.push(star(2, true, false));
        outbox.push(Change::Rate { kind: "tracks".into(), id: Uuid::from_u128(1), name: "T1".into(), rating: 6, from: 0 });
        outbox.push(Change::Rate { kind: "tracks".into(), id: Uuid::from_u128(1), name: "T1".into(), rating: 8, from: 6 });
        outbox.push(star(2, false, true));
        assert_eq!(outbox.len(), 2, "the second star of 2 undid the first: nothing to send for it");
        {
            let waiting = &outbox.inner.lock().unwrap().waiting;
            assert_eq!(waiting[0].change, star(1, true, false));
            assert!(matches!(waiting[1].change, Change::Rate { rating: 8, from: 0, .. }), "it changed from what the server had");
        }
        // Kept on disk: a restart has them.
        let again = r.outbox();
        settle(&again);
        assert_eq!(again.len(), 2);
    }

    #[test]
    fn sent_in_order_and_a_clash_is_told() {
        // The server had it starred already (elsewhere) when the change came.
        let (address, asked) = server("200 OK", r#"{"play_count":0,"rating":0,"starred":false,"was_starred":true,"was_rating":0}"#);
        let r = rig(&address);
        let outbox = r.outbox();
        outbox.push(star(1, false, false));
        settle(&outbox);
        outbox.push(Change::Play { track: Uuid::from_u128(2), at: 1_700_000_000, key: Uuid::from_u128(9) });
        settle(&outbox);
        assert!(outbox.is_empty());
        let asked = asked.lock().unwrap().clone();
        assert!(asked[0].starts_with(&format!("DELETE /api/tracks/{}/star", Uuid::from_u128(1))), "{asked:?}");
        assert!(asked[1].contains("\"played_at\":\"2023-11-14T22:13:20Z\"") && asked[1].contains(&Uuid::from_u128(9).to_string()), "{asked:?}");
        let sent = outbox.take();
        assert!(sent.contains(&Sent::Notice("“T1” had been starred elsewhere meanwhile; your change replaced it.".into())), "{sent:?}");
        assert!(sent.contains(&Sent::Played(Uuid::from_u128(2))) && sent.contains(&Sent::Marked(Uuid::from_u128(2))));
    }

    #[test]
    fn a_refused_change_is_dropped_and_undone() {
        let (address, _) = server("404 Not Found", r#"{"code":"not_found","message":"No such playlist."}"#);
        let r = rig(&address);
        let outbox = r.outbox();
        outbox.push(star(1, true, false));
        settle(&outbox);
        outbox.push(Change::Add { playlist: Uuid::from_u128(5), name: "Mix".into(), tracks: vec![Uuid::from_u128(1)], at: None });
        settle(&outbox);
        assert!(outbox.is_empty());
        let sent = outbox.take();
        assert!(sent.contains(&Sent::Unstar { id: Uuid::from_u128(1), starred: false }), "{sent:?}");
        assert!(sent.contains(&Sent::Notice("“Mix” isn't there anymore, so 1 track couldn't be added to it.".into())), "{sent:?}");
    }

    #[test]
    fn half_stars_read_as_people_say_them() {
        assert_eq!((stars(1), stars(2), stars(7), stars(10)), ("½ star".into(), "1 star".into(), "3½ stars".into(), "5 stars".into()));
    }
}
