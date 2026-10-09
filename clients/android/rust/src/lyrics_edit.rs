//! A track's lyrics, in edit mode: its entries (loaded and saved with the revision they
//! build on) and a draft of one entry's text to time and change. The rules are
//! `acetate_core::editor::lyrics_edit`.

use std::sync::Mutex;

use acetate_core::editor::lyrics_edit::{self, Draft};
use std::sync::Arc;
use uuid::Uuid;

use crate::{Core, CoreError, parse_id};

/// One lyrics entry of a track.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricsEntry {
    pub id: String,
    /// "Official · en · synced".
    pub name: String,
    pub body: String,
    pub official: bool,
    /// Empty: none.
    pub language: String,
    pub synced: bool,
    /// The revision it was read at (sent back with a change).
    pub revision: String,
}

/// A line of the draft.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SyncLine {
    pub text: String,
    /// When it shows, in milliseconds; None: no time.
    pub shows_ms: Option<i64>,
    pub heading: bool,
    pub tag: bool,
    pub language: Option<String>,
}

fn failed(code: &str, reason: &str) -> CoreError {
    CoreError::Failed { code: code.into(), reason: reason.into() }
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// A track's lyrics entries as the server has them now.
    pub async fn lyrics_entries(&self, track: String) -> Result<Vec<LyricsEntry>, CoreError> {
        let (api, track) = (self.shared.api.clone(), parse_id(&track).ok_or_else(|| failed("invalid", "That isn't an id."))?);
        let list = self.runtime.spawn(async move { lyrics_edit::entries(&api, track).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(list
            .into_iter()
            .enumerate()
            .map(|(n, l)| {
                let mut parts = vec![if l.is_official { "Official".to_string() } else { format!("Lyrics {}", n + 1) }];
                if let Some(language) = l.language.as_deref().filter(|s| !s.is_empty()) {
                    parts.push(language.to_string());
                }
                if l.is_synced {
                    parts.push("synced".into());
                }
                LyricsEntry { id: l.id.to_string(), name: parts.join(" · "), body: l.body, official: l.is_official, language: l.language.unwrap_or_default(), synced: l.is_synced, revision: l.revision_id.to_string() }
            })
            .collect())
    }

    /// Writes an entry: a new one (`entry` None) or a change of one, with the revision it
    /// was read at (refused with code `stale` if someone saved it since). Its id.
    pub async fn save_lyrics(&self, track: String, entry: Option<LyricsEntry>, body: String, official: bool, language: String, reason: String) -> Result<String, CoreError> {
        let api = self.shared.api.clone();
        let track = parse_id(&track).ok_or_else(|| failed("invalid", "That isn't an id."))?;
        let edit: Option<(Uuid, Uuid)> = match &entry {
            Some(e) => Some((parse_id(&e.id).ok_or_else(|| failed("invalid", "That isn't an id."))?, parse_id(&e.revision).ok_or_else(|| failed("invalid", "That isn't an id."))?)),
            None => None,
        };
        let (id, _) = self.runtime.spawn(async move { lyrics_edit::save(&api, track, edit, &body, official, &language, &reason).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        self.shared.track_details.lock().unwrap().refresh(track);
        Ok(id.to_string())
    }
}

#[uniffi::export]
impl Core {
    /// The text of an entry as lines to time and change.
    pub fn lyrics_draft(&self, body: String) -> Arc<LyricsDraft> {
        Arc::new(LyricsDraft { draft: Mutex::new(Draft::new(&body)) })
    }
}

#[derive(uniffi::Object)]
pub struct LyricsDraft {
    draft: Mutex<Draft>,
}

#[uniffi::export]
impl LyricsDraft {
    pub fn lines(&self) -> Vec<SyncLine> {
        self.draft.lock().unwrap().view().into_iter().map(|l| SyncLine { text: l.text, shows_ms: l.shows_ms, heading: l.heading, tag: l.tag, language: l.language }).collect()
    }

    /// The line tapping times next.
    pub fn cursor(&self) -> u32 {
        self.draft.lock().unwrap().cursor() as u32
    }

    pub fn set_cursor(&self, line: u32) {
        self.draft.lock().unwrap().set_cursor(line as usize);
    }

    /// The line at the cursor starts at this position in the song; the cursor moves on. The
    /// line timed.
    pub fn tap(&self, position_ms: i64) -> Option<u32> {
        self.draft.lock().unwrap().tap(position_ms).map(|n| n as u32)
    }

    /// Moves a line's time (`from_ms`: where to start from when it has none).
    pub fn nudge(&self, line: u32, by_ms: i64, from_ms: i64) {
        self.draft.lock().unwrap().nudge(line as usize, by_ms, from_ms);
    }

    pub fn clear_time(&self, line: u32) {
        self.draft.lock().unwrap().clear_time(line as usize);
    }

    pub fn set_text(&self, line: u32, text: String) -> bool {
        self.draft.lock().unwrap().set_text(line as usize, &text)
    }

    pub fn set_language(&self, line: u32, language: String) -> bool {
        self.draft.lock().unwrap().set_language(line as usize, &language)
    }

    /// Adds an empty line after this one (None: at the start); it's the cursor.
    pub fn insert_after(&self, line: Option<u32>) {
        self.draft.lock().unwrap().insert_after(line.map(|l| l as usize));
    }

    pub fn insert_heading_before(&self, line: u32) {
        self.draft.lock().unwrap().insert_heading_before(line as usize);
    }

    pub fn remove(&self, line: u32) {
        self.draft.lock().unwrap().remove(line as usize);
    }

    pub fn undo(&self) -> bool {
        self.draft.lock().unwrap().undo()
    }

    /// The lyrics as text (LRC) again.
    pub fn text(&self) -> String {
        self.draft.lock().unwrap().text()
    }

    pub fn changed(&self) -> bool {
        self.draft.lock().unwrap().changed()
    }
}
