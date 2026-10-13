//! A track's, album's or artist's history, and the changelist: revisions in words
//! (`acetate_core::editor::diff`), and putting one back (a new revision with its data).

use std::collections::HashMap;

use acetate_core::details::{AlbumDetail, ArtistDetail, Revision, TrackDetail};
use acetate_core::editor::diff::changes_named;
use acetate_core::account::Release;
use uuid::Uuid;

use crate::{Core, CoreError, MarkOn, parse_id};

#[derive(Debug, Clone, uniffi::Record)]
pub struct HistoryEntry {
    pub id: String,
    /// "2026-10-10 12:00".
    pub at: String,
    /// What it is of, how it was made and by whom ("metadata · edit by ann").
    pub title: String,
    pub reason: String,
    /// What it changed from the one before, in words.
    pub lines: Vec<String>,
    /// The newest of its kind: what's there now (nothing to restore).
    pub current: bool,
}

/// A page of the changelist.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChangesPage {
    pub entries: Vec<ChangeEntry>,
    /// The page was full: there may be older ones.
    pub more: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChangeEntry {
    /// The revision's id (the next page is asked for after the last one's).
    pub id: String,
    pub at: String,
    /// "track", "album" or "artist", and its id: the page it opens.
    pub on: MarkOn,
    pub entity_id: String,
    pub name: String,
    pub how: String,
    pub reason: String,
    pub lines: Vec<String>,
}

fn failed(code: &str, reason: &str) -> CoreError {
    CoreError::Failed { code: code.into(), reason: reason.into() }
}

fn when(created_at: &str) -> String {
    created_at.get(..16).unwrap_or(created_at).replace('T', " ")
}

fn entries(revisions: &[Revision], names: &HashMap<Uuid, String>, catalog: &acetate_core::catalog::Catalog, what: impl Fn(&Revision) -> String) -> Vec<HistoryEntry> {
    revisions
        .iter()
        .enumerate()
        .map(|(n, r)| {
            // The one before it: the next older of the same kind (and version).
            let before = revisions[n + 1..].iter().find(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            let current = !revisions[..n].iter().any(|o| o.kind == r.kind && o.variant_id == r.variant_id);
            let who = r.author.clone().unwrap_or_else(|| "the system".into());
            HistoryEntry {
                id: r.id.to_string(),
                at: when(&r.created_at),
                title: format!("{}{} by {who}", what(r), r.source),
                reason: r.reason.clone(),
                lines: changes_named(before.map(|b| &b.data), &r.data, catalog, names),
                current,
            }
        })
        .collect()
}

#[uniffi::export]
impl Core {
    /// Whether the user may see revisions (history, the changelist).
    pub fn can_view_history(&self) -> bool {
        self.shared.account.lock().unwrap().can("view_history")
    }

    /// What the server's version says when it isn't this app's (None: they match).
    pub fn version_warning(&self) -> Option<String> {
        self.shared.account.lock().unwrap().server.as_ref().and_then(|s| s.warning())
    }

    /// Whether this app and the server aren't the same API version and catalog format: it can't
    /// be used until one of them is updated (the server turns the app away).
    pub fn version_blocked(&self) -> bool {
        self.shared.account.lock().unwrap().server.as_ref().is_some_and(|s| s.blocked())
    }

    /// The server's release, when it is newer than this app's: the app can be brought up to it
    /// (`download_update`). Never an older one: what the app keeps may not be readable by it.
    pub fn version_update(&self) -> Option<String> {
        self.shared.account.lock().unwrap().server.as_ref().filter(|s| s.release() == Release::ServerNewer).map(|s| s.version.clone())
    }

    /// Where the app of the server's version is (the APK), to install by hand, when the server
    /// is not newer than this app.
    pub fn version_update_url(&self) -> Option<String> {
        self.shared.account.lock().unwrap().server.as_ref().filter(|s| !s.matches_app() && s.release() != Release::ServerNewer).map(|s| s.apk_url())
    }

    /// How far `download_update` has got: bytes so far, and in all (0: not known).
    pub fn update_progress(&self) -> UpdateProgress {
        let (done, total) = *self.update_progress.lock().unwrap();
        UpdateProgress { done, total }
    }
}

/// The download of an app update, for a progress bar.
#[derive(uniffi::Record)]
pub struct UpdateProgress {
    pub done: u64,
    pub total: u64,
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// Downloads the APK of the server's release, checks it against the release's checksum and
    /// keeps it in the app's folder; the path comes back, for the system's installer.
    pub async fn download_update(&self) -> Result<String, CoreError> {
        let server = self.shared.account.lock().unwrap().server.clone().filter(|s| s.release() == Release::ServerNewer);
        let Some(server) = server else { return Err(failed("update", "The server isn't on a newer release than this app.")) };
        let (url, path, progress) = (server.apk_url(), self.data_dir.join("update.apk"), self.update_progress.clone());
        *progress.lock().unwrap() = (0, 0);
        let result = self
            .runtime
            .spawn(async move {
                let apk = acetate_core::release::fetch_verified(&url, &|done, total| *progress.lock().unwrap() = (done, total.unwrap_or(0))).await?;
                tokio::fs::write(&path, apk).await.map_err(|e| format!("Couldn't keep the download: {e}"))?;
                Ok::<_, String>(path.to_string_lossy().into_owned())
            })
            .await;
        match result {
            Ok(Ok(path)) => Ok(path),
            Ok(Err(why)) => Err(failed("update", &why)),
            Err(e) => Err(failed("internal", &e.to_string())),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// Every revision of it, newest first (empty for users who may not see them).
    pub async fn history(&self, on: MarkOn, id: String) -> Result<Vec<HistoryEntry>, CoreError> {
        let (api, id) = (self.shared.api.clone(), parse_id(&id).ok_or_else(|| failed("invalid", "That isn't an id."))?);
        let catalog = self.shared.catalog().ok_or_else(|| failed("catalog", "The library isn't loaded yet."))?;
        let path = match on {
            MarkOn::Track => format!("/api/tracks/{id}"),
            MarkOn::Album => format!("/api/albums/{id}"),
            MarkOn::Artist => format!("/api/artists/{id}"),
        };
        self.runtime
            .spawn(async move {
                let get = |e: String| failed("server", &e);
                Ok(match on {
                    MarkOn::Track => {
                        let d: TrackDetail = acetate_core::changes::get(&api, &path).await.map_err(get)?;
                        let variants = d.variants.clone();
                        let what = |r: &Revision| match (r.kind.as_str(), variants.iter().find(|v| Some(v.id) == r.variant_id)) {
                            ("external", _) => "status and albums · ".to_string(),
                            (_, Some(v)) if variants.len() > 1 => format!("metadata ({}) · ", v.label),
                            _ => "metadata · ".to_string(),
                        };
                        entries(d.revisions.as_deref().unwrap_or(&[]), &d.alias_names, &catalog, what)
                    }
                    MarkOn::Album => {
                        let d: AlbumDetail = acetate_core::changes::get(&api, &path).await.map_err(get)?;
                        entries(d.revisions.as_deref().unwrap_or(&[]), &HashMap::new(), &catalog, |_| String::new())
                    }
                    MarkOn::Artist => {
                        let d: ArtistDetail = acetate_core::changes::get(&api, &path).await.map_err(get)?;
                        entries(d.revisions.as_deref().unwrap_or(&[]), &HashMap::new(), &catalog, |_| String::new())
                    }
                })
            })
            .await
            .map_err(|e| failed("unexpected", &e.to_string()))?
    }

    /// Puts a revision back: a new revision with its data.
    pub async fn restore_revision(&self, on: MarkOn, revision: String, reason: String) -> Result<(), CoreError> {
        let (api, revision) = (self.shared.api.clone(), parse_id(&revision).ok_or_else(|| failed("invalid", "That isn't an id."))?);
        let entity = match on {
            MarkOn::Track => "track",
            MarkOn::Album => "album",
            MarkOn::Artist => "artist",
        };
        self.runtime
            .spawn(async move {
                acetate_core::changes::restore(&api, entity, revision, &reason).await.map_err(|e| failed("server", &e))
            })
            .await
            .map_err(|e| failed("unexpected", &e.to_string()))?
    }

    /// The newest changes to the library's metadata, by anyone; after `before` (the last
    /// one shown) for older ones.
    pub async fn changes(&self, before: Option<String>, imports: bool) -> Result<ChangesPage, CoreError> {
        let (api, before) = (self.shared.api.clone(), before.and_then(|b| parse_id(&b)));
        let catalog = self.shared.catalog().ok_or_else(|| failed("catalog", "The library isn't loaded yet."))?;
        self.runtime
            .spawn(async move {
                let page: acetate_core::changes::Changes = acetate_core::changes::get(&api, &acetate_core::changes::path(before, imports)).await.map_err(|e| failed("server", &e))?;
                let more = page.changes.len() == acetate_core::changes::PAGE;
                let entries = page
                    .changes
                    .iter()
                    .map(|c| ChangeEntry {
                        id: c.id.to_string(),
                        at: when(&c.created_at),
                        on: match c.entity.as_str() {
                            "album" => MarkOn::Album,
                            "artist" => MarkOn::Artist,
                            _ => MarkOn::Track,
                        },
                        entity_id: c.entity_id.to_string(),
                        name: c.name.clone(),
                        how: format!("{} · {} by {}", c.what(), c.source, c.author.clone().unwrap_or_else(|| "the system".into())),
                        reason: c.reason.clone(),
                        lines: changes_named(c.before.as_ref(), &c.data, &catalog, &page.alias_names),
                    })
                    .collect();
                Ok(ChangesPage { entries, more })
            })
            .await
            .map_err(|e| failed("unexpected", &e.to_string()))?
    }
}

/// One of the messages the app showed.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PastMessage {
    pub text: String,
    /// "now", "3 min ago".
    pub ago: String,
    pub error: bool,
}

#[uniffi::export]
impl Core {
    /// Keeps what the app just told the user, to be read again in `messages`.
    pub fn note(&self, text: String, error: bool) {
        self.shared.messages.lock().unwrap().record(&text, std::time::Instant::now(), error);
    }

    /// The last hundred, newest first.
    pub fn messages(&self) -> Vec<PastMessage> {
        let log = self.shared.messages.lock().unwrap();
        log.list
            .iter()
            .rev()
            .map(|m| PastMessage { text: m.text.clone(), ago: acetate_core::messages::ago(m.at), error: m.error })
            .collect()
    }
}
