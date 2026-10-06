//! The blobs kept on this machine: `<root>/blobs/ab/cd/abcd…` (by sha256, no extension),
//! laid out like a server's volume. Each is made read-only and never changed.
//!
//! Everything comes in through `<root>/tmp`, on the same filesystem, and is hashed there:
//! a download (`<sha256>.part`, kept to go on from), or a local file taken in as a reflink
//! (a copy sharing the original's data blocks: no space, no time, yet a separate file:
//! changing the original never changes the blob) or, where the filesystem can't, a plain
//! copy. Only bytes whose hash is their name are moved in.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct LocalStore {
    blobs: PathBuf,
    tmp: PathBuf,
    /// Blobs found not to be what their name says, set aside (never deleted by the app).
    corrupt: PathBuf,
}

/// How a local file was taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakenIn {
    Reflinked,
    Copied,
}

impl LocalStore {
    pub fn open(root: &Path) -> anyhow::Result<LocalStore> {
        let store = LocalStore { blobs: root.join("blobs"), tmp: root.join("tmp"), corrupt: root.join("corrupt") };
        for dir in [&store.blobs, &store.tmp] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(store)
    }

    pub fn blobs_dir(&self) -> &Path {
        &self.blobs
    }

    /// Where a blob is (or would be). None for something that isn't a sha256.
    pub fn path_of(&self, sha256: &str) -> Option<PathBuf> {
        is_sha256(sha256).then(|| self.blobs.join(&sha256[0..2]).join(&sha256[2..4]).join(sha256))
    }

    /// Where a download of `sha256` collects.
    pub fn part_path(&self, sha256: &str) -> PathBuf {
        self.tmp.join(format!("{sha256}.part"))
    }

    /// Moves a finished temp file in as `sha256`: read-only, synced, renamed into place.
    /// The caller has checked its hash.
    pub fn commit(&self, temp: &Path, sha256: &str) -> anyhow::Result<PathBuf> {
        let dest = self.path_of(sha256).context("not a sha256")?;
        std::fs::create_dir_all(dest.parent().unwrap())?;
        let file = std::fs::File::open(temp)?;
        file.sync_all()?;
        let mut perms = file.metadata()?.permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(temp, perms)?;
        std::fs::rename(temp, &dest).with_context(|| format!("moving a blob into {}", dest.display()))?;
        Ok(dest)
    }

    /// Takes the local file `source` in as `expected` (its hash, worked out already): a
    /// reflink if the filesystem can, else a copy. Hashes what was made (not the original:
    /// a file changing meanwhile can't give a blob the wrong name) before it goes in.
    pub fn take_in(&self, source: &Path, expected: &str) -> anyhow::Result<TakenIn> {
        let temp = self.tmp.join(format!("take-{}", uuid::Uuid::new_v4()));
        let how = match reflink_copy::reflink(source, &temp) {
            Ok(()) => TakenIn::Reflinked,
            Err(_) => {
                let _ = std::fs::remove_file(&temp);
                std::fs::copy(source, &temp).with_context(|| format!("copying {}", source.display()))?;
                TakenIn::Copied
            }
        };
        let result = (|| {
            let (sha256, _) = hash_file(&temp)?;
            if sha256 != expected {
                anyhow::bail!("{} changed while it was taken in", source.display());
            }
            self.commit(&temp, expected)?;
            Ok(how)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
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
        match crate::root::remove_file(&path) {
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
                let _ = crate::root::remove_file(&item.path());
            }
        }
    }

    /// Every file in the blobs folder: (its name, its path, its size).
    pub fn walk(&self) -> Vec<(String, PathBuf, u64)> {
        let mut out = Vec::new();
        walk_files(&self.blobs, &mut |path, size| {
            out.push((path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path.to_path_buf(), size))
        });
        out
    }
}

/// Every file below `dir` with its size, recursively. Links aren't followed.
pub fn walk_files(dir: &Path, found: &mut dyn FnMut(&Path, u64)) {
    let Ok(items) = std::fs::read_dir(dir) else { return };
    for item in items.flatten() {
        let Ok(kind) = item.file_type() else { continue };
        if kind.is_dir() {
            walk_files(&item.path(), found);
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
    let mut hasher = Sha256::new();
    let size = hash_into(&mut hasher, &mut std::fs::File::open(path)?)?;
    Ok((hex::encode(hasher.finalize()), size))
}

/// Feeds everything `reader` has into `hasher`: how many bytes.
pub fn hash_into(hasher: &mut Sha256, reader: &mut impl Read) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = 0;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(size);
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn a_file_taken_in_is_a_separate_read_only_blob() {
        let (root, outside) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let store = LocalStore::open(root.path()).unwrap();
        let source = outside.path().join("song.flac");
        std::fs::write(&source, b"hello").unwrap();
        assert_eq!(hash_file(&source).unwrap(), (HELLO.to_string(), 5));

        store.take_in(&source, HELLO).unwrap();
        let blob = store.path_of(HELLO).unwrap();
        assert!(blob.ends_with(format!("2c/f2/{HELLO}")));
        assert!(std::fs::metadata(&blob).unwrap().permissions().readonly());

        // Changing the original doesn't change the blob.
        std::fs::write(&source, b"changed").unwrap();
        assert_eq!(std::fs::read(&blob).unwrap(), b"hello");

        // Taken in under the wrong hash: refused, nothing left behind.
        let wrong = "0".repeat(64);
        assert!(store.take_in(&source, &wrong).is_err());
        assert!(!store.path_of(&wrong).unwrap().exists());
        assert_eq!(std::fs::read_dir(root.path().join("tmp")).unwrap().count(), 0);

        assert_eq!(store.walk().len(), 1);
        store.remove(HELLO).unwrap();
        assert!(!blob.exists());
    }
}
