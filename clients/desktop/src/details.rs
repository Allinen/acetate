//! Everything about one track, album or artist (`GET /api/tracks/{id}` etc.), for their
//! pages: kept on disk and shown at once, fetched again in the background each time a
//! page is opened; the server's copy wins, as for the catalog.
//!
//! Only what the pages show is read here (serde ignores the rest), and anything the
//! server may leave out has a default, so a newer server doesn't break an older app.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::Api;
use crate::catalog::Cached;

#[derive(Debug, Clone, Deserialize)]
pub struct TrackDetail {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub status: String,
    pub release_date: Option<String>,
    pub primary_variant_id: Option<Uuid>,
    /// The latest external revision (status, albums, relations): the base for editing
    /// those.
    pub external_revision_id: Option<Uuid>,
    pub file: FileInfo,
    #[serde(default)]
    pub file_shared_with: Vec<Shared>,
    /// Only for users who may see history.
    pub file_history: Option<Vec<FileChange>>,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub credits: Vec<Credit>,
    #[serde(default)]
    pub albums: Vec<AlbumEntry>,
    #[serde(default)]
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub variants: Vec<Variant>,
    #[serde(default)]
    pub lyrics: Vec<Lyrics>,
    #[serde(default)]
    pub merges: Vec<Merge>,
    #[serde(default)]
    pub covers: Vec<Cover>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub sha256: String,
    /// Set only for original files (as the artist named them).
    pub original_filename: Option<String>,
    pub uploaded_filename: String,
    pub size_bytes: i64,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub bit_depth: Option<i16>,
    pub origin: Option<Origin>,
    pub uploaded_at: String,
    #[serde(default)]
    pub embedded_tags: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Origin {
    pub platform: String,
    pub external_id: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Shared {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileChange {
    pub replaced_at: String,
    pub reason: String,
    pub old_original_filename: Option<String>,
    pub old_uploaded_filename: String,
    pub old_codec: Option<String>,
    pub old_bitrate: Option<i32>,
    pub old_size_bytes: i64,
    pub old_trashed_at: Option<String>,
    pub old_purged_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub platform: String,
    pub external_id: Option<String>,
    /// Newest first.
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Snapshot {
    pub captured_at: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credit {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub role: String,
    pub alias_id: Option<Uuid>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumEntry {
    pub album_id: Uuid,
    pub title: String,
    pub disc_number: Option<i16>,
    pub track_number: Option<i16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Relation {
    pub kind: String,
    pub track_id: Uuid,
    pub title: String,
    pub artist_display: String,
    /// "%A is a remix of %B": %A is this track, %B the other one.
    pub template: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Variant {
    pub id: Uuid,
    pub label: String,
    /// Its latest revision: the base for editing it.
    pub revision_id: Uuid,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lyrics {
    pub is_official: bool,
    pub is_synced: bool,
    pub language: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Merge {
    pub from_title: String,
    pub merged_at: String,
    pub merged_by: Option<String>,
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cover {
    pub id: Uuid,
    pub blob_sha256: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumDetail {
    #[serde(default)]
    pub credits: Vec<Credit>,
    #[serde(default)]
    pub covers: Vec<Cover>,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub merges: Vec<AlbumMerge>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumMerge {
    #[serde(default)]
    pub from_title: String,
    #[serde(default)]
    pub merged_at: String,
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistDetail {
    /// Other names it's credited under (the listed ones), for "credited as".
    #[serde(default)]
    pub aliases: Vec<AliasInfo>,
    #[serde(default)]
    pub sources: Vec<Source>,
}

impl Cover {
    /// As the pages draw covers.
    pub fn cover(&self) -> Option<crate::catalog::model::Cover> {
        Some(crate::catalog::model::Cover { id: self.id, key: crate::catalog::model::CoverKey::from_hex(&self.blob_sha256)? })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AliasInfo {
    pub id: Uuid,
    pub name: String,
}

/// One kind of detail (tracks, albums or artists), kept per id.
pub struct Details<T> {
    /// The API's path for it ("tracks"), and the disk copies' file prefix.
    kind: &'static str,
    api: Api,
    runtime: tokio::runtime::Handle,
    ctx: eframe::egui::Context,
    dir: Option<PathBuf>,
    kept: HashMap<Uuid, Cached<T>>,
    /// Numbers each fetch: only the latest one's answer is kept.
    asked: HashMap<Uuid, u64>,
    answers: Arc<Mutex<Vec<(Uuid, u64, Result<(T, Vec<u8>), String>)>>>,
    _kind: PhantomData<T>,
}

impl<T: DeserializeOwned + Send + Sync + 'static> Details<T> {
    pub fn new(kind: &'static str, api: Api, runtime: tokio::runtime::Handle, ctx: eframe::egui::Context) -> Details<T> {
        Details { kind, api, runtime, ctx, dir: None, kept: HashMap::new(), asked: HashMap::new(), answers: Arc::default(), _kind: PhantomData }
    }

    /// Starts over for a login: `dir` is where this server's and user's copies are kept.
    pub fn reset(&mut self, dir: Option<PathBuf>) {
        self.dir = dir;
        self.kept.clear();
    }

    /// As kept (from disk at first); fetched in the background the first time it's asked
    /// for in this session, and again by `refresh`.
    pub fn get(&mut self, id: Uuid) -> &Cached<T> {
        if !self.kept.contains_key(&id) {
            let kept = self.path(id).and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<T>(&b).ok());
            self.kept.insert(id, Cached { value: kept.map(Arc::new), ..Cached::default() });
            self.refresh(id);
        }
        &self.kept[&id]
    }

    /// A page opened: shown as kept, and fetched again.
    pub fn open(&mut self, id: Uuid) {
        if self.kept.contains_key(&id) {
            self.refresh(id);
        } else {
            self.get(id);
        }
    }

    pub fn refresh(&mut self, id: Uuid) {
        let number = {
            let n = self.asked.entry(id).or_default();
            *n += 1;
            *n
        };
        if let Some(cached) = self.kept.get_mut(&id) {
            cached.refreshing = true;
        }
        let (api, answers, ctx, path) = (self.api.clone(), self.answers.clone(), self.ctx.clone(), format!("/api/{}/{id}", self.kind));
        self.runtime.spawn(async move {
            let result = async {
                let body: serde_json::Value = api.send_json(reqwest::Method::GET, &path, None).await?;
                let bytes = serde_json::to_vec(&body)?;
                anyhow::Ok((serde_json::from_value::<T>(body)?, bytes))
            }
            .await
            .map_err(|err| format!("{err:#}"));
            answers.lock().unwrap().push((id, number, result));
            ctx.request_repaint();
        });
    }

    /// Takes in the server's answers. Call once per frame.
    pub fn poll(&mut self) {
        let answers = std::mem::take(&mut *self.answers.lock().unwrap());
        for (id, number, result) in answers {
            if self.asked.get(&id) != Some(&number) {
                continue;
            }
            let path = self.path(id);
            let cached = self.kept.entry(id).or_default();
            cached.refreshing = false;
            match result {
                Ok((value, bytes)) => {
                    if let Some(path) = path {
                        if let Some(dir) = path.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        let _ = std::fs::write(path, bytes);
                    }
                    cached.value = Some(Arc::new(value));
                    cached.error = None;
                    cached.confirmed_at = Some(std::time::SystemTime::now());
                }
                // The copy shown stays; the error says why it may be old.
                Err(err) => cached.error = Some(err),
            }
        }
    }

    fn path(&self, id: Uuid) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}-{id}.json", self.kind)))
    }
}

/// Against a real server: every track's, album's and artist's details parse.
/// `ACETATE_TEST_SERVER=http://… ACETATE_TEST_TOKEN=… ACETATE_TEST_CATALOG=<catalog.json>
/// cargo test details_parse -- --ignored`.
#[cfg(test)]
mod against_a_server {
    use super::*;

    #[test]
    #[ignore]
    fn details_parse_for_everything() {
        let (Ok(server), Ok(token), Ok(catalog)) =
            (std::env::var("ACETATE_TEST_SERVER"), std::env::var("ACETATE_TEST_TOKEN"), std::env::var("ACETATE_TEST_CATALOG"))
        else {
            return;
        };
        let catalog: serde_json::Value = serde_json::from_slice(&std::fs::read(catalog).unwrap()).unwrap();
        let ids = |part: &str| -> Vec<String> {
            catalog[part]["rows"].as_array().unwrap().iter().map(|r| r[0].as_str().unwrap().to_string()).collect()
        };
        let dir = std::env::temp_dir().join(format!("acetate-details-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("session.json"), serde_json::json!({"server": server, "token": token, "username": "test"}).to_string()).unwrap();
        let api = Api::new(dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut lyrics = 0;
            for id in ids("tracks") {
                let d: TrackDetail = api.send_json(reqwest::Method::GET, &format!("/api/tracks/{id}"), None).await.unwrap_or_else(|e| panic!("track {id}: {e:#}"));
                lyrics += d.lyrics.len();
            }
            for id in ids("albums") {
                let _: AlbumDetail = api.send_json(reqwest::Method::GET, &format!("/api/albums/{id}"), None).await.unwrap_or_else(|e| panic!("album {id}: {e:#}"));
            }
            for id in ids("artists") {
                let _: ArtistDetail = api.send_json(reqwest::Method::GET, &format!("/api/artists/{id}"), None).await.unwrap_or_else(|e| panic!("artist {id}: {e:#}"));
            }
            println!("all parsed; {lyrics} lyrics entries");
        });
    }
}
