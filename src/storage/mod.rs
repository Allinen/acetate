//! Content-addressed blob store, on one or more volumes (disks, see `Volume`).
//!
//! Every blob (audio file or cover image) is stored under its sha256:
//! `<volume>/blobs/ab/cd/abcd1234...` with no extension. Blobs are written once,
//! made read-only, and never modified afterwards.
//!
//! Writing works in two steps so that we know the hash before deciding what to do:
//! 1. `BlobStore::writer()` gives a `BlobWriter` that streams into a temp file
//!    (in `data/tmp`) and hashes as it goes. `finish()` returns a `PendingBlob`.
//! 2. `BlobStore::commit(pending, volume)` moves it into place. Dropping a `PendingBlob`
//!    instead deletes the temp file (used when the file turns out to be a duplicate).

pub mod images;

use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

/// One place blobs are stored: a folder on some disk, e.g. a fast SSD for most music and a
/// big HDD for the rest. Its name is what the database records (`blobs.volume`); where it
/// is on disk is each machine's own configuration (`ACETATE_VOLUMES`), so a database moved
/// to another machine only needs that machine's paths.
#[derive(Debug, Clone)]
pub struct Volume {
    pub name: String,
    blobs_dir: PathBuf,
    /// Temp files for this volume: on the same filesystem, so moving a finished blob into
    /// place is an atomic rename (and reflinks work).
    tmp_dir: PathBuf,
}

/// The blob store: one or more volumes. The first is the default (covers always go there,
/// and files unless an admin picks another volume). A blob is on one volume; reading finds
/// it by looking in each (a quick "does this file exist", fast disks first as configured).
#[derive(Debug, Clone)]
pub struct BlobStore {
    volumes: Vec<Volume>,
}

impl BlobStore {
    /// A store with one volume, `main`, inside `data_dir` (`blobs/` and `tmp/`). For tests;
    /// the server and CLI use `open_volumes` with the configured volumes.
    #[cfg(test)]
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        Self::open_volumes(&[("main".to_string(), data_dir.to_path_buf())])
    }

    /// A store with these volumes (name, folder), the first being the default. Creates
    /// their `blobs/` and `tmp/` folders if needed.
    pub fn open_volumes(volumes: &[(String, PathBuf)]) -> anyhow::Result<Self> {
        if volumes.is_empty() {
            anyhow::bail!("no storage volumes configured");
        }
        let mut opened = Vec::new();
        for (name, dir) in volumes {
            let blobs_dir = dir.join("blobs");
            let tmp_dir = dir.join("tmp");
            std::fs::create_dir_all(&blobs_dir)
                .with_context(|| format!("creating {}", blobs_dir.display()))?;
            std::fs::create_dir_all(&tmp_dir)
                .with_context(|| format!("creating {}", tmp_dir.display()))?;
            opened.push(Volume {
                name: name.clone(),
                blobs_dir,
                tmp_dir,
            });
        }
        Ok(BlobStore { volumes: opened })
    }

    /// The default volume's name.
    pub fn default_volume(&self) -> &str {
        &self.volumes[0].name
    }

    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    /// Whether a volume with this name is configured.
    pub fn has_volume(&self, name: &str) -> bool {
        self.volumes.iter().any(|v| v.name == name)
    }

    fn volume(&self, name: &str) -> anyhow::Result<&Volume> {
        self.volumes.iter().find(|v| v.name == name).with_context(|| {
            let names: Vec<&str> = self.volumes.iter().map(|v| v.name.as_str()).collect();
            format!("no storage volume called \"{name}\" (configured: {})", names.join(", "))
        })
    }

    /// Where a blob is: the volume it's on and its path. None if it's on none of them.
    pub async fn find(&self, sha256: &str) -> Option<(String, PathBuf)> {
        let rel = blob_relative_path(sha256)?;
        for volume in &self.volumes {
            let path = volume.blobs_dir.join(&rel);
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Some((volume.name.clone(), path));
            }
        }
        None
    }

    /// Where to read a blob from: where it is, or (if it's nowhere) where it would be on the
    /// default volume, so reading it fails with "not found".
    pub async fn path_for(&self, sha256: &str) -> PathBuf {
        match self.find(sha256).await {
            Some((_, path)) => path,
            None => self.volumes[0].path_of(sha256),
        }
    }

    /// Starts writing a new blob into a temp file (on the default volume).
    pub async fn writer(&self) -> anyhow::Result<BlobWriter> {
        let temp_path = tempfile::Builder::new()
            .prefix("upload-")
            .tempfile_in(&self.volumes[0].tmp_dir)?
            .into_temp_path();
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&temp_path)
            .await?;
        Ok(BlobWriter {
            file,
            temp_path,
            hasher: Sha256::new(),
            size: 0,
        })
    }

    /// Moves a finished blob into place on `volume`. If a blob with the same hash is already
    /// stored (on any volume), that one is kept and the temp file is just deleted. Returns
    /// the volume it's on and its path.
    pub async fn commit(&self, pending: PendingBlob, volume: &str) -> anyhow::Result<(String, PathBuf)> {
        if let Some(existing) = self.find(&pending.sha256).await {
            // Identical content is already stored; dropping `pending` removes the temp file.
            return Ok(existing);
        }
        let target = self.volume(volume)?;
        let dest = target.path_of(&pending.sha256);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Make the file read-only before it becomes visible under its final name.
        let mut perms = tokio::fs::metadata(&pending.temp_path).await?.permissions();
        perms.set_readonly(true);
        tokio::fs::set_permissions(&pending.temp_path, perms).await?;

        // `persist` is a rename, and it also stops TempPath from deleting the file on drop.
        // A rename can't cross filesystems: then the file is copied into the volume's own
        // temp folder first, and renamed from there.
        match pending.temp_path.persist(&dest) {
            Ok(()) => {}
            Err(err) if err.error.kind() == std::io::ErrorKind::CrossesDevices => {
                let temp_path = err.path;
                let copy = target.tmp_dir.join(format!("copy-{}", uuid::Uuid::now_v7()));
                tokio::fs::copy(&temp_path, &copy).await?;
                let copy = TempPath::try_from_path(&copy)?;
                tokio::fs::File::open(&copy).await?.sync_all().await?;
                copy.persist(&dest)
                    .map_err(|e| e.error)
                    .with_context(|| format!("moving blob into {}", dest.display()))?;
                drop(temp_path); // deletes the original temp file
            }
            Err(err) => {
                return Err(err.error).with_context(|| format!("moving blob into {}", dest.display()));
            }
        }
        Ok((target.name.clone(), dest))
    }

    /// Deletes a blob (from every volume it's on). Only purging does this (see `db::trash`),
    /// after making sure nothing uses it anymore; otherwise blobs are never touched once
    /// written. Returns false if it was already gone.
    pub async fn delete(&self, sha256: &str) -> anyhow::Result<bool> {
        let Some(rel) = blob_relative_path(sha256) else {
            anyhow::bail!("not a sha256: {sha256}");
        };
        let mut deleted = false;
        for volume in &self.volumes {
            match tokio::fs::remove_file(volume.blobs_dir.join(&rel)).await {
                // Removing a read-only file is fine: that depends on the directory's permissions.
                Ok(()) => deleted = true,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err).with_context(|| format!("deleting blob {sha256}")),
            }
        }
        Ok(deleted)
    }

    /// A blob's size in bytes, or None if it isn't there.
    pub async fn size(&self, sha256: &str) -> Option<u64> {
        let (_, path) = self.find(sha256).await?;
        tokio::fs::metadata(path).await.ok().map(|m| m.len())
    }

    /// Takes a file on the same machine into `volume`'s temp folder as a *reflink*: a copy
    /// that shares the original's data blocks on disk (btrfs, XFS), so it takes no time and
    /// almost no space, yet is a separate file. Changing or deleting the original later
    /// never affects it (unlike a hard link, which would be the same file). Then hashes
    /// that copy (reading it, not the original, so a file changing underneath can't give a
    /// blob the wrong name).
    ///
    /// None if it can't reflink (the volume is on another filesystem, or one that can't,
    /// like ext4): then the caller copies the bytes instead.
    pub async fn reflink_in(&self, source: &Path, volume: &str) -> anyhow::Result<Option<PendingBlob>> {
        let source = source.to_path_buf();
        let tmp_dir = self.volume(volume)?.tmp_dir.clone();
        // Blocking file work, on tokio's thread pool for that.
        tokio::task::spawn_blocking(move || -> anyhow::Result<Option<PendingBlob>> {
            let dest = tmp_dir.join(format!("reflink-{}", uuid::Uuid::now_v7()));
            if let Err(err) = reflink_copy::reflink(&source, &dest) {
                tracing::debug!("can't reflink {}: {err}", source.display());
                let _ = std::fs::remove_file(&dest);
                return Ok(None);
            }
            // From here on the temp file is deleted if anything fails (or when dropped).
            let temp_path = TempPath::try_from_path(&dest)?;
            let (sha256, size) = hash_file(&temp_path)?;
            // Make sure it's on disk before the database records it.
            std::fs::File::open(&temp_path)?.sync_all()?;
            Ok(Some(PendingBlob { sha256, size, temp_path }))
        })
        .await?
    }

    /// Convenience for small in-memory blobs (cover art), on the default volume. Returns
    /// the sha256.
    pub async fn put_bytes(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let mut writer = self.writer().await?;
        writer.write(bytes).await?;
        let pending = writer.finish().await?;
        let sha256 = pending.sha256.clone();
        self.commit(pending, self.default_volume()).await?;
        Ok(sha256)
    }
}

impl Volume {
    /// The folder holding this volume's blobs.
    pub fn blobs_dir(&self) -> &Path {
        &self.blobs_dir
    }

    /// Where a blob would be on this volume.
    fn path_of(&self, sha256: &str) -> PathBuf {
        blob_relative_path(sha256)
            .map(|rel| self.blobs_dir.join(rel))
            .unwrap_or_else(|| self.blobs_dir.join("invalid"))
    }
}

/// The sha256 of a file on disk (as blobs are named) and its size. Blocking: call it from
/// `spawn_blocking`. Only reads the file.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size))
}

/// The sha256 of some bytes, as stored blobs are named. For a dry run, which works out
/// what would be stored without storing it.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A blob being written. Feed it chunks with `write`, then call `finish`.
pub struct BlobWriter {
    file: tokio::fs::File,
    temp_path: TempPath,
    hasher: Sha256,
    size: u64,
}

impl BlobWriter {
    /// Bytes written so far.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub async fn write(&mut self, chunk: &[u8]) -> anyhow::Result<()> {
        self.hasher.update(chunk);
        self.file.write_all(chunk).await?;
        self.size += chunk.len() as u64;
        Ok(())
    }

    pub async fn finish(mut self) -> anyhow::Result<PendingBlob> {
        self.file.flush().await?;
        // Make sure the data is on disk before we record it in the database.
        self.file.sync_all().await?;
        Ok(PendingBlob {
            sha256: hex::encode(self.hasher.finalize()),
            size: self.size,
            temp_path: self.temp_path,
        })
    }
}

/// A fully written and hashed blob that hasn't been moved into the store yet.
/// Dropping it deletes the temp file.
pub struct PendingBlob {
    pub sha256: String,
    pub size: u64,
    temp_path: TempPath,
}

impl PendingBlob {
    /// Path of the temp file, for reading it (probing, tags) before committing.
    pub fn path(&self) -> &Path {
        &self.temp_path
    }
}

/// `abcd1234...` -> `ab/cd/abcd1234...`. Returns None if the hash isn't 64 lowercase hex chars.
fn blob_relative_path(sha256: &str) -> Option<PathBuf> {
    let valid = sha256.len() == 64
        && sha256
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    if !valid {
        return None;
    }
    Some(PathBuf::from(&sha256[0..2]).join(&sha256[2..4]).join(sha256))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shards_by_first_two_bytes() {
        let hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(
            blob_relative_path(hash).unwrap(),
            PathBuf::from("ab").join("cd").join(hash)
        );
    }

    #[test]
    fn rejects_bad_hashes() {
        assert!(blob_relative_path("abc").is_none());
        assert!(blob_relative_path(&"G".repeat(64)).is_none());
        assert!(blob_relative_path(&"A".repeat(64)).is_none());
        // Path traversal attempts can't sneak through.
        assert!(blob_relative_path(&format!("../{}", "a".repeat(61))).is_none());
    }

    #[tokio::test]
    async fn write_commit_and_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();

        let sha = store.put_bytes(b"hello").await.unwrap();
        assert_eq!(
            sha,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let path = store.path_for(&sha).await;
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());

        // Same content again: same hash, no error, temp dir left empty.
        let sha2 = store.put_bytes(b"hello").await.unwrap();
        assert_eq!(sha, sha2);
        assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn blobs_go_to_the_chosen_volume_and_stay_there() {
        let (ssd, hdd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let store = BlobStore::open_volumes(&[
            ("main".into(), ssd.path().to_path_buf()),
            ("archive".into(), hdd.path().to_path_buf()),
        ])
        .unwrap();
        assert_eq!(store.default_volume(), "main");

        let pending = |bytes: &'static [u8]| {
            let store = store.clone();
            async move {
                let mut w = store.writer().await.unwrap();
                w.write(bytes).await.unwrap();
                w.finish().await.unwrap()
            }
        };
        let (volume, path) = store.commit(pending(b"song").await, "archive").await.unwrap();
        assert_eq!(volume, "archive");
        assert!(path.starts_with(hdd.path()));

        // The same bytes again, asked for the main volume: it stays where it is.
        let sha = crate::storage::sha256_hex(b"song");
        let (volume, _) = store.commit(pending(b"song").await, "main").await.unwrap();
        assert_eq!(volume, "archive");
        assert_eq!(store.find(&sha).await.unwrap().0, "archive");
        assert_eq!(store.size(&sha).await, Some(4));

        assert!(store.commit(pending(b"x").await, "nope").await.is_err());
        assert!(store.delete(&sha).await.unwrap());
        assert!(store.find(&sha).await.is_none());
    }
}
