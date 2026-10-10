//! Who's logged in and what they may do (`GET /api/auth/me`), and the server's fixed
//! lists (`GET /api/vocab`: statuses, credit roles, album kinds, relation kinds; and
//! `GET /api/source-shapes`), which edit forms choose from. Fetched after logging in (and at start), in the background.
//!
//! `me` is kept on disk too (`me-<key>.json`, per server and user), so an app started
//! offline still knows what its user may do; the server's answer replaces it.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::api::Api;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Where `me` is kept.
    file: Option<PathBuf>,
    arrived: Arc<Mutex<Option<(Option<Me>, Option<Vocab>)>>>,
}

impl Account {
    /// Whose account this is: `me` as kept in `file` (None: logged out), until the server
    /// says.
    pub fn open(file: Option<PathBuf>) -> Account {
        let me = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Account { me, file, ..Account::default() }
    }

    /// Asks the server (again: at start, after a login, when it's back online). What's
    /// known stays until the answer; no answer keeps it.
    pub fn fetch(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        let (api, arrived, waker, file) = (api.clone(), self.arrived.clone(), waker.clone(), self.file.clone());
        runtime.spawn(async move {
            let me = api.send_json::<Me>(reqwest::Method::GET, "/api/auth/me", None).await.ok();
            if let (Some(me), Some(file)) = (&me, &file)
                && let Err(err) = std::fs::write(file, serde_json::to_vec(me).unwrap_or_default())
            {
                eprintln!("couldn't keep what the account may do: {err}");
            }
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
            if me.is_some() {
                self.me = me;
            }
            if let Some(vocab) = vocab {
                self.vocab = Some(Arc::new(vocab));
            }
        }
    }

    /// Asks for the vocabulary again (e.g. a kind of flag was made), keeping who's logged
    /// in.
    pub fn refresh_vocab(&mut self, api: &Api, runtime: &tokio::runtime::Handle, waker: &crate::Waker) {
        let (api, arrived, waker) = (api.clone(), self.arrived.clone(), waker.clone());
        runtime.spawn(async move {
            let mut vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            if let Some(vocab) = vocab.as_mut() {
                vocab.source_shapes = api.send_json(reqwest::Method::GET, "/api/source-shapes", None).await.unwrap_or_default();
            }
            if vocab.is_some() {
                *arrived.lock().unwrap() = Some((None, vocab));
                waker.changed();
            }
        });
    }

    pub fn can(&self, permission: &str) -> bool {
        self.me.as_ref().is_some_and(|me| me.can(permission))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_account_may_do_is_kept_for_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("me-x.json");
        std::fs::write(&file, br#"{"permissions":["admin"]}"#).unwrap();
        let account = Account::open(Some(file.clone()));
        assert!(account.can("edit_metadata"));
        // Logged out: nothing.
        assert!(!Account::open(None).can("edit_metadata"));
        // An answer that didn't come keeps what's known.
        let mut account = account;
        *account.arrived.lock().unwrap() = Some((None, None));
        account.poll();
        assert!(account.can("admin"));
    }
}
