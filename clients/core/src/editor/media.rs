//! An album's covers and notes on tracks, albums and artists, with no UI: loading what a
//! screen shows and the changes, each sent at once (nothing here is deleted: a cover goes to
//! the trash, a note is taken off and can be brought back). A track's covers are changed in
//! its editor (`Session::add_staged_cover`, `cover:` lines).

use serde::Deserialize;
use uuid::Uuid;

use crate::api::{Api, ApiError};
use crate::details::{AlbumDetail, ArtistDetail, Cover, Note, TrackDetail};

/// What a note is written on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteOn {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl NoteOn {
    fn path(self) -> String {
        match self {
            NoteOn::Track(id) => format!("/api/tracks/{id}"),
            NoteOn::Album(id) => format!("/api/albums/{id}"),
            NoteOn::Artist(id) => format!("/api/artists/{id}"),
        }
    }

    /// The field naming it in `POST /api/notes`.
    fn field(self) -> (&'static str, Uuid) {
        match self {
            NoteOn::Track(id) => ("track_id", id),
            NoteOn::Album(id) => ("album_id", id),
            NoteOn::Artist(id) => ("artist_id", id),
        }
    }
}

/// An album's covers, the main one first.
pub async fn album_covers(api: &Api, album: Uuid) -> Result<Vec<Cover>, ApiError> {
    Ok(api.send_json::<AlbumDetail>(reqwest::Method::GET, &format!("/api/albums/{album}"), None).await?.covers)
}

/// A new cover from an image's bytes (the server says if it isn't one it can use).
pub async fn add_album_cover(api: &Api, album: Uuid, bytes: Vec<u8>, name: &str) -> Result<(), ApiError> {
    api.upload::<serde_json::Value>(&format!("/api/albums/{album}/covers"), bytes, name).await.map(|_| ())
}

/// The covers in this order (all of them).
pub async fn order_album_covers(api: &Api, album: Uuid, order: &[Uuid]) -> Result<(), ApiError> {
    api.send(reqwest::Method::PUT, &format!("/api/albums/{album}/covers/order"), Some(serde_json::json!({ "cover_ids": order }))).await
}

/// The cover goes to the trash (the image stays wherever else it's used).
pub async fn trash_cover(api: &Api, cover: Uuid) -> Result<(), ApiError> {
    api.send(reqwest::Method::DELETE, &format!("/api/covers/{cover}"), None).await
}

/// The cover on each of the album's tracks that hasn't got it.
pub async fn cover_on_tracks(api: &Api, album: Uuid, cover: Uuid) -> Result<(), ApiError> {
    api.send(reqwest::Method::POST, &format!("/api/albums/{album}/covers/apply-to-tracks"), Some(serde_json::json!({ "cover_id": cover }))).await
}

/// An image uploaded to be a cover when an edit is applied (staged on the server): its hash.
pub async fn stage_cover(api: &Api, bytes: Vec<u8>, name: &str) -> Result<String, ApiError> {
    #[derive(Deserialize)]
    struct Staged {
        blob_sha256: String,
    }
    Ok(api.upload::<Staged>("/api/covers/staged", bytes, name).await?.blob_sha256)
}

/// What it's written on has these notes (taken-off ones flagged), oldest first.
pub async fn notes(api: &Api, on: NoteOn) -> Result<Vec<Note>, ApiError> {
    Ok(match on {
        NoteOn::Track(_) => api.send_json::<TrackDetail>(reqwest::Method::GET, &on.path(), None).await?.notes,
        NoteOn::Album(_) => api.send_json::<AlbumDetail>(reqwest::Method::GET, &on.path(), None).await?.notes,
        NoteOn::Artist(_) => api.send_json::<ArtistDetail>(reqwest::Method::GET, &on.path(), None).await?.notes,
    })
}

/// A new note (`edit`: None) or a change of one (`edit`: its id and the revision it was read
/// at: refused as `stale` if someone changed it since).
pub async fn save_note(api: &Api, on: NoteOn, edit: Option<(Uuid, Uuid)>, title: &str, body: &str, reason: &str) -> Result<(), ApiError> {
    match edit {
        Some((note, base)) => {
            api.send(reqwest::Method::PUT, &format!("/api/notes/{note}"), Some(serde_json::json!({ "base_revision_id": base, "title": title, "body": body, "reason": reason }))).await
        }
        None => {
            let (field, id) = on.field();
            let mut json = serde_json::json!({ "title": title, "body": body, "reason": reason });
            json[field] = serde_json::json!(id);
            api.send(reqwest::Method::POST, "/api/notes", Some(json)).await
        }
    }
}

/// Takes a note off (kept) or brings it back.
pub async fn hide_note(api: &Api, note: Uuid, hidden: bool) -> Result<(), ApiError> {
    api.send(reqwest::Method::POST, &format!("/api/notes/{note}/hidden"), Some(serde_json::json!({ "hidden": hidden }))).await
}
