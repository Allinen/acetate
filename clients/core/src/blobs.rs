//! Files kept on this machine, by their content: `<root>/blobs/ab/cd/abcd…` (the SHA-256,
//! no extension), laid out like a server's volume. Each is made read-only and never
//! changed. The desktop's mirror and the phone's offline downloads are built on this.
//!
//! Everything comes in through `<root>/tmp`, on the same filesystem: a download
//! (`<name>.part`, kept to go on from, hashed as it comes), moved in only when its hash is
//! what it should be.
//!
//! Long work (hashing) is done in chunks and stops between them when `stop` says.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::api::Api;

#[derive(Debug, Clone)]
pub struct Store {
    blobs: PathBuf,
    tmp: PathBuf,
    /// Blobs found not to be what their name says, set aside (never deleted by the app).
    corrupt: PathBuf,
}

/// Why something stopped: asked to (not a problem to show).
#[derive(Debug)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Stopped {}

/// The server doesn't have it (anymore).
#[derive(Debug)]
pub struct NotOnServer;

impl std::fmt::Display for NotOnServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the server doesn't have it")
    }
}

impl std::error::Error for NotOnServer {}

impl Store {
    pub fn open(root: &Path) -> anyhow::Result<Store> {
        let store = Store { blobs: root.join("blobs"), tmp: root.join("tmp"), corrupt: root.join("corrupt") };
        for dir in [&store.blobs, &store.tmp] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(store)
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.blobs
    }

    /// The folder things are put together in before they come in.
    pub fn tmp_dir(&self) -> &Path {
        &self.tmp
    }

    /// Where a blob is (or would be). None for something that isn't a sha256.
    pub fn path_of(&self, sha256: &str) -> Option<PathBuf> {
        is_sha256(sha256).then(|| self.blobs.join(&sha256[0..2]).join(&sha256[2..4]).join(sha256))
    }

    /// Where a blob is, if it's here.
    pub fn find(&self, sha256: &str) -> Option<PathBuf> {
        self.path_of(sha256).filter(|p| p.is_file())
    }

    /// Where a download collects.
    pub fn part_path(&self, name: &str) -> PathBuf {
        self.tmp.join(format!("{name}.part"))
    }

    /// Moves a finished temp file in as `sha256`: read-only, renamed into place; synced to
    /// disk first if `sync` (else the caller calls `sync_written` before recording it). The
    /// caller has checked its hash, or trusts it.
    pub fn commit(&self, temp: &Path, sha256: &str, sync: bool) -> anyhow::Result<PathBuf> {
        let dest = self.path_of(sha256).context("not a sha256")?;
        std::fs::create_dir_all(dest.parent().unwrap())?;
        let file = std::fs::File::open(temp)?;
        if sync {
            file.sync_all()?;
        }
        let mut perms = file.metadata()?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(temp, perms)?;
        std::fs::rename(temp, &dest).with_context(|| format!("moving a blob into {}", dest.display()))?;
        Ok(dest)
    }

    /// Writes everything taken in since the last time to disk at once (Linux: the whole
    /// filesystem, `syncfs`; a sync per file made taking in eight times slower). Elsewhere
    /// each file was synced as it came in.
    pub fn sync_written(&self) -> anyhow::Result<()> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            let dir = std::fs::File::open(&self.blobs)?;
            // SAFETY: a valid file descriptor, open for the call.
            if unsafe { libc::syncfs(dir.as_raw_fd()) } != 0 {
                return Err(std::io::Error::last_os_error()).context("writing the files to disk");
            }
        }
        Ok(())
    }

    /// Sets a blob that isn't what its name says aside (`<root>/corrupt/`), so the right
    /// one can come in. Returns where it went.
    pub fn set_aside(&self, path: &Path) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.corrupt)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let dest = self.corrupt.join(format!("{name}-{}", uuid::Uuid::new_v4()));
        std::fs::rename(path, &dest)?;
        Ok(dest)
    }

    /// Removes a blob (one its user chose to remove).
    pub fn remove(&self, sha256: &str) -> anyhow::Result<()> {
        let path = self.path_of(sha256).context("not a sha256")?;
        match remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }

    /// Temp files left by an earlier run that was stopped, except downloads (`.part`,
    /// gone on from).
    pub fn clear_leftovers(&self) {
        let Ok(items) = std::fs::read_dir(&self.tmp) else { return };
        for item in items.flatten() {
            if !item.file_name().to_string_lossy().ends_with(".part") {
                let _ = remove_file(&item.path());
            }
        }
    }

    /// Every file in the blobs folder: (its name, its path, its size).
    pub fn walk(&self) -> Vec<(String, PathBuf, u64)> {
        let mut out = Vec::new();
        walk_files(
            &self.blobs,
            &mut |path, size| out.push((path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path.to_path_buf(), size)),
            &|| false,
        );
        out
    }
}

/// Removes a file, also a read-only one.
pub fn remove_file(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Ok(meta) = path.metadata() {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    std::fs::remove_file(path)
}

/// Every file below `dir` with its size, recursively. Links aren't followed. Stops early
/// when `stop` says.
pub fn walk_files(dir: &Path, found: &mut dyn FnMut(&Path, u64), stop: &dyn Fn() -> bool) {
    let Ok(items) = std::fs::read_dir(dir) else { return };
    for item in items.flatten() {
        if stop() {
            return;
        }
        let Ok(kind) = item.file_type() else { continue };
        if kind.is_dir() {
            walk_files(&item.path(), found, stop);
        } else if kind.is_file()
            && let Ok(meta) = item.metadata()
        {
            found(&item.path(), meta.len());
        }
    }
}

pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A file's sha256 (hex, as blobs are named) and size. Only reads it.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    Ok(hash_file_until(path, &|| false)?.expect("never stopped"))
}

/// `hash_file`, unless `stop` says to stop (checked between chunks): then None.
pub fn hash_file_until(path: &Path, stop: &dyn Fn() -> bool) -> std::io::Result<Option<(String, u64)>> {
    let mut hasher = Sha256::new();
    let Some(size) = hash_into(&mut hasher, &mut std::fs::File::open(path)?, stop)? else { return Ok(None) };
    Ok(Some((hex::encode(hasher.finalize()), size)))
}

/// Feeds everything `reader` has into `hasher`: how many bytes; None if `stop` said to
/// stop first.
pub fn hash_into(hasher: &mut Sha256, reader: &mut impl Read, stop: &dyn Fn() -> bool) -> std::io::Result<Option<u64>> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0;
    loop {
        if stop() {
            return Ok(None);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(Some(size));
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
}

/// One download from `path` on the server into the store: collected in `tmp/<name>.part`
/// (from where an earlier one stopped), hashed as it comes, moved in under its hash. When
/// the hash should be `expected`, a download that isn't is thrown away. `size`: how big it
/// should be, if known (a part already that big isn't fetched again). `progress` gets the
/// bytes as they come; `stopped` says when to stop (it's kept to go on from). Returns the
/// blob's hash and size.
pub async fn fetch(
    api: &Api,
    store: &Store,
    path: &str,
    name: &str,
    expected: Option<&str>,
    size: Option<u64>,
    progress: &(dyn Fn(u64) + Sync),
    stopped: &(dyn Fn() -> bool + Sync),
) -> anyhow::Result<(String, u64)> {
    let part = store.part_path(name);
    // What an earlier download got: hashed first, then gone on from.
    let earlier = part.clone();
    let (mut hasher, mut got) = tokio::task::spawn_blocking(move || -> std::io::Result<(Sha256, u64)> {
        let mut hasher = Sha256::new();
        let got = match std::fs::File::open(&earlier) {
            Ok(mut file) => hash_into(&mut hasher, &mut file, &|| false)?.unwrap_or(0),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => return Err(err),
        };
        Ok((hasher, got))
    })
    .await??;
    if size.is_some_and(|size| got > size) {
        (hasher, got) = (Sha256::new(), 0);
        let _ = tokio::fs::remove_file(&part).await;
    }
    if Some(got) != size {
        match api.stream_from(path, got).await {
            Ok(mut response) => {
                let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&part).await?;
                if response.status() != reqwest::StatusCode::PARTIAL_CONTENT && got > 0 {
                    // The whole file, not the rest: start over.
                    file.set_len(0).await?;
                    (hasher, got) = (Sha256::new(), 0);
                }
                while let Some(chunk) = response.chunk().await.context("downloading")? {
                    if stopped() {
                        file.flush().await?;
                        return Err(Stopped.into());
                    }
                    hasher.update(&chunk);
                    file.write_all(&chunk).await?;
                    got += chunk.len() as u64;
                    progress(chunk.len() as u64);
                }
                file.flush().await?;
            }
            // Asked from its end: it was all here.
            Err(err) if err.status == Some(416) && got > 0 => {}
            Err(err) if err.status == Some(404) => return Err(NotOnServer.into()),
            Err(err) => return Err(err.into()),
        }
    }
    // Nothing was fetched for an empty file (size 0): it's made here, empty.
    tokio::fs::OpenOptions::new().create(true).append(true).open(&part).await?;
    let hashed = hex::encode(hasher.finalize());
    if let Some(expected) = expected
        && hashed != expected
    {
        let _ = tokio::fs::remove_file(&part).await;
        anyhow::bail!("what came hashes to {hashed}, not {expected}: thrown away (it's fetched again next time)");
    }
    let (store, sha) = (store.clone(), hashed.clone());
    tokio::task::spawn_blocking(move || store.commit(&part, &sha, true)).await??;
    Ok((hashed, got))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn blobs_are_kept_by_hash_read_only() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let temp = store.tmp_dir().join("x.part");
        std::fs::write(&temp, b"hello").unwrap();
        assert_eq!(hash_file(&temp).unwrap(), (HELLO.to_string(), 5));
        assert!(store.find(HELLO).is_none());
        store.commit(&temp, HELLO, true).unwrap();
        let blob = store.find(HELLO).unwrap();
        assert!(blob.ends_with(format!("2c/f2/{HELLO}")));
        assert!(std::fs::metadata(&blob).unwrap().permissions().readonly());
        assert_eq!(store.walk().len(), 1);
        store.remove(HELLO).unwrap();
        assert!(store.find(HELLO).is_none());
        assert!(store.path_of("not a hash").is_none());
    }

    /// A server of one file, `body`, that understands Range, on a thread of its own.
    pub(crate) fn serve(body: &'static [u8]) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut from = 0usize;
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok() {
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(range) = line.to_lowercase().strip_prefix("range: bytes=") {
                        from = range.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                    line.clear();
                }
                let rest = &body[from.min(body.len())..];
                let head = if from > 0 { "206 Partial Content" } else { "200 OK" };
                let _ = write!(stream, "HTTP/1.1 {head}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", rest.len());
                let _ = stream.write_all(rest);
            }
        });
        address
    }

    #[test]
    fn a_download_goes_on_from_its_part_and_only_comes_in_if_it_is_what_it_should_be() {
        let root = tempfile::tempdir().unwrap();
        let server = serve(b"hello");
        std::fs::write(root.path().join("session.json"), serde_json::json!({"server": server, "token": "t", "username": "u"}).to_string()).unwrap();
        let api = Api::new(root.path().to_path_buf());
        let store = Store::open(&root.path().join("kept")).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        // Wrong hash expected: thrown away, nothing comes in.
        let wrong = "0".repeat(64);
        let err = runtime.block_on(fetch(&api, &store, "/f", "one", Some(&wrong), None, &|_| {}, &|| false)).unwrap_err();
        assert!(err.to_string().contains("thrown away"), "{err}");
        assert!(store.walk().is_empty() && !store.part_path("one").exists());

        // A part from an earlier try ("he"): the rest is fetched, and the whole is hashed.
        std::fs::write(store.part_path("two"), b"he").unwrap();
        let got = std::sync::atomic::AtomicU64::new(0);
        let progress = |n: u64| {
            got.fetch_add(n, std::sync::atomic::Ordering::SeqCst);
        };
        let (sha, size) = runtime.block_on(fetch(&api, &store, "/f", "two", Some(HELLO), Some(5), &progress, &|| false)).unwrap();
        assert_eq!((sha.as_str(), size), (HELLO, 5));
        assert_eq!(got.load(std::sync::atomic::Ordering::SeqCst), 3, "only the rest came");
        assert_eq!(std::fs::read(store.find(HELLO).unwrap()).unwrap(), b"hello");

        // No hash expected (a copy the server made): named by its own.
        let (own, _) = runtime.block_on(fetch(&api, &store, "/f", "three", None, None, &|_| {}, &|| false)).unwrap();
        assert_eq!(own, HELLO);

        // Stopped: the part stays to go on from.
        let err = runtime.block_on(fetch(&api, &store, "/f", "four", None, None, &|_| {}, &|| true)).unwrap_err();
        assert!(err.downcast_ref::<Stopped>().is_some());
    }
}
