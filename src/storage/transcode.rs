//! Transcoded copies of audio files, for listeners who aren't allowed the original
//! (`download_original`), for share links that don't give it, and for clients that ask for
//! one (`?transcode=true`): Opus in an Ogg file, with `ffmpeg`.
//!
//! A copy is made the first time it's asked for and kept in the cache folder
//! (`Config::cache_dir`, `transcodes/opus-<kbps>/ab/<sha256>.opus`), one set for each quality the
//! admin allows (`ACETATE_TRANSCODE_KBPS`: a list; a request for any other is refused). The whole file is made
//! before anything is sent, so it's served like any file: with its size and `Range`
//! requests (seeking). A file never changes under its hash, so a copy is good until the
//! folder is pruned: the oldest-used copies go when it grows past the limit
//! (`ACETATE_TRANSCODE_CACHE_GB`), and it can be emptied at any time (they're made again).
//! The bitrate is in the folder's name, so a new one starts a new set; the limit is for all.
//!
//! The original is never touched, and a copy carries nothing of the original's tags,
//! cover or chapters (even a file that's small already gets a copy: its tags are not for
//! those who aren't allowed the original): it's for listening.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use tokio::sync::Semaphore;

/// Longer than this and something is wrong (an hour-long mix takes about half a minute).
const TIME_LIMIT: Duration = Duration::from_secs(15 * 60);
/// The cache is checked against its limit after this many new copies.
const PRUNE_EVERY: u32 = 10;
/// A copy used again is marked as used (for pruning) if it wasn't for this long.
const TOUCH_AFTER: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug)]
pub enum TranscodeError {
    /// `ffmpeg` isn't installed (or couldn't be started).
    NoFfmpeg,
    Failed(anyhow::Error),
}

pub struct TranscodeCache {
    /// `transcodes/`: a folder for each bitrate in it.
    root: PathBuf,
    /// The qualities allowed, the first being the default.
    allowed: Vec<u32>,
    /// The most the folder may hold, in bytes; 0: no limit.
    limit_bytes: u64,
    /// Encoding is CPU work: this many at once (the rest wait).
    working: Semaphore,
    /// Files being made now, so two requests for the same one make it once.
    making: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    made_since_prune: AtomicU32,
}

impl TranscodeCache {
    pub fn new(cache_dir: PathBuf, allowed: Vec<u32>, limit_bytes: u64) -> TranscodeCache {
        assert!(!allowed.is_empty(), "at least one quality");
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        TranscodeCache {
            root: cache_dir.join("transcodes"),
            allowed,
            limit_bytes,
            working: Semaphore::new(cores.div_ceil(2)),
            making: Mutex::new(HashMap::new()),
            made_since_prune: AtomicU32::new(0),
        }
    }

    /// The content type of a copy.
    pub fn mime(&self) -> mime::Mime {
        "audio/ogg".parse().expect("a valid content type")
    }

    /// The qualities that can be asked for (kbps), the default first.
    pub fn allowed(&self) -> &[u32] {
        &self.allowed
    }

    /// The bitrate to make: what was asked for, if that's allowed; the default if nothing
    /// was. The error says what's allowed.
    pub fn pick(&self, asked: Option<u32>) -> Result<u32, String> {
        match asked {
            None => Ok(self.allowed[0]),
            Some(kbps) if self.allowed.contains(&kbps) => Ok(kbps),
            Some(kbps) => {
                let list: Vec<String> = self.allowed.iter().map(|k| k.to_string()).collect();
                Err(format!("this server doesn't make copies at {kbps} kbps: it makes {}", list.join(", ")))
            }
        }
    }

    fn path(&self, sha256: &str, kbps: u32) -> PathBuf {
        self.root.join(format!("opus-{kbps}")).join(&sha256[..2]).join(format!("{sha256}.opus"))
    }

    /// The copy of `original` (the blob with this hash): made now if it isn't there yet.
    /// `kbps` is one of `allowed` (see `pick`).
    pub async fn get(self: &Arc<Self>, original: &Path, sha256: &str, kbps: u32) -> Result<PathBuf, TranscodeError> {
        let path = self.path(sha256, kbps);
        if used(&path).await {
            return Ok(path);
        }
        // The work goes on in a task of its own, so a listener who gives up (skips the
        // track) doesn't waste it: the next request finds the copy.
        let this = self.clone();
        let (original, sha256) = (original.to_path_buf(), sha256.to_string());
        tokio::spawn(async move { this.make_once(&original, &sha256, kbps, &path).await.map(|()| path) })
            .await
            .map_err(|err| TranscodeError::Failed(err.into()))?
    }

    async fn make_once(&self, original: &Path, sha256: &str, kbps: u32, path: &Path) -> Result<(), TranscodeError> {
        let key = format!("{sha256}-{kbps}");
        let lock = self.making.lock().unwrap().entry(key.clone()).or_default().clone();
        let result = async {
            let _one_at_a_time = lock.lock().await;
            // Made meanwhile, by the request that was here first.
            if tokio::fs::try_exists(path).await.unwrap_or(false) {
                return Ok(());
            }
            let _permit = self.working.acquire().await.map_err(|err| TranscodeError::Failed(err.into()))?;
            encode(original, path, kbps).await?;
            if self.limit_bytes > 0 && self.made_since_prune.fetch_add(1, Ordering::Relaxed) + 1 >= PRUNE_EVERY {
                self.made_since_prune.store(0, Ordering::Relaxed);
                let (dir, limit) = (self.root.clone(), self.limit_bytes);
                tokio::task::spawn_blocking(move || prune(&dir, limit));
            }
            Ok(())
        }
        .await;
        let mut making = self.making.lock().unwrap();
        // Last one out clears the entry (this task's `lock` and the map's are the two).
        if making.get(&key).is_some_and(|l| Arc::strong_count(l) <= 2) {
            making.remove(&key);
        }
        result
    }
}

/// Whether the copy is there; if so, marks it as used now (at most once a day: a write
/// for every play isn't worth it).
async fn used(path: &Path) -> bool {
    let Ok(meta) = tokio::fs::metadata(path).await else { return false };
    let stale = meta.modified().ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age > TOUCH_AFTER);
    if stale && let Ok(file) = tokio::fs::File::open(path).await {
        let _ = file.into_std().await.set_modified(SystemTime::now());
    }
    true
}

/// Runs `ffmpeg`: `original` → Opus at `kbps` in `target` (through a temporary file in the
/// same folder, so a half-written copy is never served).
async fn encode(original: &Path, target: &Path, kbps: u32) -> Result<(), TranscodeError> {
    let dir = target.parent().context("the cache folder").map_err(TranscodeError::Failed)?;
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("making {}", dir.display()))
        .map_err(TranscodeError::Failed)?;
    let temp = tempfile::Builder::new()
        .suffix(".part")
        .tempfile_in(dir)
        .context("making a temporary file")
        .map_err(TranscodeError::Failed)?;
    let output = tokio::process::Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(original)
        // The first audio stream only, without tags or pictures: a copy to listen to.
        .args(["-map", "0:a:0", "-vn", "-map_metadata", "-1", "-map_chapters", "-1", "-bitexact"])
        .args(["-c:a", "libopus", "-application", "audio", "-vbr", "on", "-b:a"])
        .arg(format!("{kbps}k"))
        .args(["-f", "ogg"])
        .arg(temp.path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(TIME_LIMIT, output).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => return Err(TranscodeError::NoFfmpeg),
        Ok(Err(err)) => return Err(TranscodeError::Failed(anyhow::Error::new(err).context("running ffmpeg"))),
        Err(_) => return Err(TranscodeError::Failed(anyhow::anyhow!("ffmpeg took more than {} minutes", TIME_LIMIT.as_secs() / 60))),
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said.trim();
        let tail: String = said.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(TranscodeError::Failed(anyhow::anyhow!("ffmpeg failed ({}): {tail}", output.status)));
    }
    temp.persist(target)
        .map_err(|err| TranscodeError::Failed(anyhow::Error::new(err.error).context("saving the copy")))?;
    Ok(())
}

/// Deletes the least recently used copies until the folder is under 90% of `limit` bytes.
/// Blocking.
fn prune(dir: &Path, limit: u64) {
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    // `dir` (all the bitrates) / opus-128 / ab / copies.
    let shards = std::fs::read_dir(dir).into_iter().flatten().flatten().flat_map(|set| std::fs::read_dir(set.path()).into_iter().flatten().flatten());
    for shard in shards {
        let Ok(entries) = std::fs::read_dir(shard.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            // A copy being made (`.part`) isn't counted or touched.
            if path.extension().is_none_or(|e| e != "opus") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), path));
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return;
    }
    files.sort();
    let target = limit / 10 * 9;
    let mut removed = 0;
    for (_, size, path) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            removed += 1;
        }
    }
    tracing::info!("pruned {removed} transcoded copies; {} MB left", total / 1_000_000);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(dir: &Path) -> Arc<TranscodeCache> {
        Arc::new(TranscodeCache::new(dir.to_path_buf(), vec![128, 64], 0))
    }

    #[test]
    fn only_the_listed_qualities_can_be_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        assert_eq!(cache.pick(None), Ok(128), "the first listed is the default");
        assert_eq!(cache.pick(Some(64)), Ok(64));
        let refused = cache.pick(Some(320)).unwrap_err();
        assert!(refused.contains("320") && refused.contains("128, 64"), "{refused}");
    }

    #[test]
    fn pruning_drops_the_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let shard = dir.path().join("opus-128").join("ab");
        std::fs::create_dir_all(&shard).unwrap();
        let now = SystemTime::now();
        for (name, age) in [("old", 300), ("mid", 200), ("new", 100)] {
            let path = shard.join(format!("{name}.opus"));
            std::fs::write(&path, vec![0u8; 1000]).unwrap();
            std::fs::File::open(&path).unwrap().set_modified(now - Duration::from_secs(age)).unwrap();
        }
        std::fs::write(shard.join("half.part"), vec![0u8; 5000]).unwrap();
        prune(dir.path(), 2500);
        assert!(!shard.join("old.opus").exists());
        assert!(shard.join("mid.opus").exists() && shard.join("new.opus").exists());
        assert!(shard.join("half.part").exists());
    }

    /// With ffmpeg installed: a file becomes an Ogg Opus copy, once (the second ask finds it).
    #[tokio::test]
    async fn makes_a_copy_once() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-audio").join("01 - Glass Harbor.flac");
        let dir = tempfile::tempdir().unwrap();
        let cache = cache(dir.path());
        let sha = "ab".repeat(32);
        let path = match cache.get(&source, &sha, 128).await {
            Ok(path) => path,
            Err(TranscodeError::NoFfmpeg) => return, // no ffmpeg here
            Err(TranscodeError::Failed(err)) => panic!("{err:#}"),
        };
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"OggS");
        // Nothing of the original's tags (Glass Harbor's title, artist, cover…).
        let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle));
        assert!(!has(b"Glass Harbor") && !has(b"Mira Vale") && !has(b"METADATA_BLOCK_PICTURE"));
        assert!(bytes.windows(8).any(|w| w == b"OpusHead"));
        let again = cache.get(Path::new("/nonexistent"), &sha, 128).await.ok();
        assert_eq!(again, Some(path), "found, not made again");
    }
}
