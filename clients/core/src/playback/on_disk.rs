//! While offline, only what's on this device can play: the hashes of the files here
//! (downloads, and on the desktop the mirror), for the player to skip the rest and the
//! lists to dim it. Online it's empty (None) and everything counts as playable; it's
//! worked out when the server goes away (the files here don't change while it's gone:
//! nothing can be fetched), and dropped when it's back.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, RwLock};

use super::{AudioSource, QueueItem};

/// A file's hash (SHA-256).
pub type Hash = [u8; 32];

/// Shared by whoever plays and whoever draws lists (clone it freely).
#[derive(Clone, Default)]
pub struct OnDisk(Arc<RwLock<Option<Arc<HashSet<Hash>>>>>);

impl OnDisk {
    /// What's here (None: online, or not worked out yet: everything counts as playable).
    pub fn set(&self, hashes: Option<HashSet<Hash>>) {
        *self.0.write().unwrap() = hashes.map(Arc::new);
    }

    /// The hashes, if playing is limited to them. For a list's rows, read once per frame.
    pub fn get(&self) -> Option<Arc<HashSet<Hash>>> {
        self.0.read().unwrap().clone()
    }

    pub fn is_limited(&self) -> bool {
        self.0.read().unwrap().is_some()
    }

    /// Whether this can play now. A file that didn't import isn't kept anywhere here:
    /// it plays only online.
    pub fn playable(&self, item: &QueueItem) -> bool {
        let Some(here) = self.get() else { return true };
        item.source != AudioSource::NotImported && item.sha256.as_deref().and_then(parse).is_some_and(|h| here.contains(&h))
    }
}

/// What's here: the downloads (`kept`, their hashes on the server) and the blobs in a
/// content-addressed folder (`blobs/ab/cd/<hash>`: the desktop's mirror), if there is one.
/// Reads folders: call it off the UI's thread.
pub fn gather(kept: impl IntoIterator<Item = String>, blobs: Option<&Path>) -> HashSet<Hash> {
    let mut here: HashSet<Hash> = kept.into_iter().filter_map(|h| parse(&h)).collect();
    let entries = |dir: &Path| std::fs::read_dir(dir).into_iter().flatten().flatten();
    for first in blobs.into_iter().flat_map(entries) {
        for second in entries(&first.path()) {
            here.extend(entries(&second.path()).filter_map(|blob| parse(blob.file_name().to_str()?)));
        }
    }
    here
}

/// Whether a file (by its hash) can play, given what `get` returned.
pub fn has(here: Option<&HashSet<Hash>>, hash: Option<Hash>) -> bool {
    match here {
        None => true,
        Some(here) => hash.is_some_and(|h| here.contains(&h)),
    }
}

/// A hash written in hex.
pub fn parse(hex: &str) -> Option<Hash> {
    let mut out = [0u8; 32];
    hex::decode_to_slice(hex, &mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_everything_plays_offline_only_whats_here() {
        let mut item = crate::playback::queue::tests::item(1);
        let on_disk = OnDisk::default();
        assert!(on_disk.playable(&item));
        let hash = "ab".repeat(32);
        on_disk.set(Some([parse(&hash).unwrap()].into()));
        assert!(!on_disk.playable(&item), "no hash known");
        item.sha256 = Some(hash);
        assert!(on_disk.playable(&item));
        item.source = AudioSource::NotImported;
        assert!(!on_disk.playable(&item));
    }

    #[test]
    fn whats_here_is_the_downloads_and_the_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = ("ab".repeat(32), "cd".repeat(32));
        let folder = dir.path().join("ab").join("ab");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(&a), b"").unwrap();
        std::fs::write(folder.join("not-a-hash.part"), b"").unwrap();
        let here = gather([b.clone()], Some(dir.path()));
        assert_eq!(here, [parse(&a).unwrap(), parse(&b).unwrap()].into());
        assert_eq!(gather([], Some(&dir.path().join("missing"))).len(), 0);
    }
}
