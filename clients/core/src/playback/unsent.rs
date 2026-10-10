//! Listens that couldn't be reported (no connection), kept in a file and sent again at the
//! next start, so a play offline or on a bad line still counts.

use std::path::PathBuf;

use uuid::Uuid;

pub struct Unsent {
    file: PathBuf,
    tracks: Vec<Uuid>,
}

impl Unsent {
    /// The ones kept in `file` from before (none if it isn't there).
    pub fn load(file: PathBuf) -> Unsent {
        let tracks = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Unsent { file, tracks }
    }

    pub fn add(&mut self, track: Uuid) {
        self.tracks.push(track);
        self.save();
    }

    /// Takes them all out (to send; what fails again is added back).
    pub fn take(&mut self) -> Vec<Uuid> {
        let all = std::mem::take(&mut self.tracks);
        self.save();
        all
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    fn save(&self) {
        if self.tracks.is_empty() {
            let _ = std::fs::remove_file(&self.file);
        } else if let Ok(bytes) = serde_json::to_vec(&self.tracks) {
            let _ = std::fs::write(&self.file, bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listens_survive_a_restart_and_are_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("unsent.json");
        let a = Uuid::from_u128(1);
        let mut unsent = Unsent::load(file.clone());
        unsent.add(a);
        unsent.add(a);
        assert_eq!(Unsent::load(file.clone()).len(), 2);
        assert_eq!(unsent.take(), vec![a, a]);
        assert_eq!(Unsent::load(file).len(), 0);
    }
}
