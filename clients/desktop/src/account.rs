//! Who's logged in and what they may do (`GET /api/auth/me`), and the server's fixed
//! lists (`GET /api/vocab`: statuses, credit roles, album kinds, relation kinds), which
//! edit forms choose from. Fetched after logging in (and at start), in the background.

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
    pub fn fetch(&mut self, api: &Api, runtime: &tokio::runtime::Handle, ctx: &eframe::egui::Context) {
        self.me = None;
        self.vocab = None;
        let (api, arrived, ctx) = (api.clone(), self.arrived.clone(), ctx.clone());
        runtime.spawn(async move {
            let me = api.send_json::<Me>(reqwest::Method::GET, "/api/auth/me", None).await.ok();
            let vocab = api.send_json::<Vocab>(reqwest::Method::GET, "/api/vocab", None).await.ok();
            *arrived.lock().unwrap() = Some((me, vocab));
            ctx.request_repaint();
        });
    }

    /// Takes in what arrived. Call once per frame.
    pub fn poll(&mut self) {
        if let Some((me, vocab)) = self.arrived.lock().unwrap().take() {
            self.me = me;
            self.vocab = vocab.map(Arc::new);
        }
    }

    pub fn can(&self, permission: &str) -> bool {
        self.me.as_ref().is_some_and(|me| me.can(permission))
    }
}
