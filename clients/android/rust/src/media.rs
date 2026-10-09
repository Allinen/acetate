//! An album's covers and notes (on tracks, albums and artists): what a screen shows, and the
//! changes, each sent at once. The rules are `acetate_core::editor::media`.

use acetate_core::editor::media::{self, NoteOn};
use uuid::Uuid;

use crate::{Core, CoreError, MarkOn, parse_id};

/// One of an album's covers.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoverInfo {
    pub id: String,
    /// For `image_request`.
    pub key: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Where it came from ("embedded", "upload", "copy"…).
    pub source: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NoteInfo {
    pub id: String,
    pub title: String,
    pub body: String,
    /// Taken off (kept).
    pub hidden: bool,
    /// The revision it was read at (sent back with a change).
    pub revision: String,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

fn failed(code: &str, reason: &str) -> CoreError {
    CoreError::Failed { code: code.into(), reason: reason.into() }
}

fn id_of(text: &str) -> Result<Uuid, CoreError> {
    parse_id(text).ok_or_else(|| failed("invalid", "That isn't an id."))
}

fn owner(on: MarkOn, id: &str) -> Result<NoteOn, CoreError> {
    let id = id_of(id)?;
    Ok(match on {
        MarkOn::Track => NoteOn::Track(id),
        MarkOn::Album => NoteOn::Album(id),
        MarkOn::Artist => NoteOn::Artist(id),
    })
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// An album's covers, the main one first.
    pub async fn album_covers(&self, album: String) -> Result<Vec<CoverInfo>, CoreError> {
        let (api, album) = (self.shared.api.clone(), id_of(&album)?);
        let covers = self.runtime.spawn(async move { media::album_covers(&api, album).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(covers.into_iter().map(|c| CoverInfo { id: c.id.to_string(), key: c.blob_sha256.chars().take(16).collect(), width: c.width, height: c.height, source: c.source }).collect())
    }

    /// A new cover from the bytes of an image stored on the phone.
    pub async fn add_album_cover(&self, album: String, bytes: Vec<u8>, name: String) -> Result<(), CoreError> {
        let (api, album) = (self.shared.api.clone(), id_of(&album)?);
        self.runtime.spawn(async move { media::add_album_cover(&api, album, bytes, &name).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }

    /// The covers in this order (all of them); the first is the main one.
    pub async fn order_album_covers(&self, album: String, order: Vec<String>) -> Result<(), CoreError> {
        let (api, album) = (self.shared.api.clone(), id_of(&album)?);
        let order: Vec<Uuid> = order.iter().filter_map(|i| parse_id(i)).collect();
        self.runtime.spawn(async move { media::order_album_covers(&api, album, &order).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }

    /// Moves a cover to the trash (the server keeps it there).
    pub async fn trash_cover(&self, cover: String) -> Result<(), CoreError> {
        let (api, cover) = (self.shared.api.clone(), id_of(&cover)?);
        self.runtime.spawn(async move { media::trash_cover(&api, cover).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }

    /// Puts a cover on each of the album's tracks that hasn't got it.
    pub async fn cover_on_tracks(&self, album: String, cover: String) -> Result<(), CoreError> {
        let (api, album, cover) = (self.shared.api.clone(), id_of(&album)?, id_of(&cover)?);
        self.runtime.spawn(async move { media::cover_on_tracks(&api, album, cover).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }

    /// The notes on a track, album or artist (taken-off ones flagged), oldest first.
    pub async fn notes(&self, on: MarkOn, id: String) -> Result<Vec<NoteInfo>, CoreError> {
        let (api, on) = (self.shared.api.clone(), owner(on, &id)?);
        let notes = self.runtime.spawn(async move { media::notes(&api, on).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(notes
            .into_iter()
            .map(|n| NoteInfo { id: n.id.to_string(), title: n.title, body: n.body, hidden: n.hidden, revision: n.revision_id.to_string(), updated_at: n.updated_at, updated_by: n.updated_by })
            .collect())
    }

    /// Writes a note: a new one on this (`note` None), or a change of one (its id and the
    /// revision it was read at: refused with code `stale` if someone changed it since).
    pub async fn save_note(&self, on: MarkOn, id: String, note: Option<NoteInfo>, title: String, body: String, reason: String) -> Result<(), CoreError> {
        let (api, on) = (self.shared.api.clone(), owner(on, &id)?);
        let edit = match note {
            Some(n) => Some((id_of(&n.id)?, id_of(&n.revision)?)),
            None => None,
        };
        self.runtime.spawn(async move { media::save_note(&api, on, edit, &title, &body, &reason).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }

    /// Takes a note off (kept) or brings it back.
    pub async fn hide_note(&self, note: String, hidden: bool) -> Result<(), CoreError> {
        let (api, note) = (self.shared.api.clone(), id_of(&note)?);
        self.runtime.spawn(async move { media::hide_note(&api, note, hidden).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(())
    }
}
