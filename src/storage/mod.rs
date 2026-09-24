//! Content-addressed blob store.
//!
//! Every blob (audio file or cover image) is stored under its sha256:
//! `data/blobs/ab/cd/abcd1234...` with no extension. Blobs are written once,
//! made read-only, and never modified afterwards.
//!
//! Writing works in two steps so that we know the hash before deciding what to do:
//! 1. `BlobStore::writer()` gives a `BlobWriter` that streams into a temp file
//!    (in `data/tmp`) and hashes as it goes. `finish()` returns a `PendingBlob`.
//! 2. `BlobStore::commit(pending)` moves it into place. Dropping a `PendingBlob`
//!    instead deletes the temp file (used when the file turns out to be a duplicate).

use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct BlobStore {
    blobs_dir: PathBuf,
    tmp_dir: PathBuf,
}

impl BlobStore {
    /// Opens (and creates, if needed) the blob store inside `data_dir`.
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let blobs_dir = data_dir.join("blobs");
        // tmp must be on the same filesystem as blobs so that the final move is
        // an atomic rename, which is why it lives in the data dir and not in /tmp.
        let tmp_dir = data_dir.join("tmp");
        std::fs::create_dir_all(&blobs_dir)
            .with_context(|| format!("creating {}", blobs_dir.display()))?;
        std::fs::create_dir_all(&tmp_dir)
            .with_context(|| format!("creating {}", tmp_dir.display()))?;
        Ok(BlobStore { blobs_dir, tmp_dir })
    }

    /// Where the blob with this hash lives (whether or not it exists yet).
    pub fn path_for(&self, sha256: &str) -> PathBuf {
        blob_relative_path(sha256)
            .map(|rel| self.blobs_dir.join(rel))
            .unwrap_or_else(|| self.blobs_dir.join("invalid"))
    }

    /// Starts writing a new blob into a temp file.
    pub async fn writer(&self) -> anyhow::Result<BlobWriter> {
        let temp_path = tempfile::Builder::new()
            .prefix("upload-")
            .tempfile_in(&self.tmp_dir)?
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

    /// Moves a finished blob to its final content-addressed location.
    /// If a blob with the same hash already exists, the temp file is just deleted.
    pub async fn commit(&self, pending: PendingBlob) -> anyhow::Result<PathBuf> {
        let dest = self.path_for(&pending.sha256);
        if tokio::fs::try_exists(&dest).await? {
            // Identical content is already stored; dropping `pending` removes the temp file.
            return Ok(dest);
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Make the file read-only before it becomes visible under its final name.
        let mut perms = tokio::fs::metadata(&pending.temp_path).await?.permissions();
        perms.set_readonly(true);
        tokio::fs::set_permissions(&pending.temp_path, perms).await?;

        // `persist` is a rename, and it also stops TempPath from deleting the file on drop.
        pending
            .temp_path
            .persist(&dest)
            .with_context(|| format!("moving blob into {}", dest.display()))?;
        Ok(dest)
    }

    /// Convenience for small in-memory blobs (e.g. embedded cover art). Returns the sha256.
    pub async fn put_bytes(&self, bytes: &[u8]) -> anyhow::Result<String> {
        let mut writer = self.writer().await?;
        writer.write(bytes).await?;
        let pending = writer.finish().await?;
        let sha256 = pending.sha256.clone();
        self.commit(pending).await?;
        Ok(sha256)
    }
}

/// A blob being written. Feed it chunks with `write`, then call `finish`.
pub struct BlobWriter {
    file: tokio::fs::File,
    temp_path: TempPath,
    hasher: Sha256,
    size: u64,
}

impl BlobWriter {
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
        let path = store.path_for(&sha);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert!(std::fs::metadata(&path).unwrap().permissions().readonly());

        // Same content again: same hash, no error, temp dir left empty.
        let sha2 = store.put_bytes(b"hello").await.unwrap();
        assert_eq!(sha, sha2);
        assert_eq!(std::fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    }
}
