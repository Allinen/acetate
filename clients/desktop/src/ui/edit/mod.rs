//! Edit mode's pieces: forms that keep the revision they were filled from, save on
//! Ctrl+Enter (one revision, with an optional reason), discard on Esc, and handle a
//! stale save (someone else saved in between) by loading theirs and keeping your
//! changes on top; and pickers for artists, albums and tracks (the palette, in a mode
//! that hands the choice back to the form that asked).

pub mod track_form;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use uuid::Uuid;

use crate::api::{Api, ApiError};

/// What a picker picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickKind {
    Artist,
    Album,
    Track,
}

/// A choice from a picker: which, and its name (a new one isn't in the catalog yet).
#[derive(Debug, Clone)]
pub struct Picked {
    pub kind: PickKind,
    pub id: Uuid,
    pub name: String,
}

/// Where picks arrive (made new ones come back from the server a moment later).
#[derive(Default, Clone)]
pub struct Picker {
    picked: Arc<Mutex<Option<Picked>>>,
}

impl Picker {
    pub fn pick(&self, kind: PickKind, id: Uuid, name: String) {
        *self.picked.lock().unwrap() = Some(Picked { kind, id, name });
    }

    /// Makes a new artist or album named `name` on the server, then picks it. Errors go
    /// to `failed`.
    pub fn create(&self, api: &Api, runtime: &tokio::runtime::Handle, kind: PickKind, name: String, failed: impl Fn(ApiError) + Send + 'static) {
        let (api, picker) = (api.clone(), self.clone());
        runtime.spawn(async move {
            #[derive(serde::Deserialize)]
            struct Created {
                id: Uuid,
            }
            let (path, body) = match kind {
                PickKind::Artist => ("/api/artists", serde_json::json!({ "name": name })),
                PickKind::Album => ("/api/albums", serde_json::json!({ "title": name, "kind": "album" })),
                PickKind::Track => return,
            };
            match api.send_json::<Created>(reqwest::Method::POST, path, Some(body)).await {
                Ok(created) => picker.pick(kind, created.id, name),
                Err(err) => failed(err),
            }
        });
    }

    /// The pick, if one arrived (taken: each is handed out once).
    pub fn take(&self) -> Option<Picked> {
        self.picked.lock().unwrap().take()
    }
}

/// Where a form's save is.
#[derive(Debug, Clone, Default)]
pub enum SaveState {
    #[default]
    Idle,
    Saving,
    Saved(Instant),
    /// Someone else saved in between: the server's message.
    Stale(String),
    Failed(String),
}
