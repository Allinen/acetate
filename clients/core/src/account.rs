//! Who's logged in and what they may do (`GET /api/auth/me`), and the server's fixed
//! lists (`GET /api/vocab`: statuses, credit roles, album kinds, relation kinds; and
//! `GET /api/source-shapes`), which edit forms choose from. Fetched after logging in (and at start), in the background.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::api::Api;

#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub permissions: HashSet<String>,
}

impl Me {
    /// Whether they may (`admin` may everything).
    pub fn can(&self, permission: &str) -> bool {
        self.permissions.contains("admin") || self.permissions.contains(permission)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Vocab {
    pub statuses: Vec<String>,
    pub credit_roles: Vec<String>,
    /// For album forms (not yet).
    #[allow(dead_code)]
    pub album_kinds: Vec<String>,
    pub relation_kinds: Vec<RelationKind>,
    /// The kinds of review flag, in the order to show them.
    pub review_kinds: Vec<ReviewKind>,
    /// What each platform's sources hold (`GET /api/source-shapes`, fetched with it).
    #[serde(skip)]
    pub source_shapes: Vec<SourceShape>,
}

/// A platform's sources of one kind (track, album, artist) and their fields.
#[derive(Debug, Clone, Deserialize)]
pub struct SourceShape {
    pub platform: String,
    pub kind: String,
    /// The field with the platform's id: sources with the same id are one source.
    pub id_field: Option<String>,
    pub fields: Vec<ShapeField>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapeField {
    pub name: String,
    /// "int", "str" or "strs" (a list of strings).
    #[serde(rename = "type")]
    pub kind: String,
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewKind {
    pub kind: String,
    /// For people ("No cover").
    pub name: String,
    /// Raised and settled by the server itself, from what the track has ("no cover").
    pub automatic: bool,
    /// Made by people, from the app (raised and settled by hand, like "Fix later").
    #[serde(default)]
    pub custom: bool,
}

impl ReviewKind {
    /// Whether people flag tracks with it (in the editor: `flag:…`).
    pub fn by_hand(&self) -> bool {
        self.custom || self.kind == "fix_later"
    }
}

impl Vocab {
    /// A kind of flag as people read it (a kind not in the list: as the server names it).
    pub fn review_kind_name<'a>(&'a self, kind: &'a str) -> &'a str {
        self.review_kinds.iter().find(|k| k.kind == kind).map_or(kind, |k| k.name.as_str())
    }

    pub fn is_automatic(&self, kind: &str) -> bool {
        self.review_kinds.iter().any(|k| k.kind == kind && k.automatic)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelationKind {
    pub kind: String,
    /// "%A is a remix of %B": %A is this track, %B the other.
    pub template: String,
}

/// `me` and `vocab`, as they arrive.
#[derive(Default)]
pub struct Account {
    pub me: Option<Me>,
    pub vocab: Option<Arc<Vocab>>,
    arrived: Arc<Mutex<Option<(Option<Me>, Option<Vocab>)>>>,
}

impl Account {
    /// Asks the server (again, after a login).
    pub fn fetch(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        self.me = None;
        self.vocab = None;
        let (api, arrived, waker) = (api.clone(), self.arrived.clone(), waker.clone());
        runtime.spawn(async move {
            let me = api.send_json::<Me>(reqwest::Method::GET, "/api/auth/me", None).await.ok();
            let mut vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            if let Some(vocab) = vocab.as_mut() {
                vocab.source_shapes = api.send_json(reqwest::Method::GET, "/api/source-shapes", None).await.unwrap_or_default();
            }
            *arrived.lock().unwrap() = Some((me, vocab));
            waker.changed();
        });
    }

    /// Takes in what arrived. Call once per frame.
    pub fn poll(&mut self) {
        if let Some((me, vocab)) = self.arrived.lock().unwrap().take() {
            self.me = me;
            self.vocab = vocab.map(Arc::new);
        }
    }

    /// Asks for the vocabulary again (e.g. a kind of flag was made), keeping who's logged
    /// in.
    pub fn refresh_vocab(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        let me = self.me.clone();
        let (api, arrived, waker) = (api.clone(), self.arrived.clone(), waker.clone());
        runtime.spawn(async move {
            let mut vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            if let Some(vocab) = vocab.as_mut() {
                vocab.source_shapes = api.send_json(reqwest::Method::GET, "/api/source-shapes", None).await.unwrap_or_default();
            }
            if vocab.is_some() {
                *arrived.lock().unwrap() = Some((me, vocab));
                waker.changed();
            }
        });
    }

    pub fn can(&self, permission: &str) -> bool {
        self.me.as_ref().is_some_and(|me| me.can(permission))
    }
}
