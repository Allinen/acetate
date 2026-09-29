//! Playing a file while it downloads.
//!
//! A `RemoteFile` is what the decoder reads (`Read + Seek`): a buffer the size of the
//! file, filled by a download in the background. A read of bytes that haven't arrived
//! waits for them. A seek to a part that isn't there (and isn't coming soon) makes the
//! download start again from there, with a Range request; when it reaches the end, it
//! goes back for the parts it skipped, so in the end the whole file is here (and seeking
//! back is instant).

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;


use crate::api::Api;

/// A seek this far past what the download has reached just waits for it, rather than
/// starting a new request (on a normal connection, a few hundred ms).
const NEAR: u64 = 2 * 1024 * 1024;
/// How long a read waits for bytes before giving up (the connection is gone).
const PATIENCE: Duration = Duration::from_secs(30);

struct State {
    data: Vec<u8>,
    /// The parts downloaded, sorted, not overlapping or touching.
    have: Vec<Range<u64>>,
    /// Where the reader wants bytes from (the download goes there).
    wanted: u64,
    /// The download's current position (where its next bytes go).
    frontier: u64,
    /// Bumped when the download should start again from `wanted`.
    restart: u64,
    error: Option<String>,
    /// How many readers there are (see `Clone`); none left: the download stops.
    readers: usize,
    /// Nobody reads it anymore: the download stops.
    dropped: bool,
}

impl State {
    /// How many bytes from `at` on are here, in one piece.
    fn available(&self, at: u64) -> u64 {
        self.have.iter().find(|r| r.contains(&at)).map(|r| r.end - at).unwrap_or(0)
    }

    fn add(&mut self, range: Range<u64>) {
        self.have.push(range);
        self.have.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<u64>> = Vec::with_capacity(self.have.len());
        for r in self.have.drain(..) {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        self.have = merged;
    }

    /// The first byte at or after `from` that isn't here, wrapping around to the start;
    /// None when the whole file is.
    fn next_missing(&self, from: u64) -> Option<u64> {
        let len = self.data.len() as u64;
        let missing_from = |at: u64| -> Option<u64> {
            let mut at = at;
            for r in &self.have {
                if r.start <= at && at < r.end {
                    at = r.end;
                }
            }
            (at < len).then_some(at)
        };
        missing_from(from.min(len)).or_else(|| missing_from(0))
    }
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when bytes arrive (for waiting readers) or a restart is wanted.
    changed: Condvar,
}

/// A file being downloaded, readable (and seekable) as if it were all here.
pub struct RemoteFile {
    shared: Arc<Shared>,
    position: u64,
}

impl RemoteFile {
    /// Starts downloading, and returns once the file's size is known.
    /// `path`: where it's fetched from (see `QueueItem::stream_path`).
    pub async fn open(api: &Api, path: String) -> anyhow::Result<RemoteFile> {
        let response = api.stream_from(&path, 0).await?;
        let len = total_length(&response).ok_or_else(|| anyhow::anyhow!("The server didn't say how big the file is, so it can't be played while it downloads."))?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                data: vec![0; len as usize],
                have: Vec::new(),
                wanted: 0,
                frontier: 0,
                restart: 0,
                error: None,
                readers: 1,
                dropped: false,
            }),
            changed: Condvar::new(),
        });
        tokio::spawn(download(api.clone(), path, shared.clone(), response));
        Ok(RemoteFile { shared, position: 0 })
    }

    pub fn len(&self) -> u64 {
        self.shared.state.lock().unwrap().data.len() as u64
    }
}

/// Another reader of the same download, from the start: what's downloaded is shared, so
/// playing a file that's already (being) downloaded doesn't fetch it again.
impl Clone for RemoteFile {
    fn clone(&self) -> RemoteFile {
        self.shared.state.lock().unwrap().readers += 1;
        RemoteFile { shared: self.shared.clone(), position: 0 }
    }
}

impl Drop for RemoteFile {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.readers -= 1;
        if state.readers == 0 {
            state.dropped = true;
            self.shared.changed.notify_all();
        }
    }
}

impl Read for RemoteFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut state = self.shared.state.lock().unwrap();
        let len = state.data.len() as u64;
        if self.position >= len || buf.is_empty() {
            return Ok(0);
        }
        // Wait until the bytes at our position are here.
        loop {
            let available = state.available(self.position);
            if available > 0 {
                let n = (available as usize).min(buf.len());
                let at = self.position as usize;
                buf[..n].copy_from_slice(&state.data[at..at + n]);
                self.position += n as u64;
                return Ok(n);
            }
            if let Some(err) = &state.error {
                return Err(io::Error::other(err.clone()));
            }
            self.want(&mut state);
            let (next, timeout) = self.shared.changed.wait_timeout(state, PATIENCE).unwrap();
            state = next;
            if timeout.timed_out() && state.available(self.position) == 0 {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "The download stopped: no data for 30 seconds."));
            }
        }
    }
}

impl RemoteFile {
    /// Tells the download where we read, restarting it there if it isn't coming soon.
    fn want(&self, state: &mut State) {
        let at = self.position;
        state.wanted = at;
        let coming_soon = state.frontier <= at && at < state.frontier + NEAR;
        if !coming_soon && state.available(at) == 0 {
            state.restart += 1;
            self.shared.changed.notify_all();
        }
    }
}

impl Seek for RemoteFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let len = self.len() as i64;
        let position = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => len + n,
            SeekFrom::Current(n) => self.position as i64 + n,
        };
        if position < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.position = position as u64;
        let mut state = self.shared.state.lock().unwrap();
        if self.position < len as u64 && state.available(self.position) == 0 {
            self.want(&mut state);
        }
        Ok(self.position)
    }
}

/// For reading with symphonia directly (Opus files, see `opus`).
impl symphonia::core::io::MediaSource for RemoteFile {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len())
    }
}

/// The download: from where it's wanted to the end, then the parts skipped, until the
/// whole file is here. `first` is the response `open` already has (from byte 0).
async fn download(api: Api, path: String, shared: Arc<Shared>, first: reqwest::Response) {
    let mut response = Some(first);
    let mut from = 0u64;
    loop {
        let restart = shared.state.lock().unwrap().restart;
        let mut current = match response.take() {
            Some(r) => r,
            None => match api.stream_from(&path, from).await {
                Ok(r) => r,
                Err(err) => {
                    shared.state.lock().unwrap().error = Some(format!("{err:#}"));
                    shared.changed.notify_all();
                    return;
                }
            },
        };
        let mut at = from;
        shared.state.lock().unwrap().frontier = at;
        loop {
            let chunk = match current.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(err) => {
                    shared.state.lock().unwrap().error = Some(format!("{err:#}"));
                    shared.changed.notify_all();
                    return;
                }
            };
            let mut state = shared.state.lock().unwrap();
            if state.dropped {
                return;
            }
            let end = (at + chunk.len() as u64).min(state.data.len() as u64);
            let n = (end - at) as usize;
            state.data[at as usize..end as usize].copy_from_slice(&chunk[..n]);
            state.add(at..end);
            at = end;
            state.frontier = at;
            shared.changed.notify_all();
            // Stop here if the reader wants to be somewhere else now, or this part was
            // already here (we reached what an earlier request downloaded).
            if state.restart != restart || (state.available(at) > 0 && at < state.data.len() as u64) {
                break;
            }
        }
        let state = shared.state.lock().unwrap();
        if state.dropped {
            return;
        }
        match state.next_missing(state.wanted) {
            Some(next) => from = next,
            None => return,
        }
    }
}

/// The file's full size, from `Content-Range: bytes a-b/total` (a 206) or
/// `Content-Length` (a 200, the whole file).
fn total_length(response: &reqwest::Response) -> Option<u64> {
    let headers = response.headers();
    if let Some(range) = headers.get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
        return range.rsplit('/').next()?.parse().ok();
    }
    response.content_length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(len: usize, have: &[Range<u64>]) -> State {
        let mut s = State { data: vec![0; len], have: Vec::new(), wanted: 0, frontier: 0, restart: 0, error: None, readers: 1, dropped: false };
        for r in have {
            s.add(r.clone());
        }
        s
    }

    #[test]
    fn parts_merge_and_the_missing_ones_are_found_in_order() {
        let s = state(100, &[0..10, 10..20, 50..60, 55..70]);
        assert_eq!(s.have, [0..20, 50..70]);
        assert_eq!(s.available(5), 15);
        assert_eq!(s.available(20), 0);
        assert_eq!(s.next_missing(0), Some(20));
        assert_eq!(s.next_missing(60), Some(70));
        // From the end, it wraps around to what was skipped.
        let s = state(100, &[0..20, 50..100]);
        assert_eq!(s.next_missing(60), Some(20));
        assert_eq!(state(100, &[0..100]).next_missing(0), None);
    }
}

/// Against a real server: `ACETATE_TEST_SERVER=http://… ACETATE_TEST_TOKEN=… ACETATE_TEST_FILE=<file id>
/// cargo test remote_file -- --ignored`.
#[cfg(test)]
mod against_a_server {
    use super::*;

    #[test]
    #[ignore]
    fn remote_file_reads_and_seeks_like_the_whole_file() {
        let (Ok(server), Ok(token), Ok(file)) =
            (std::env::var("ACETATE_TEST_SERVER"), std::env::var("ACETATE_TEST_TOKEN"), std::env::var("ACETATE_TEST_FILE"))
        else {
            return;
        };
        let dir = tempfile_dir();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let file: uuid::Uuid = file.parse().unwrap();
        let whole = runtime.block_on(async {
            api.file_from(file, 0).await.unwrap().bytes().await.unwrap().to_vec()
        });
        let mut remote = runtime.block_on(RemoteFile::open(&api, format!("/api/files/{file}/stream"))).unwrap();
        assert_eq!(remote.len(), whole.len() as u64);
        // Near the end first (before the download gets there), then from the start.
        let near_end = whole.len() as u64 - 1000;
        remote.seek(SeekFrom::Start(near_end)).unwrap();
        let mut tail = Vec::new();
        // Reads block, so off the runtime's threads (as the audio thread does).
        let (tail, mut remote) = std::thread::spawn(move || {
            remote.read_to_end(&mut tail).unwrap();
            (tail, remote)
        })
        .join()
        .unwrap();
        assert_eq!(tail, whole[near_end as usize..]);
        remote.seek(SeekFrom::Start(0)).unwrap();
        // A second reader of the same download (as a kept file is played again).
        let mut second = remote.clone();
        let all = std::thread::spawn(move || {
            let mut all = Vec::new();
            remote.read_to_end(&mut all).unwrap();
            all
        })
        .join()
        .unwrap();
        assert_eq!(all.len(), whole.len());
        assert!(all == whole, "the same bytes");
        // The first reader is gone; the second still reads, from the middle, at once
        // (the bytes are here already).
        let middle = whole.len() as u64 / 2;
        second.seek(SeekFrom::Start(middle)).unwrap();
        let rest = std::thread::spawn(move || {
            let mut rest = Vec::new();
            second.read_to_end(&mut rest).unwrap();
            rest
        })
        .join()
        .unwrap();
        assert!(rest == whole[middle as usize..], "the second reader has the same bytes");
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("acetate-test-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
