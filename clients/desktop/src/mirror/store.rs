//! The blobs kept on this machine: `acetate_core::blobs` (by sha256, laid out like a
//! server's volume), with what only the mirror does: taking a local file in as a reflink (a
//! copy sharing the original's data blocks: no space, no time, yet a separate file:
//! changing the original never changes the blob) or, where the filesystem can't, a plain
//! copy (hashed if asked to; else trusted by its path and size, and checked later).

use std::io::Read;
use std::path::Path;

use anyhow::Context;

pub use acetate_core::blobs::{Store as LocalStore, hash_file, hash_file_until, walk_files};

/// How a local file was taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakenIn {
    Reflinked,
    Copied,
}

/// Takes the local file `source` in as `expected`: a reflink if the filesystem can, else a
/// copy. `check`: hashes what was made (not the original: a file changing meanwhile can't
/// give a blob the wrong name) and refuses it if it isn't `expected`. Not synced to disk on
/// Linux: call `sync_written` before recording it.
pub fn take_in(store: &LocalStore, source: &Path, expected: &str, check: bool, stop: &dyn Fn() -> bool) -> anyhow::Result<TakenIn> {
    let temp = store.tmp_dir().join(format!("take-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let how = match reflink_copy::reflink(source, &temp) {
            Ok(()) => TakenIn::Reflinked,
            Err(_) => {
                let _ = std::fs::remove_file(&temp);
                copy_file(source, &temp, stop).with_context(|| format!("copying {}", source.display()))?;
                TakenIn::Copied
            }
        };
        if check {
            let (sha256, _) = hash_file_until(&temp, stop)?.ok_or(super::Stopped)?;
            if sha256 != expected {
                anyhow::bail!("{} isn't the server's file (it hashes to {sha256})", source.display());
            }
        }
        store.commit(&temp, expected, !cfg!(target_os = "linux"))?;
        Ok(how)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Copies `from` to `to` in chunks, stopping between them when `stop` says (then fails).
fn copy_file(from: &Path, to: &Path, stop: &dyn Fn() -> bool) -> anyhow::Result<()> {
    let mut reader = std::fs::File::open(from)?;
    let mut writer = std::fs::File::create(to)?;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if stop() {
            return Err(super::Stopped.into());
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        std::io::Write::write_all(&mut writer, &buf[..n])?;
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

        take_in(&store, &source, HELLO, true, &|| false).unwrap();
        let blob = store.path_of(HELLO).unwrap();
        assert!(blob.ends_with(format!("2c/f2/{HELLO}")));
        assert!(std::fs::metadata(&blob).unwrap().permissions().readonly());

        // Changing the original doesn't change the blob.
        std::fs::write(&source, b"changed").unwrap();
        assert_eq!(std::fs::read(&blob).unwrap(), b"hello");

        // Taken in under the wrong hash: refused, nothing left behind.
        let wrong = "0".repeat(64);
        assert!(take_in(&store, &source, &wrong, true, &|| false).is_err());
        assert!(!store.path_of(&wrong).unwrap().exists());
        assert_eq!(std::fs::read_dir(root.path().join("tmp")).unwrap().count(), 0);

        // Without a hash check: trusted (by its path and size, the caller's look).
        let other = "1".repeat(64);
        take_in(&store, &source, &other, false, &|| false).unwrap();
        assert!(store.path_of(&other).unwrap().exists());
        // Stopped: nothing taken in, nothing left behind.
        let third = "2".repeat(64);
        assert!(take_in(&store, &source, &third, true, &|| true).is_err());
        assert!(!store.path_of(&third).unwrap().exists());
        assert_eq!(std::fs::read_dir(root.path().join("tmp")).unwrap().count(), 0);

        assert_eq!(store.walk().len(), 2);
        store.remove(HELLO).unwrap();
        assert!(!blob.exists());
    }
}
