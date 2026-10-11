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
    /// Its notes (and those of tracks merged into it), oldest first; hidden ones flagged.
    #[serde(default)]
    pub notes: Vec<Note>,
    /// Extras linked to it (vocals, stems…); taken-off ones flagged.
    #[serde(default)]
    pub extras: Vec<Extra>,
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
    /// Changes of the file's origin, newest first (only for users who may see history).
    #[serde(default)]
    pub origin_changes: Option<Vec<OriginChange>>,
    /// Changes of whether the file is an original file (and its name), newest first (only
    /// for users who may see history).
    #[serde(default)]
    pub original_filename_changes: Option<Vec<OriginalFilenameChange>>,
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
    /// Lyrics entries taken off it (kept, to bring back).
    #[serde(default)]
    pub removed_lyrics: Vec<Lyrics>,
    /// The names of the aliases its history credits, listed or not (with history only).
    #[serde(default)]
    pub alias_names: HashMap<Uuid, String>,
    #[serde(default)]
    pub merges: Vec<Merge>,
    /// The extra its file was (it was made a track from it): undone on the extra's page.
    #[serde(default)]
    pub made_from_extra: Option<Uuid>,
    #[serde(default)]
    pub covers: Vec<Cover>,
    /// Every revision, newest first (only for users who may see history).
    pub revisions: Option<Vec<Revision>>,
}

/// A revision of a track: of a version's metadata ("internal", with `variant_id`) or of
/// its facts ("external": status, albums, relations).
#[derive(Debug, Clone, Deserialize)]
pub struct Revision {
    pub id: Uuid,
    /// A track's: "internal" (its metadata, per version) or "external" (its facts). An
    /// album's or artist's: none.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub variant_id: Option<Uuid>,
    pub created_at: String,
    pub author: Option<String>,
    /// "import", "edit", "merge", "restore"…
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub id: Uuid,
    pub sha256: String,
    /// Set only for original files (as the artist named them).
    pub original_filename: Option<String>,
    pub uploaded_filename: String,
    pub size_bytes: i64,
    pub codec: Option<String>,
    /// In kbps (as the server stores it).
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
    pub snapshot_id: Uuid,
    pub source_id: Uuid,
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
    pub old_file_id: Uuid,
    pub replaced_at: String,
    pub reason: String,
    pub old_original_filename: Option<String>,
    pub old_uploaded_filename: String,
    pub old_codec: Option<String>,
    /// In kbps.
    pub old_bitrate: Option<i32>,
    pub old_size_bytes: i64,
    /// Where the earlier file came from (a snapshot of one of the track's sources).
    #[serde(default)]
    pub old_origin: Option<Origin>,
    pub old_trashed_at: Option<String>,
    pub old_purged_at: Option<String>,
}

/// An extra (vocals, stems, a sample…: not a track), as its page and the pages it's linked
/// to show it (`GET /api/extras/{id}`, and in track, album and artist details).
#[derive(Debug, Clone, Deserialize)]
pub struct Extra {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub description: String,
    /// Where it came from: a link, or in words.
    #[serde(default)]
    pub source: String,
    /// Made from this track (hidden while it's an extra).
    #[serde(default)]
    pub from_track: Option<Uuid>,
    /// Its file was made a track (from its YouTube video), which can be undone.
    #[serde(default)]
    pub made_track: Option<Uuid>,
    pub is_restricted: bool,
    /// Who sees it when restricted (besides admins).
    #[serde(default)]
    pub groups: Vec<GroupRef>,
    /// Taken off (kept).
    #[serde(default)]
    pub hidden: bool,
    /// Merged into this extra (this one is in the trash).
    #[serde(default)]
    pub merged_into: Option<Uuid>,
    #[serde(default)]
    pub files: Vec<ExtraFile>,
    /// What it belongs to (those the user sees).
    #[serde(default)]
    pub links: Vec<ExtraLink>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExtraFile {
    pub id: Uuid,
    pub name: String,
    /// The file's hash (a mirror plays it from the disk).
    #[serde(default)]
    pub sha256: Option<String>,
    pub size_bytes: i64,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i16>,
    pub duration_ms: Option<i64>,
    /// A video's (else None).
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
    #[serde(default)]
    pub frame_rate: Option<f32>,
    #[serde(default)]
    pub video_codec: Option<String>,
    /// A video's thumbnail (its image's key).
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default)]
    pub hidden: bool,
}

impl ExtraFile {
    pub fn is_video(&self) -> bool {
        self.width.is_some()
    }

    pub fn thumb(&self) -> Option<crate::catalog::model::Cover> {
        Some(crate::catalog::model::Cover { id: self.id, key: crate::catalog::model::CoverKey::from_hex(self.thumb.as_deref()?)? })
    }
}

/// What an extra is linked to: "track", "album" or "artist", and its name.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtraLink {
    pub kind: String,
    pub id: Uuid,
    pub title: String,
}

/// A note on a track, album or artist (see the server's `db::notes`).
#[derive(Debug, Clone, Deserialize)]
pub struct Note {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    /// Taken off (kept): shown only in edit mode, to bring back.
    #[serde(default)]
    pub hidden: bool,
    /// The base for editing it.
    pub revision_id: Uuid,
    pub updated_at: String,
    pub updated_by: Option<String>,
    /// Written on something merged into this one since.
    pub merged_from: Option<Uuid>,
}

/// A change of whether a file is an original file (`PUT /api/files/{id}/original-filename`).
/// Names None: not an original file.
#[derive(Debug, Clone, Deserialize)]
pub struct OriginalFilenameChange {
    pub changed_at: String,
    pub changed_by: Option<String>,
    #[serde(default)]
    pub reason: String,
    pub old_name: Option<String>,
    pub new_name: Option<String>,
}

/// A change of where a file came from (`PUT /api/files/{id}/origin`).
#[derive(Debug, Clone, Deserialize)]
pub struct OriginChange {
    pub changed_at: String,
    pub changed_by: Option<String>,
    #[serde(default)]
    pub reason: String,
    /// None: purged since.
    pub old: Option<OriginRef>,
    pub new: Option<OriginRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OriginRef {
    pub snapshot_id: Uuid,
    pub platform: String,
    pub external_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub id: Uuid,
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
    pub id: Uuid,
    pub is_official: bool,
    pub is_synced: bool,
    pub language: Option<String>,
    /// Where it came from: "embedded" (in the file), "manual"…
    #[serde(default)]
    pub source: String,
    pub body: String,
    /// Its latest revision: the base for editing it.
    pub revision_id: Uuid,
    /// Newest first (only for users who may see history).
    #[serde(default)]
    pub revisions: Option<Vec<LyricsRevision>>,
}

impl Lyrics {
    /// A lyric from the catalog, to show (not to edit from: it has no revision).
    pub fn from_catalog(l: &crate::catalog::lyrics::Lyric) -> Lyrics {
        Lyrics {
            id: l.id,
            is_official: l.is_official,
            is_synced: l.is_synced,
            language: l.language.map(str::to_string),
            source: String::new(),
            body: l.body.to_string(),
            revision_id: Uuid::nil(),
            revisions: None,
        }
    }
}

/// A track's lyrics to show: from its details if the server sent them in this session
/// (the freshest: just edited, say), else from the catalog (every track's, offline too),
/// else from the details kept on disk.
pub fn lyrics_to_show(catalog: Option<&crate::catalog::Catalog>, detail: &crate::catalog::Cached<TrackDetail>, track: Uuid) -> Vec<Lyrics> {
    if let (Some(d), Some(_)) = (&detail.value, detail.confirmed_at) {
        return d.lyrics.clone();
    }
    let from_catalog: Vec<Lyrics> = catalog.map(|c| c.lyrics.of(track).map(|l| Lyrics::from_catalog(&l)).collect()).unwrap_or_default();
    if !from_catalog.is_empty() {
        return from_catalog;
    }
    detail.value.as_ref().map(|d| d.lyrics.clone()).unwrap_or_default()
}

/// A revision of a lyrics entry: its data (`body`, `is_official`, `language`) then.
#[derive(Debug, Clone, Deserialize)]
pub struct LyricsRevision {
    pub id: Uuid,
    pub created_at: String,
    pub author: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Merge {
    pub id: Uuid,
    /// The track merged away (it's back, as itself, once the merge is undone).
    pub from_track_id: Uuid,
    pub from_title: String,
    /// Why it can't be undone (empty: it can).
    #[serde(default)]
    pub undo_blockers: Vec<String>,
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
    pub id: Uuid,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub extras: Vec<Extra>,
    pub title: String,
    pub kind: String,
    pub release_date: Option<String>,
    /// The latest album revision: the base for editing it.
    pub revision_id: Option<Uuid>,
    #[serde(default)]
    pub credits: Vec<Credit>,
    #[serde(default)]
    pub covers: Vec<Cover>,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub merges: Vec<AlbumMerge>,
    /// Its revisions, newest first (only for those who may see history).
    #[serde(default)]
    pub revisions: Option<Vec<Revision>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumMerge {
    pub id: Uuid,
    /// The album merged away (back as itself once the merge is undone).
    pub from_album_id: Uuid,
    /// None if it isn't visible to the user.
    pub from_title: Option<String>,
    pub merged_at: String,
    pub merged_by: Option<String>,
    #[serde(default)]
    pub reason: String,
    pub undone_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArtistDetail {
    pub id: Uuid,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub extras: Vec<Extra>,
    pub name: String,
    #[serde(default)]
    pub disambiguation: String,
    /// The latest artist revision: the base for editing it.
    pub revision_id: Option<Uuid>,
    /// Other names it's credited under (the listed ones), for "credited as".
    #[serde(default)]
    pub aliases: Vec<AliasInfo>,
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Its revisions, newest first (only for those who may see history).
    #[serde(default)]
    pub revisions: Option<Vec<Revision>>,
}

/// A grouping (a collective, an era, a project…), as its page shows it
/// (`GET /api/groupings/{id}`). Its tracks and albums are in the catalog too.
#[derive(Debug, Clone, Deserialize)]
pub struct GroupingDetail {
    #[serde(default)]
    pub description: String,
    /// Who's in it (said by hand), with how many of its tracks each is credited on.
    #[serde(default)]
    pub members: Vec<GroupingMember>,
    /// Others credited on its tracks (not members unless someone says so).
    #[serde(default)]
    pub credited: Vec<GroupingMember>,
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Extras linked to it (taken-off ones too, flagged).
    #[serde(default)]
    pub extras: Vec<Extra>,
    /// Every change, newest first.
    #[serde(default)]
    pub changes: Vec<GroupingChange>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupingMember {
    pub artist_id: Uuid,
    pub name: String,
    pub tracks: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupingChange {
    pub at: String,
    pub by: Option<String>,
    /// created, edited, added, removed, from_artist.
    pub action: String,
    pub track_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
    /// A member added or taken out.
    #[serde(default)]
    pub artist_id: Option<Uuid>,
    #[serde(default)]
    pub detail: serde_json::Value,
    #[serde(default)]
    pub reason: String,
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
    waker: crate::Waker,
    dir: Option<PathBuf>,
    kept: HashMap<Uuid, Cached<T>>,
    /// Numbers each fetch: only the latest one's answer is kept.
    asked: HashMap<Uuid, u64>,
    /// Errors: why, and for a track made into an extra, that extra.
    answers: Arc<Mutex<Vec<(Uuid, u64, Result<(T, Vec<u8>), (String, Option<Uuid>)>)>>>,
    _kind: PhantomData<T>,
}

impl<T: DeserializeOwned + Send + Sync + 'static> Details<T> {
    pub fn new(kind: &'static str, api: Api, runtime: tokio::runtime::Handle, waker: crate::Waker) -> Details<T> {
        Details { kind, api, runtime, waker, dir: None, kept: HashMap::new(), asked: HashMap::new(), answers: Arc::default(), _kind: PhantomData }
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
        let (api, answers, waker, path) = (self.api.clone(), self.answers.clone(), self.waker.clone(), format!("/api/{}/{id}", self.kind));
        self.runtime.spawn(async move {
            let result = async {
                let body: serde_json::Value = api.send_json(reqwest::Method::GET, &path, None).await?;
                let bytes = serde_json::to_vec(&body)?;
                anyhow::Ok((serde_json::from_value::<T>(body)?, bytes))
            }
            .await
            .map_err(|err| {
                let extra = err.downcast_ref::<crate::api::ApiError>().and_then(|e| e.extra_id);
                (format!("{err:#}"), extra)
            });
            answers.lock().unwrap().push((id, number, result));
            waker.changed();
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
                    cached.became_extra = None;
                    cached.confirmed_at = Some(std::time::SystemTime::now());
                }
                // The copy shown stays; the error says why it may be old.
                Err((err, extra)) => {
                    cached.error = Some(err);
                    cached.became_extra = extra;
                }
            }
        }
    }

    fn path(&self, id: Uuid) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{}-{id}.json", self.kind)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyrics_show_from_the_catalog_without_details() {
        let catalog = crate::catalog::fixture::sample();
        let track = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001);
        let shown = lyrics_to_show(Some(&catalog), &Default::default(), track);
        let bodies: Vec<&str> = shown.iter().map(|l| l.body.as_str()).collect();
        assert_eq!(bodies, ["ein \u{e4}", "[00:01.00]one"], "official first");
        assert!(shown[1].is_synced && shown[1].language.as_deref() == Some("en"));
        assert!(lyrics_to_show(None, &Default::default(), track).is_empty());
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
