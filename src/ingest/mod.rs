//! Turning uploaded files into tracks, and replacing a track's file.
//!
//! Flow: hash (done while receiving, by the blob store) -> skip if already stored ->
//! probe + read tags -> check the tags are good enough -> store blob -> one DB
//! transaction creating file, track, variant, revision 0, credits, album, covers, lyrics.
//!
//! Metadata is taken verbatim from the tags. Nothing is guessed: files without the
//! required tags are rejected instead of being imported with made-up values.

pub mod comment_links;
pub mod describe;
pub mod layouts;
pub mod probe;
pub mod video;
pub mod tags;

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::db;
use crate::db::ingest::StoredFile;
use crate::db::revisions::Source;
use crate::model::{AlbumMembership, Credit, ImageInfo, LyricsData, TrackFacts, TrackMetadata};
use crate::model::sources::{SourceOwner, ValidSource};
use crate::storage::{BlobStore, PendingBlob};
use describe::{Hints, MatchedBy, Report, ReportStatus, Resolver};
use probe::ProbeInfo;
use tags::TagInfo;

#[derive(Debug, Clone)]
pub struct IngestOptions {
    /// Who is uploading. None = the system (CLI import).
    pub user: Option<Uuid>,
    /// The literal filename the file was uploaded / imported with.
    pub uploaded_filename: String,
    /// Where this file came from, as a source snapshot (e.g. the SoundCloud upload, or a
    /// custom "sent on Discord" source). Required. It's added to the track's sources and
    /// the file points at it. For a duplicate, it's added to the existing track's sources
    /// (the stored file keeps its original origin).
    pub origin: ValidSource,
    /// Initial track status, e.g. "released" or "unreleased".
    pub status: String,
    /// External source data for the track (already validated), supplied by the uploader.
    /// Also added when the file turns out to be a duplicate, to the existing track.
    pub sources: Vec<ValidSource>,
    /// Create the track restricted to these groups (None = visible to everyone). Done in
    /// the same transaction, so the track is never briefly public.
    pub restrict_to: Option<Vec<Uuid>>,
    /// What a description says about the artists and album (see `describe`). Empty for
    /// plain uploads.
    pub hints: Hints,
    /// Where the file was on disk (the CLI's, or what an HTTP client says): kept as
    /// `files.imported_from`, and shown in the review list if it doesn't import.
    pub local_path: Option<String>,
    /// Values typed in by hand for what the tags lack (see `Given`).
    pub given: Given,
    /// Which storage volume the audio goes to (None: the default one). Admins choose.
    pub volume: Option<String>,
    /// Read the metadata from this copy of the file instead of the file itself (see
    /// `TagsFrom`).
    pub tags_from: Option<TagsFrom>,
}

/// A tagged copy of the file being imported, whose tags (and embedded pictures) are the
/// metadata, while the file stored is the untouched original: e.g. a pristine download
/// of an original file, tagged by hand in a local copy. The copy itself isn't kept; its
/// raw tags are kept next to the file's own (`files.embedded_tags.tags_from_copy`).
#[derive(Debug, Clone)]
pub struct TagsFrom {
    /// Where the copy is (a temp file while the request lasts).
    pub path: PathBuf,
    /// The copy's own name (its extension helps tell its format).
    pub filename: String,
}

/// Title and artists given by hand for a file whose tags lack them (the Review page's
/// "import with these values"). They're used instead of the tags' for those fields, in the
/// track's first revision, which says so. The file and its stored raw tags aren't changed.
#[derive(Debug, Clone, Default)]
pub struct Given {
    pub title: Option<String>,
    /// Split on `;` like the ARTIST tag.
    pub artists: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum IngestOutcome {
    Created(Uuid),
    /// The exact same file (same sha256) is already stored, for this track. None: for a
    /// track the uploader can't see (which isn't revealed).
    Duplicate(Option<Uuid>),
}

/// Reasons a file is refused. These are the user's problem (bad input), not server
/// errors, so the API reports them as 4xx and the CLI as "skipped".
#[derive(Debug)]
pub enum Rejected {
    NotAudio(String),
    /// The tags lack fields we require. Lists the missing ones.
    MissingTags(Vec<&'static str>),
    TrackNotFound(Uuid),
    /// The metadata or options don't fit (e.g. an unknown status).
    Invalid(String),
    /// This exact file is already stored (as the current or a previous file of `track_id`).
    AlreadyStored { track_id: Option<Uuid> },
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejected::NotAudio(details) => write!(f, "not a recognized audio file ({details})"),
            Rejected::MissingTags(missing) => {
                write!(f, "missing required tags: {}", missing.join(", "))
            }
            Rejected::TrackNotFound(id) => write!(f, "track {id} not found"),
            Rejected::Invalid(msg) => write!(f, "{msg}"),
            Rejected::AlreadyStored { track_id: Some(id) } => {
                write!(f, "this exact file is already stored (track {id})")
            }
            Rejected::AlreadyStored { track_id: None } => {
                write!(f, "this exact file is already stored")
            }
        }
    }
}

// Lets `Rejected` be used as an error (and so be wrapped in anyhow::Error and downcast later).
impl std::error::Error for Rejected {}

/// Takes a file from disk into a pending blob (hashed). Used by the CLI.
///
/// On a filesystem that can, it's a reflink (see `BlobStore::reflink_in`): no bytes are
/// copied and the library isn't stored twice. Otherwise it's streamed into the store's
/// temp folder, hashing on the way.
///
/// `volume`: where it's going to be stored, so the reflink lands on that volume's disk.
pub async fn receive_path(store: &BlobStore, path: &Path, volume: &str) -> anyhow::Result<PendingBlob> {
    if let Some(pending) = store.reflink_in(path, volume).await? {
        return Ok(pending);
    }
    copy_path(store, path).await
}

/// Takes a file from disk into a pending blob by copying its bytes (into the default
/// volume's temp folder; `BlobStore::commit` moves it to the right volume).
pub async fn copy_path(store: &BlobStore, path: &Path) -> anyhow::Result<PendingBlob> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut writer = store.writer().await?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write(&buf[..n]).await?;
    }
    writer.finish().await
}

/// Ingests a blob that has already been received and hashed, creating a new track.
pub async fn ingest(
    pool: &PgPool,
    store: &BlobStore,
    pending: PendingBlob,
    options: IngestOptions,
) -> anyhow::Result<IngestOutcome> {
    let report = ingest_with_report(pool, store, pending, &options, false).await?;
    Ok(match report.status {
        ReportStatus::Created => IngestOutcome::Created(report.track_id.context("created a track")?),
        ReportStatus::Duplicate => IngestOutcome::Duplicate(report.track_id),
    })
}

/// A received file, ready to be ingested: its hash, and where its bytes are.
pub struct StagedFile {
    pub sha256: String,
    pub size: u64,
    /// The volume it's stored on (a dry run: would be).
    pub volume: String,
    /// Where to read it: in the blob store, or (dry run) the temp file.
    path: PathBuf,
    /// A dry run's temp file, deleted when this is dropped.
    _temp: Option<PendingBlob>,
}

impl StagedFile {
    /// For a real run, the file goes into the blob store right away, *before* it's
    /// checked: if it's refused, the audio is kept for the review list (see
    /// `db::problems`). If it's a duplicate, nothing new is stored (same hash, same blob).
    /// A dry run only keeps the temp file, which goes away.
    ///
    /// It goes to `volume`, unless the same bytes are already stored on another one.
    pub async fn new(store: &BlobStore, pending: PendingBlob, dry_run: bool, volume: &str) -> anyhow::Result<StagedFile> {
        let sha256 = pending.sha256.clone();
        let size = pending.size;
        if dry_run {
            let path = pending.path().to_path_buf();
            let volume = volume.to_string();
            Ok(StagedFile { sha256, size, volume, path, _temp: Some(pending) })
        } else {
            let (volume, path) = store.commit(pending, volume).await?;
            Ok(StagedFile { sha256, size, volume, path, _temp: None })
        }
    }

    /// A file already in the blob store (e.g. one kept for the review list).
    pub async fn stored(store: &BlobStore, sha256: &str, size: u64) -> StagedFile {
        let (volume, path) = match store.find(sha256).await {
            Some(found) => found,
            None => (store.default_volume().to_string(), store.path_for(sha256).await),
        };
        StagedFile {
            sha256: sha256.to_string(),
            size,
            volume,
            path,
            _temp: None,
        }
    }
}

/// Ingests one file in its own transaction, and says what it did. With `dry_run`, works
/// out everything the same way and then rolls back, storing nothing.
///
/// A file that isn't imported (refused, or an error) is listed for review, with its
/// audio kept (not in a dry run).
pub async fn ingest_with_report(
    pool: &PgPool,
    store: &BlobStore,
    pending: PendingBlob,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<Report> {
    let volume = options.volume.as_deref().unwrap_or(store.default_volume());
    let file = StagedFile::new(store, pending, dry_run, volume).await?;
    ingest_staged(pool, store, &file, options, dry_run).await
}

/// `ingest_with_report` for a file that's staged already (e.g. kept for the review list).
pub async fn ingest_staged(
    pool: &PgPool,
    store: &BlobStore,
    file: &StagedFile,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<Report> {
    let sha256 = file.sha256.clone();
    let mut tx = db::begin(pool, options.user).await?;
    match ingest_in(&mut tx, store, file, options, dry_run).await {
        Ok(report) => {
            if !dry_run {
                tx.commit().await?;
            }
            Ok(report)
        }
        Err(err) => {
            drop(tx); // rolls back
            // Two uploads of the same file at the same time: the second one loses the race on
            // the unique sha256 constraint. Treat that as a duplicate rather than an error.
            if is_unique_violation(&err, "files_sha256_key") {
                let mut tx = db::begin(pool, options.user).await?;
                if let Some(report) = duplicate_in(&mut tx, &sha256, options).await? {
                    if !dry_run {
                        tx.commit().await?;
                    }
                    return Ok(report);
                }
            }
            let err = as_rejected(err);
            if !dry_run {
                // Listing it is a convenience: if that fails too, the original error is
                // what matters.
                if let Err(record_err) = record_problem(pool, store, file, options, &err).await {
                    tracing::error!("couldn't list {} for review: {record_err:#}", options.uploaded_filename);
                }
            }
            Err(err)
        }
    }
}

/// A track's cover as ingest found it: embedded in the file, or given with the description.
struct TrackCover {
    sha256: String,
    mime: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    /// Bytes.
    size: i64,
    source: &'static str,
}

/// Whether an error means the file (or what was said about it) was refused, rather than
/// something going wrong on our side.
pub fn is_refusal(err: &anyhow::Error) -> bool {
    err.is::<Rejected>()
        || err.is::<db::revisions::EditRejected>()
        || err.is::<crate::model::sources::InvalidSource>()
}

/// Lists a file that didn't import on the Review page (see `db::problems`).
async fn record_problem(
    pool: &PgPool,
    store: &BlobStore,
    file: &StagedFile,
    options: &IngestOptions,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let refused = is_refusal(err);
    let reason = if refused { err.to_string() } else { format!("{err:#}") };
    // The tags, to see what's missing. (Not there for files that aren't audio.)
    let path = file.path.clone();
    let extension = file_extension(&options.uploaded_filename);
    let tags = tokio::task::spawn_blocking(move || tags::read_tags(&path, extension.as_deref()))
        .await?
        .ok();
    let mut description = describe::description_json(
        &options.origin,
        &options.status,
        &options.sources,
        &options.hints,
        options.restrict_to.as_deref(),
    );
    // The covers sent with it are kept as blobs, named in the description by their hash
    // (`load_problem_covers` reads them back for importing it again).
    let mut cover_blobs = Vec::new();
    let mut keep = async |images: &[Vec<u8>]| -> anyhow::Result<Vec<String>> {
        let mut names = Vec::new();
        for bytes in images {
            let sha256 = store.put_bytes(bytes).await?;
            names.push(format!("{}{sha256}", describe::STORED_COVER));
            cover_blobs.push(sha256);
        }
        Ok(names)
    };
    let track_covers = keep(&options.hints.track_covers).await?;
    let album_covers = match &options.hints.album {
        Some(album) => keep(&album.covers).await?,
        None => Vec::new(),
    };
    if !track_covers.is_empty() {
        description["track"]["covers"] = json!(track_covers);
    }
    if !album_covers.is_empty() {
        let album = if options.hints.described.is_some() { &mut description["metadata"]["album"] } else { &mut description["album"] };
        if album.is_object() {
            album["covers"] = json!(album_covers);
        }
    }
    let problem = db::problems::NewProblem {
        kind: if refused { "skipped" } else { "failed" },
        sha256: &file.sha256,
        size_bytes: file.size as i64,
        uploaded_filename: &options.uploaded_filename,
        path: options.local_path.as_deref(),
        reason: &reason,
        description,
        embedded_tags: tags.as_ref().map(|t| json!({ "tags": t.raw })),
        tag_title: tags.as_ref().and_then(|t| t.title.as_deref()),
        tag_artists: tags.as_ref().map_or(&[], |t| &t.artists),
    };
    let mut tx = db::begin(pool, options.user).await?;
    db::blobs::register(&mut tx, &file.sha256, &file.volume, Some(file.size as i64)).await?;
    for sha256 in &cover_blobs {
        let size = store.size(sha256).await.map(|s| s as i64);
        db::blobs::register(&mut tx, sha256, store.default_volume(), size).await?;
    }
    db::problems::record(&mut tx, &problem, &cover_blobs).await?;
    tx.commit().await?;
    Ok(())
}

fn file_extension(filename: &str) -> Option<String> {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

/// Turns an invalid edit (e.g. an unknown status) into a refused file, which the API
/// reports as the client's problem and the CLI as "skipped". Other errors stay as they are.
pub fn as_rejected(err: anyhow::Error) -> anyhow::Error {
    match err.downcast_ref::<db::revisions::EditRejected>() {
        Some(db::revisions::EditRejected::Invalid(msg)) => Rejected::Invalid(msg.clone()).into(),
        _ => err,
    }
}

/// Ingests one file inside the caller's transaction, acting as the transaction's user.
/// Committing (or not) is up to the caller: `ingest_with_report` commits each file, while
/// the CLI's dry run keeps a whole run in one transaction that's rolled back at the end,
/// so later files see what earlier ones would have created.
///
/// On an error the transaction must be rolled back (it may still be acting as the system).
pub async fn ingest_in(
    conn: &mut PgConnection,
    store: &BlobStore,
    file: &StagedFile,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<Report> {
    // Explicit ids have to be ones the uploader can see (checked with their access).
    describe::check_explicit_ids(conn, &options.hints).await?;
    // Metadata given whole: its roles and album kind are checked before anything, also for
    // a file that's stored already.
    if let Some(described) = &options.hints.described {
        describe::check_described_vocab(conn, described).await?;
    }
    // The groups to restrict it to exist: refused also for a file that's stored already
    // (which isn't restricted by this import), so a wrong id never goes unnoticed.
    if let Some(groups) = &options.restrict_to {
        db::access::check_groups(conn, groups).await?;
    }

    // 1. Same bytes already stored? What this import says that the track doesn't have yet
    //    (another album, other metadata...) is merged into it (`merge_duplicate`). Else,
    //    or with nothing new, there's nothing to do but note the sources.
    if !options.hints.no_duplicate_merge {
        if let Some(report) = merge_duplicate(conn, store, file, options, dry_run).await? {
            return Ok(report);
        }
    }
    if let Some(mut report) = duplicate_in(conn, &file.sha256, options).await? {
        report.album_covers_added = covers_for_known_album(conn, store, options, dry_run).await?;
        return Ok(report);
    }
    new_track(conn, store, file, options, dry_run, None).await
}

/// A file that's already stored, imported again: whatever this import says that its track
/// doesn't have yet is added, the way merging two tracks does it. The file is imported as a
/// new track that plays the stored file (nothing is copied), which is then merged into the
/// track that has it, keeping that track's metadata (see `revisions::merge_tracks`, with
/// `leave_identical`): another album (e.g. the single, when it came in on the album),
/// covers, lyrics, ids for its artists and album, and metadata the track never had, as
/// another variant (only from a tagged copy or values given by hand: the file's own tags
/// are the same bytes as before). It's a merge like any other, so it can be undone.
///
/// None when there's nothing new (then nothing is kept, and it's a plain duplicate), when
/// the stored file isn't a track's current file, or when the uploader can't see that track.
async fn merge_duplicate(
    conn: &mut PgConnection,
    store: &BlobStore,
    file: &StagedFile,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<Option<Report>> {
    let system = db::as_system(conn).await?;
    let found = db::ingest::current_file_by_sha256(conn, &file.sha256).await?;
    system.end(conn).await?;
    let Some((file_id, existing)) = found else {
        return Ok(None);
    };
    if !db::sources::owner_exists(conn, SourceOwner::Track(existing)).await? {
        return Ok(None);
    }

    // Tried in a savepoint: rolled back if it brings nothing new.
    let mut attempt = sqlx::Connection::begin(&mut *conn).await?;
    let system = db::as_system(&mut attempt).await?;
    let snapshots_before = db::ingest::album_and_artist_snapshots(&mut attempt).await?;
    system.end(&mut attempt).await?;
    let created = match new_track(&mut attempt, store, file, options, dry_run, Some(file_id)).await {
        Ok(report) => report,
        // E.g. tags too poor for a track of its own: then it's a plain duplicate. Metadata
        // given whole that's refused is the description's mistake, though: said as such.
        Err(err) => {
            attempt.rollback().await?;
            if options.hints.described.is_some() && is_refusal(&err) {
                return Err(err);
            }
            return Ok(None);
        }
    };
    let Some(new) = created.track_id else {
        attempt.rollback().await?;
        return Ok(None);
    };

    // What the new track has that the stored one doesn't.
    let system = db::as_system(&mut attempt).await?;
    let mut added = Vec::new();
    let albums = sqlx::query!(
        r#"
        SELECT a.title, a.kind FROM album_tracks n JOIN albums a ON a.id = n.album_id
        WHERE n.track_id = $1
          AND NOT EXISTS (SELECT 1 FROM album_tracks e WHERE e.track_id = $2 AND e.album_id = n.album_id)
        "#,
        new,
        existing
    )
    .fetch_all(&mut *attempt)
    .await?;
    for album in albums {
        let kind = if album.kind == "single" { "single" } else { "album" };
        added.push(format!("on the {kind} \"{}\"", album.title));
    }
    // Metadata the track has never had (in any revision of any variant). Only when it
    // comes from elsewhere than the file (a tagged copy, values given by hand): the file's
    // own tags are the same bytes as before, so a difference there would only come from
    // how tags are read now, or from edits made since.
    let metadata_from_elsewhere = options.tags_from.is_some()
        || options.hints.described.is_some()
        || options.given.title.is_some()
        || !options.given.artists.is_empty();
    let mut new_metadata = false;
    if metadata_from_elsewhere {
        if let Some(metadata) = db::revisions::primary_metadata(&mut attempt, new).await? {
            if !db::revisions::metadata_is_known(&mut attempt, existing, &metadata).await? {
                new_metadata = true;
                added.push("its tags' metadata, which differs (as another variant)".to_string());
            }
        }
    }
    let covers = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!" FROM covers d
        WHERE d.track_id = $1 AND d.trashed_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM covers k WHERE k.track_id = $2 AND k.trashed_at IS NULL
                                                 AND k.blob_sha256 = d.blob_sha256)
        "#,
        new,
        existing
    )
    .fetch_one(&mut *attempt)
    .await?;
    if covers > 0 {
        added.push(format!("{covers} cover{}", if covers == 1 { "" } else { "s" }));
    }
    let lyrics = sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!" FROM lyrics n
        WHERE n.track_id = $1 AND NOT n.removed
          AND NOT EXISTS (SELECT 1 FROM lyrics e WHERE e.track_id = $2 AND e.body = n.body AND NOT e.removed)
        "#,
        new,
        existing
    )
    .fetch_one(&mut *attempt)
    .await?;
    if lyrics > 0 {
        added.push("lyrics".to_string());
    }
    if created.album_covers_added > 0 {
        added.push(format!("{} album cover{}", created.album_covers_added, if created.album_covers_added == 1 { "" } else { "s" }));
    }
    if db::ingest::album_and_artist_snapshots(&mut attempt).await? > snapshots_before {
        added.push("ids for its artists or album".to_string());
    }
    system.end(&mut attempt).await?;

    if added.is_empty() {
        attempt.rollback().await?;
        return Ok(None);
    }
    let reason = format!(
        "imported again as \"{}\" (the same file): {}",
        options.uploaded_filename,
        added.join(", ")
    );
    let picks = db::revisions::MergePicks { leave_identical: true, leave_variants: !new_metadata, ..Default::default() };
    let merged = db::revisions::merge_tracks(&mut attempt, existing, new, &picks, None, None, &reason).await?;
    attempt.commit().await?;
    added.push(format!("(merge {}, can be undone)", merged.merge_id));
    Ok(Some(Report {
        status: ReportStatus::Duplicate,
        track_id: Some(existing),
        same_song_as: Vec::new(),
        artists: created.artists,
        album: created.album,
        album_covers_added: created.album_covers_added,
        warnings: created.warnings,
        added,
        collectives: created.collectives,
    }))
}

/// Steps 2 to 4 of `ingest_in`: a new track for the file. `shared_file`: the file is
/// already stored (this id); the new track plays that one (see `merge_duplicate`).
async fn new_track(
    conn: &mut PgConnection,
    store: &BlobStore,
    file: &StagedFile,
    options: &IngestOptions,
    dry_run: bool,
    shared_file: Option<Uuid>,
) -> anyhow::Result<Report> {
    // 2. Probe and read tags, and make sure the tags are good enough.
    let (probe, mut tags) = read_file_info(&file.path, &options.uploaded_filename).await?;
    // With a tagged copy, its tags are the metadata; the file's own stay in its raw dump,
    // as do its audio properties.
    let mut copy_tags = None;
    if let Some(copy) = &options.tags_from {
        let (_, from_copy) = read_file_info(&copy.path, &copy.filename).await?;
        let own = std::mem::take(&mut tags);
        copy_tags = Some(json!({ "filename": copy.filename, "tags": from_copy.raw }));
        tags = TagInfo {
            raw: own.raw,
            bitrate_kbps: own.bitrate_kbps,
            duration_ms: own.duration_ms,
            // The file's own original filename if it says one, else the copy's.
            original_filename: own.original_filename.or(from_copy.original_filename.clone()),
            ..from_copy
        };
    }
    // All of the metadata from the description: the file's tags are used for nothing. They
    // stay in `raw`, kept with the file as always; the audio's own facts are the file's.
    let described_metadata = options.hints.described.as_ref();
    if let Some(d) = described_metadata {
        if options.tags_from.is_some() {
            return Err(Rejected::Invalid("a tagged copy isn't used with `metadata` (the metadata is given)".to_string()).into());
        }
        tags = TagInfo {
            raw: std::mem::take(&mut tags.raw),
            bitrate_kbps: tags.bitrate_kbps,
            duration_ms: tags.duration_ms,
            title: Some(d.title.clone()),
            artists: d.credits.iter().filter(|c| c.role == "primary").map(|c| c.name.clone()).collect(),
            album: d.album.as_ref().map(|a| a.title.clone()),
            disc_number: d.album.as_ref().and_then(|a| a.disc_number).and_then(|n| u32::try_from(n).ok()),
            date: d.release_date.clone(),
            genres: d.genres.clone(),
            comment: d.comment.clone(),
            original_filename: options.hints.original_filename.clone(),
            ..TagInfo::default()
        };
    }
    let mut given = Vec::new();
    if let Some(title) = options.given.title.as_ref().filter(|t| !t.trim().is_empty()) {
        tags.title = Some(title.clone());
        given.push("title");
    }
    let given_artists = tags::split_artists(options.given.artists.clone());
    if !given_artists.is_empty() {
        tags.artists = given_artists;
        given.push("artist");
    }
    // The original file's name from the description, only if the tags have none.
    if tags.original_filename.is_none() {
        tags.original_filename = options.hints.original_filename.clone();
    }
    // A date from the description: only if the tags have none, unless it's meant to win.
    let tag_date = tags.date.clone();
    let dated_by_hint = options.hints.release_date.is_some()
        && (tags.date.is_none() || options.hints.release_date_over_tags)
        && tags.date != options.hints.release_date;
    if dated_by_hint {
        tags.date = options.hints.release_date.clone();
    }
    // Values given with the description (see `describe::GivenValues`): a title where the
    // tags have none; artists where they have none, or always if they're meant to win.
    let described = &options.hints.given;
    let mut described_notes = Vec::new();
    if let Some(title) = described.title.as_ref().filter(|t| !t.trim().is_empty()) {
        if tags.title.is_none() {
            tags.title = Some(title.clone());
            described_notes.push(format!("title from {}", described.title_from));
        }
    }
    let described_artists = tags::split_artists(described.artists.clone());
    if !described_artists.is_empty()
        && (tags.artists.is_empty() || described.artists_over_tags)
        && tags.artists != described_artists
    {
        let over = !tags.artists.is_empty();
        tags.artists = described_artists.clone();
        // The album is theirs too (its tags' ALBUM ARTIST would be the same wrong name).
        if described.artists_over_tags && !tags.album_artists.is_empty() {
            tags.album_artists = described_artists;
        }
        described_notes.push(format!("artist from {}{}", described.artists_from, if over { ", over the tags" } else { "" }));
    }
    let missing: Vec<&'static str> = [
        ("title", tags.title.is_none()),
        ("artist", tags.artists.is_empty()),
    ]
    .into_iter()
    .filter_map(|(name, is_missing)| is_missing.then_some(name))
    .collect();
    if !missing.is_empty() {
        return Err(Rejected::MissingTags(missing).into());
    }
    let album_covers = checked_album_covers(options)?;

    // 3. Store the pictures as blobs (the audio already is, see `StagedFile`). A dry run
    //    only hashes them.
    let sha256 = file.sha256.clone();
    let size_bytes = file.size as i64;
    // The same image only once.
    let mut covers: Vec<TrackCover> = Vec::new();
    if !options.hints.skip_embedded_covers {
        for picture in &tags.pictures {
            let sha = put_image(store, &picture.data, dry_run).await?;
            if !covers.iter().any(|c| c.sha256 == sha) {
                let image = crate::model::image_info(&picture.data);
                covers.push(TrackCover {
                    sha256: sha,
                    mime: picture.mime.clone(),
                    width: image.as_ref().and_then(|i| i.width),
                    height: image.as_ref().and_then(|i| i.height),
                    size: picture.data.len() as i64,
                    source: "embedded",
                });
            }
        }
    }
    for (n, bytes) in options.hints.track_covers.iter().enumerate() {
        let Some(image) = crate::model::image_info(bytes) else {
            return Err(Rejected::Invalid(format!("track cover {}: not a JPEG, PNG, WebP or GIF image", n + 1)).into());
        };
        let sha = put_image(store, bytes, dry_run).await?;
        if !covers.iter().any(|c| c.sha256 == sha) {
            covers.push(TrackCover {
                sha256: sha,
                mime: Some(image.mime.to_string()),
                width: image.width,
                height: image.height,
                size: image.size,
                source: "description",
            });
        }
    }
    let mut album_cover_blobs = Vec::new();
    for (bytes, image) in album_covers {
        album_cover_blobs.push((put_image(store, bytes, dry_run).await?, image));
    }

    // 4. Creating a track only adds rows, but it looks across the whole library: for
    //    artists, albums and platform ids that already exist (also ones only on tracks the
    //    uploader can't see), and for similar titles to flag. So it runs as the system
    //    (see `db::as_system`); what the uploader gets back is filtered afterwards.
    let system = db::as_system(conn).await?;
    // The first revision says where its values came from.
    let variant_label = match (described_metadata.is_some(), given.is_empty()) {
        (false, true) => "File tags",
        (false, false) => "File tags + given by hand",
        (true, true) => "From the description",
        (true, false) => "From the description + given by hand",
    }
    .to_string();
    let mut revision_reason = if described_metadata.is_some() {
        "given with the upload (the file's own tags weren't used; they're kept with the file)".to_string()
    } else {
        "imported_from_file".to_string()
    };
    if !given.is_empty() {
        revision_reason += &format!("; given by hand (not from the tags): {}", given.join(", "));
    }
    if let Some(copy) = &options.tags_from {
        revision_reason += &format!("; metadata from the tags of a copy ({}), not the stored file's", copy.filename);
    }
    if !described_notes.is_empty() {
        revision_reason += &format!("; given with the description: {}", described_notes.join(", "));
    }
    if shared_file.is_some() {
        revision_reason += "; the same file as a track already in the library (plays that file)";
    }
    if dated_by_hint {
        revision_reason += &match &tag_date {
            None => "; release date from the description (the tags have none)".to_string(),
            Some(date) => format!("; release date from the description (the tags say {date})"),
        };
    }
    let new_file = NewFileInfo {
        volume: &file.volume,
        cover_volume: store.default_volume(),
        variant_label: &variant_label,
        revision_reason: &revision_reason,
        sha256: &sha256,
        size_bytes,
        probe: &probe,
        tags: &tags,
        copy_tags: copy_tags.as_ref(),
        shared_file,
    };
    let mut report = create_track_rows(conn, options, &new_file, &covers, &album_cover_blobs).await?;
    system.end(conn).await?;
    describe::hide_invisible(conn, &mut report).await?;
    Ok(report)
}

/// The description's album covers, with what kind of image each is. Refuses the file if
/// one isn't an image.
fn checked_album_covers(options: &IngestOptions) -> Result<Vec<(&Vec<u8>, ImageInfo)>, Rejected> {
    let Some(album) = &options.hints.album else {
        return Ok(Vec::new());
    };
    let mut covers = Vec::new();
    for (n, bytes) in album.covers.iter().enumerate() {
        let Some(image) = crate::model::image_info(bytes) else {
            return Err(Rejected::Invalid(format!(
                "album cover {}: not a JPEG, PNG, WebP or GIF image",
                n + 1
            )));
        };
        covers.push((bytes, image));
    }
    Ok(covers)
}

/// For a file that's already stored: adds the description's album covers to its album,
/// if that album is known by id (see `describe::known_album`). So running an import again
/// with covers adds the ones missing the first time. Returns how many were added.
async fn covers_for_known_album(
    conn: &mut PgConnection,
    store: &BlobStore,
    options: &IngestOptions,
    dry_run: bool,
) -> anyhow::Result<usize> {
    let covers = checked_album_covers(options)?;
    let Some(hint) = &options.hints.album else {
        return Ok(0);
    };
    if covers.is_empty() {
        return Ok(0);
    }
    let Some(album_id) = describe::known_album(conn, hint).await? else {
        return Ok(0);
    };
    let mut blobs = Vec::new();
    for (bytes, image) in covers {
        blobs.push((put_image(store, bytes, dry_run).await?, image));
    }
    describe::add_album_covers(conn, album_id, &blobs, store.default_volume()).await
}

/// Stores an image as a blob and returns its sha256. In a dry run, only hashes it.
async fn put_image(store: &BlobStore, bytes: &[u8], dry_run: bool) -> anyhow::Result<String> {
    if dry_run {
        Ok(crate::storage::sha256_hex(bytes))
    } else {
        store.put_bytes(bytes).await
    }
}

/// If these bytes are already stored: adds the origin and the given sources to the track
/// that has them, and reports a duplicate. The stored file keeps the origin it was first
/// uploaded with. None if the file is new.
///
/// As the system: the file can only be stored once, whoever can see it, and where it came
/// from is worth keeping even when the track is one the uploader can't see. Nothing about
/// it is returned then (the id is left out).
async fn duplicate_in(
    conn: &mut PgConnection,
    sha256: &str,
    options: &IngestOptions,
) -> anyhow::Result<Option<Report>> {
    let system = db::as_system(conn).await?;
    let Some(existing) = db::ingest::find_file_by_sha256(conn, sha256).await? else {
        system.end(conn).await?;
        return Ok(None);
    };
    let Some(track_id) = existing.track_id else {
        return Err(Rejected::AlreadyStored { track_id: None }.into());
    };
    db::sources::add_snapshot(conn, SourceOwner::Track(track_id), &options.origin).await?;
    for source in &options.sources {
        db::sources::add_snapshot(conn, SourceOwner::Track(track_id), source).await?;
    }
    db::problems::resolve_imported(conn, sha256, track_id, "already in the library").await?;
    if let Some(path) = &options.local_path {
        db::ingest::fill_in_imported_from(conn, sha256, path).await?;
    }
    system.end(conn).await?;
    let visible = db::sources::owner_exists(conn, SourceOwner::Track(track_id)).await?;
    Ok(Some(Report::duplicate(visible.then_some(track_id))))
}

#[derive(Debug, Clone)]
pub struct ReplaceOptions {
    /// Who is replacing the file. None = the system (CLI).
    pub user: Option<Uuid>,
    pub uploaded_filename: String,
    /// Where the new file came from (required, see `IngestOptions::origin`).
    pub origin: ValidSource,
    /// Why the file was replaced, e.g. "upgrade: 320k mp3 -> flac".
    pub reason: String,
    /// Which storage volume the new file goes to (None: the default one).
    pub volume: Option<String>,
    /// The file is an original file (an og file) with this name, when its tags don't say
    /// (said by whoever adds it, as a description's `original_filename` hint).
    pub original_filename: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ReplaceOutcome {
    pub old_file_id: Uuid,
    pub new_file_id: Uuid,
}

/// Replaces a track's file (e.g. with a higher quality version). The old file stays
/// stored and is recorded in `track_file_history`. The track's metadata, credits,
/// relations etc. are untouched; the new file's own tags are kept in its `files` row.
pub async fn replace_file(
    pool: &PgPool,
    store: &BlobStore,
    track_id: Uuid,
    pending: PendingBlob,
    options: ReplaceOptions,
) -> anyhow::Result<ReplaceOutcome> {
    // Check before storing anything, so a typo in the id doesn't leave a stray blob.
    if let Some(existing) = find_file(pool, options.user, &pending.sha256).await? {
        let track_id = match existing.track_id {
            Some(id) => visible(pool, options.user, id).await?,
            None => None,
        };
        return Err(Rejected::AlreadyStored { track_id }.into());
    }
    if visible(pool, options.user, track_id).await?.is_none() {
        return Err(Rejected::TrackNotFound(track_id).into());
    }
    let volume = options.volume.as_deref().unwrap_or(store.default_volume());
    let file = StagedFile::new(store, pending, false, volume).await?;
    add_file_to_track(pool, &file, track_id, AddAs::CurrentFile, &options).await
}

/// How a file is added to an existing track (see `add_file_to_track`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddAs {
    /// It becomes the track's file; the one it had becomes a previous file (an upgrade).
    CurrentFile,
    /// It's kept as one of the track's previous files; the current file stays.
    PreviousFile,
}

/// Adds a stored file to an existing track: as its new file (like `replace_file`), or as a
/// previous file. Its own tags are kept on its `files` row; the track's metadata doesn't
/// change. Used for replacing files, and for files that didn't import because their tags
/// lack things, when they belong to a track that's already there.
pub async fn add_file_to_track(
    pool: &PgPool,
    file: &StagedFile,
    track_id: Uuid,
    add_as: AddAs,
    options: &ReplaceOptions,
) -> anyhow::Result<ReplaceOutcome> {
    if let Some(existing) = find_file(pool, options.user, &file.sha256).await? {
        let track_id = match existing.track_id {
            Some(id) => visible(pool, options.user, id).await?,
            None => None,
        };
        return Err(Rejected::AlreadyStored { track_id }.into());
    }
    let (probe, mut tags) = read_file_info(&file.path, &options.uploaded_filename).await?;
    if tags.original_filename.is_none() {
        tags.original_filename = options.original_filename.clone();
    }

    let mut tx = db::begin(pool, options.user).await?;
    // Locking the row means two changes to the same track's files can't interleave.
    let Some(current_file_id) = db::ingest::lock_track_file(&mut tx, track_id).await? else {
        return Err(Rejected::TrackNotFound(track_id).into());
    };
    let origin =
        db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), &options.origin).await?;
    let new_file_id = Uuid::now_v7();
    insert_file_row(
        &mut tx,
        new_file_id,
        origin.snapshot_id,
        &options.uploaded_filename,
        &file.sha256,
        file.size as i64,
        &probe,
        &tags,
        None, // a replacement or a file added by hand: no path to record
        None,
    )
    .await?;
    db::blobs::register(&mut tx, &file.sha256, &file.volume, Some(file.size as i64)).await?;
    let resolution = match add_as {
        AddAs::CurrentFile => {
            db::ingest::replace_track_file(&mut tx, track_id, current_file_id, new_file_id, &options.reason)
                .await?;
            "added to a track as its file"
        }
        AddAs::PreviousFile => {
            db::ingest::add_previous_file(&mut tx, track_id, new_file_id, current_file_id, &options.reason)
                .await?;
            "added to a track as a previous file"
        }
    };
    // If it was listed as not imported, it's in now. (As the system: the entry may be
    // someone else's.)
    let system = db::as_system(&mut tx).await?;
    db::problems::resolve_imported(&mut tx, &file.sha256, track_id, resolution).await?;
    system.end(&mut tx).await?;
    tx.commit().await?;

    Ok(ReplaceOutcome {
        old_file_id: current_file_id,
        new_file_id,
    })
}

/// Whether these exact bytes are already stored, as any track's file, including tracks
/// the user can't see (as the system: the file can only be stored once).
async fn find_file(
    pool: &PgPool,
    user: Option<Uuid>,
    sha256: &str,
) -> anyhow::Result<Option<StoredFile>> {
    let mut tx = db::begin(pool, user).await?;
    let system = db::as_system(&mut tx).await?;
    let found = db::ingest::find_file_by_sha256(&mut tx, sha256).await?;
    system.end(&mut tx).await?;
    Ok(found)
}

/// `Some(track_id)` if the user can see the track, else None.
async fn visible(pool: &PgPool, user: Option<Uuid>, track_id: Uuid) -> anyhow::Result<Option<Uuid>> {
    let mut tx = db::begin(pool, user).await?;
    let exists = db::sources::owner_exists(&mut tx, SourceOwner::Track(track_id)).await?;
    Ok(exists.then_some(track_id))
}

/// Probes and reads tags on tokio's blocking thread pool (both are blocking,
/// CPU/disk-heavy library calls that would otherwise stall the async runtime).
/// Fails only if neither symphonia nor lofty recognizes the file.
async fn read_file_info(
    path: &Path,
    uploaded_filename: &str,
) -> anyhow::Result<(ProbeInfo, TagInfo)> {
    let path: PathBuf = path.to_path_buf();
    let extension = file_extension(uploaded_filename);

    tokio::task::spawn_blocking(move || {
        let probe = probe::probe_file(&path, extension.as_deref());
        let tags = tags::read_tags(&path, extension.as_deref());
        match (probe, tags) {
            (Err(probe_err), Err(tag_err)) => {
                Err(Rejected::NotAudio(format!("{probe_err}; {tag_err}")).into())
            }
            (probe, tags) => {
                let probe = probe.unwrap_or_else(|err| {
                    tracing::warn!("probing failed, storing without technical info: {err:#}");
                    ProbeInfo::default()
                });
                let tags = tags.unwrap_or_else(|err| {
                    tracing::warn!("reading tags failed: {err:#}");
                    TagInfo::default()
                });
                Ok((probe, tags))
            }
        }
    })
    .await?
}

// Many arguments, but each is a distinct piece of the file row; a struct wouldn't be clearer.
#[allow(clippy::too_many_arguments)]
async fn insert_file_row(
    conn: &mut PgConnection,
    file_id: Uuid,
    origin_snapshot_id: Uuid,
    uploaded_filename: &str,
    sha256: &str,
    size_bytes: i64,
    probe: &ProbeInfo,
    tags: &TagInfo,
    imported_from: Option<&str>,
    copy_tags: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    // `original_filename` is set only for original files ("og files", the file as the
    // artist named it), and is their name. The old tag-based library kept that name in an
    // ORIGINALFILENAME-like tag, so that's where it's read from (or, without one, from the
    // description; see `ingest_in`). It's never filled in from the name a file was
    // uploaded with (that's `uploaded_filename`).
    let original_filename = tags.original_filename.as_deref();
    let mut embedded_tags = json!({ "tags": tags.raw });
    if let Some(copy) = copy_tags {
        embedded_tags["tags_from_copy"] = copy.clone();
    }

    db::ingest::insert_file(
        conn,
        &db::ingest::NewFile {
            id: file_id,
            origin_snapshot_id,
            sha256,
            original_filename,
            uploaded_filename,
            size_bytes,
            container: probe.container.as_deref(),
            codec: probe.codec.as_deref(),
            bitrate: tags.bitrate_kbps.and_then(|b| i32::try_from(b).ok()),
            sample_rate: probe.sample_rate.and_then(|r| i32::try_from(r).ok()),
            channels: probe.channels.and_then(|c| i16::try_from(c).ok()),
            bit_depth: probe.bit_depth.and_then(|b| i16::try_from(b).ok()),
            duration_ms: probe
                .duration_ms
                .or(tags.duration_ms)
                .and_then(|d| i64::try_from(d).ok()),
            embedded_tags: &embedded_tags,
            imported_from,
        },
    )
    .await?;
    Ok(())
}

/// The new file, as read and stored.
struct NewFileInfo<'a> {
    /// The volume the audio is on, and the one covers go to.
    volume: &'a str,
    cover_volume: &'a str,
    /// The first variant's label and the first revision's reason.
    variant_label: &'a str,
    revision_reason: &'a str,
    sha256: &'a str,
    size_bytes: i64,
    probe: &'a ProbeInfo,
    tags: &'a TagInfo,
    /// The raw tags of the copy the metadata was read from, if any (see `TagsFrom`).
    copy_tags: Option<&'a serde_json::Value>,
    /// The file is already stored, as this row: the track plays it too (no new file row).
    shared_file: Option<Uuid>,
}

/// Creates the track and everything around it. `covers`: the embedded pictures' blobs;
/// `album_covers`: the description's album covers.
async fn create_track_rows(
    conn: &mut PgConnection,
    options: &IngestOptions,
    file: &NewFileInfo<'_>,
    covers: &[TrackCover],
    album_covers: &[(String, ImageInfo)],
) -> anyhow::Result<Report> {
    let tags = file.tags;
    let mut resolver = Resolver::default();
    // With `lax_artist_names`, a hint for a name the tags don't have may go to the one tag
    // artist without a hint (see `describe::lax_artist_names`).
    // Ids for the first ARTIST, whatever its name (see `Hints::first_artist_sources`).
    let first;
    let hints = match tags.artists.first() {
        Some(name) if !options.hints.first_artist_sources.is_empty() => {
            first = describe::with_first_artist(&options.hints, name);
            &first
        }
        _ => &options.hints,
    };
    let lax;
    let hints = if hints.lax_artist_names {
        let tag_artists: Vec<String> = tags.artists.iter().chain(&tags.album_artists).cloned().collect();
        let note;
        (lax, note) = describe::lax_artist_names(hints, &tag_artists);
        resolver.warnings.extend(note);
        &lax
    } else {
        hints
    };
    // The file row is written last (see the comment on tracks.file_id in the schema),
    // so its id is generated up front. A file already stored is played as it is.
    let file_id = file.shared_file.unwrap_or_else(Uuid::now_v7);

    // Credits: one per artist in the ARTIST tag (see `tags::split_artists`). A value like
    // "A feat. B" is NOT split; it's one credit. Fixing that is an edit.
    //
    // On an album, primary are the artists also in ALBUM ARTIST (whose album it is); the
    // others are featured. If none are in both (e.g. ALBUM ARTIST "Various Artists"), the
    // album artist says nothing about this track, and all are primary. Names are compared
    // ignoring case, like artist matching.
    //
    // On a single, the first artist is primary and the others featured, whatever ALBUM
    // ARTIST says (NetEase lists everyone there on singles). A single is what the
    // description (or the folders, see `layouts::release_kind`) says, or else a track
    // count of 1 ("1/1"). Anything else is an album.
    let single = hints
        .album
        .as_ref()
        .and_then(|a| a.single)
        .unwrap_or(tags.track_total == Some(1));
    let album_artist_names = if tags.album_artists.is_empty() {
        &tags.artists
    } else {
        &tags.album_artists
    };
    let on_album = |name: &String| album_artist_names.iter().any(|a| a.to_lowercase() == name.to_lowercase());
    let any_on_album = tags.artists.iter().any(on_album);
    let mut credits: Vec<Credit> = Vec::new();
    let mut ambiguous = Vec::new();
    // Given with the description: its credits, in its roles and order.
    let described_credits = hints.described.as_ref().map(|d| describe::described_credits(d, &tags.artists));
    for credit in described_credits.iter().flatten() {
        let hint = credit.artist.and_then(|i| hints.artists.get(i));
        if resolver.collective_for(conn, hint, &credit.name).await?.is_some() {
            continue;
        }
        let artist = resolver.artist_for(conn, hint, &credit.name).await?;
        if credits.iter().any(|c| c.artist_id == artist.id && c.role == credit.role) {
            continue;
        }
        if !artist.ambiguous.is_empty() {
            ambiguous.push((credit.name.clone(), artist.id, artist.ambiguous));
        }
        credits.push(Credit { artist_id: artist.id, role: credit.role.clone(), alias_id: artist.alias_id });
    }
    // Else from the tags.
    let tag_artists: &[String] = if described_credits.is_some() { &[] } else { &tags.artists };
    for name in tag_artists {
        // A collective (its account): the track goes in its grouping, no credit.
        if resolver.collective(conn, hints, name).await?.is_some() {
            continue;
        }
        let artist = resolver.artist(conn, hints, name).await?;
        // The same artist twice in the tag would be a duplicate credit.
        if credits.iter().any(|c| c.artist_id == artist.id) {
            continue;
        }
        if !artist.ambiguous.is_empty() {
            ambiguous.push((name.clone(), artist.id, artist.ambiguous));
        }
        let primary = if single {
            credits.is_empty()
        } else {
            !any_on_album || on_album(name)
        };
        let role = if primary { "primary" } else { "featured" };
        // Credited under the alias with the tag's exact spelling, if it's not the name.
        credits.push(Credit {
            artist_id: artist.id,
            role: role.to_string(),
            alias_id: artist.alias_id,
        });
    }

    // COMPOSER: producer credits, after the artists. Someone already credited (e.g. a
    // self-produced track) is credited again as producer, since that's also true.
    let composers: &[String] = if hints.no_composers || described_credits.is_some() { &[] } else { &tags.composers };
    for name in composers {
        if resolver.collective(conn, hints, name).await?.is_some() {
            continue;
        }
        let artist = resolver.artist(conn, hints, name).await?;
        if credits.iter().any(|c| c.artist_id == artist.id && c.role == "producer") {
            continue;
        }
        if !artist.ambiguous.is_empty() {
            ambiguous.push((name.clone(), artist.id, artist.ambiguous));
        }
        credits.push(Credit {
            artist_id: artist.id,
            role: "producer".to_string(),
            alias_id: artist.alias_id,
        });
    }

    // Revision 0: exactly what the tags said. (Title presence was checked by the caller.)
    let metadata = TrackMetadata {
        title: tags.title.clone().context("title is required")?,
        release_date: tags.date.clone(),
        genres: tags.genres.clone(),
        comment: tags.comment.clone(),
        credits,
    };

    // Album. Its artists come from the ALBUM ARTIST tag, falling back to the track's.
    let mut albums = Vec::new();
    let mut album_groupings: Vec<Uuid> = Vec::new();
    let mut album_id = None;
    if let Some(album_title) = &tags.album {
        // The album's artists and their roles: the description's, else from ALBUM ARTIST.
        let described_album = hints.described.as_ref().and_then(|d| d.album.as_ref());
        // (name, who its hint says it is, role).
        let named: Vec<(String, Option<&describe::ArtistHint>, Option<String>)> = match described_album {
            Some(album) => album
                .artists
                .iter()
                .map(|c| (c.name.clone(), c.artist.and_then(|i| hints.artists.get(i)), Some(c.role.clone())))
                .collect(),
            None => album_artist_names.iter().map(|name| (name.clone(), describe::hint_named(hints, name), None)).collect(),
        };
        let mut found = Vec::new();
        for (name, hint, role) in named {
            // A collective's album: in its grouping (below), not credited to it.
            match resolver.collective_for(conn, hint, &name).await? {
                Some(grouping) => album_groupings.push(grouping),
                None => found.push((resolver.artist_for(conn, hint, &name).await?, role)),
            }
        }
        let mut album_artists: Vec<(db::ingest::FoundArtist, String)> = match described_album {
            Some(_) => found.into_iter().map(|(artist, role)| (artist, role.unwrap_or_else(|| "primary".to_string()))).collect(),
            None => db::ingest::album_roles_from_tags(&found.into_iter().map(|(a, _)| a).collect::<Vec<_>>(), single),
        };
        // Only collectives: the album's artists are the track's primary ones.
        if album_artists.is_empty() {
            album_artists = metadata
                .credits
                .iter()
                .filter(|c| c.role == "primary")
                .map(|c| (db::ingest::FoundArtist { id: c.artist_id, alias_id: c.alias_id, ambiguous: Vec::new() }, "primary".to_string()))
                .collect();
        }
        let kind = match described_album {
            Some(album) => album.kind.clone(),
            None => if single { "single" } else { "album" }.to_string(),
        };
        // The album's own date if the description has one, else the track's.
        let album_date = hints.album.as_ref().and_then(|a| a.release_date.as_deref()).or(tags.date.as_deref());
        let id = resolver
            .album(conn, hints, album_title, &album_artists, &kind, album_date)
            .await?;
        album_id = Some(id);
        albums.push(AlbumMembership {
            album_id: id,
            disc_number: tags.disc_number.and_then(|n| i16::try_from(n).ok()),
            // A position given with the album (e.g. from a NetEase album map) wins over
            // the TRACK tag.
            track_number: hints
                .album
                .as_ref()
                .and_then(|a| a.track_number)
                .or_else(|| tags.track_number.and_then(|n| i16::try_from(n).ok())),
        });
    } else if hints.album.is_some() {
        resolver
            .warnings
            .push("the file has no ALBUM tag, so what was given about the album wasn't used".to_string());
    }
    resolver.check_unused_hints(hints);
    let facts = TrackFacts {
        status: options.status.clone(),
        albums,
        relations: Vec::new(),
    };

    let track_id = db::revisions::create_track(
        conn,
        file_id,
        &metadata,
        &facts,
        file.variant_label,
        file.revision_reason,
    )
    .await?;

    // Into the collectives' groupings (the names that were their accounts).
    let mut collectives: Vec<Uuid> = resolver.collectives.iter().map(|(_, _, g)| *g).collect();
    collectives.sort();
    collectives.dedup();
    for grouping in collectives {
        let reason = "imported: posted under the collective's account";
        let album: Vec<Uuid> = album_id.filter(|_| album_groupings.contains(&grouping)).into_iter().collect();
        db::groupings::add(conn, grouping, &[track_id], &album, reason).await?;
    }

    for (position, cover) in covers.iter().enumerate() {
        let new = db::covers::NewCover {
            blob_sha256: &cover.sha256,
            mime: cover.mime.as_deref(),
            width: cover.width,
            height: cover.height,
            source: cover.source,
        };
        db::ingest::insert_track_cover(conn, track_id, &new, position as i16).await?;
    }
    let mut album_covers_added = 0;
    if let Some(album_id) = album_id {
        album_covers_added = describe::add_album_covers(conn, album_id, album_covers, file.cover_volume).await?;
        // An album this import created, still without covers of its own (none given):
        // the track's, copied, in order. So it shows its own cover rather than borrowing
        // its first track's, which may be another release's (e.g. a single made for a
        // track that came in on an album first, see `merge_duplicate`).
        let created = resolver.report_album.as_ref().is_some_and(|a| a.matched_by == MatchedBy::Created);
        let has_covers = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM covers WHERE album_id = $1 AND trashed_at IS NULL) AS "e!""#,
            album_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if created && !has_covers {
            let track_covers = sqlx::query_scalar!(
                "SELECT id FROM covers WHERE track_id = $1 AND trashed_at IS NULL ORDER BY position, id",
                track_id
            )
            .fetch_all(&mut *conn)
            .await?;
            for cover in track_covers {
                if db::album_merges::copy_if_new(conn, cover, album_id).await?.is_some() {
                    album_covers_added += 1;
                }
            }
        }
    }

    for lyrics in hints.described.iter().flat_map(|d| &d.lyrics) {
        db::revisions::create_lyrics(conn, track_id, "description", lyrics, Source::Import, "given with the upload").await?;
    }
    for body in &tags.lyrics {
        // Whether embedded lyrics are official is unknown, so they start as unofficial.
        let lyrics = LyricsData {
            body: body.clone(),
            ..Default::default()
        };
        db::revisions::create_lyrics(conn, track_id, "embedded", &lyrics, Source::Import, "imported_from_file")
            .await?;
    }

    // The same song on a platform (same song id) already in the library, as another file?
    // Checked before this track gets its own sources.
    let mut same_song_as: Vec<Uuid> = Vec::new();
    let mut same_song_reasons: Vec<String> = Vec::new();
    for source in std::iter::once(&options.origin).chain(&options.sources) {
        let Some(external_id) = &source.external_id else {
            continue;
        };
        let tracks = db::ingest::owners_with_platform_id(
            conn,
            crate::model::sources::Kind::Track,
            &source.platform,
            external_id,
        )
        .await?;
        if !tracks.is_empty() {
            same_song_reasons.push(format!("same {} song id {external_id}", source.platform));
        }
        for id in tracks {
            if !same_song_as.contains(&id) {
                same_song_as.push(id);
            }
        }
    }

    for source in &options.sources {
        db::sources::add_snapshot(conn, SourceOwner::Track(track_id), source).await?;
    }
    let origin =
        db::sources::add_snapshot(conn, SourceOwner::Track(track_id), &options.origin).await?;
    if file.shared_file.is_none() {
        insert_file_row(
            conn,
            file_id,
            origin.snapshot_id,
            &options.uploaded_filename,
            file.sha256,
            file.size_bytes,
            file.probe,
            tags,
            options.local_path.as_deref(),
            file.copy_tags,
        )
        .await?;
        db::blobs::register(conn, file.sha256, file.volume, Some(file.size_bytes)).await?;
    }
    for cover in covers {
        db::blobs::register(conn, &cover.sha256, file.cover_volume, Some(cover.size)).await?;
    }
    // If this file was listed as not imported before (e.g. its tags were fixed since), it
    // is now.
    db::problems::resolve_imported(conn, file.sha256, track_id, "imported").await?;

    // Things a person should look at: an artist name that matched several artists, the
    // same song id as another track, and tracks by the same artists with a similar title.
    // Nothing is merged automatically. (Not for a track playing a stored file: it's about
    // to be merged into the track that has it, see `merge_duplicate`.)
    if file.shared_file.is_some() {
        return Ok(Report {
            status: ReportStatus::Created,
            track_id: Some(track_id),
            same_song_as: Vec::new(),
            artists: resolver.report_artists,
            album: resolver.report_album,
            album_covers_added,
            warnings: resolver.warnings,
            added: Vec::new(),
            collectives: resolver.report_collectives,
        });
    }
    for (name, chosen, artists) in &ambiguous {
        db::review::flag_ambiguous_artist(conn, track_id, name, *chosen, artists).await?;
    }
    if !same_song_as.is_empty() {
        db::review::flag_same_song(conn, track_id, &same_song_as, &same_song_reasons.join(", "))
            .await?;
    }
    let artist_ids: Vec<Uuid> = metadata.credits.iter().map(|c| c.artist_id).collect();
    db::review::flag_possible_duplicates(conn, track_id, &metadata.title, &artist_ids, &same_song_as)
        .await?;

    if let Some(groups) = &options.restrict_to {
        db::access::set(conn, track_id, true, groups, "restricted when uploaded").await?;
    }
    // "Fix later": on the Review page with the importer's note.
    if let Some(note) = &hints.flag {
        db::review::flag_fix_later(conn, track_id, note).await?;
    }

    Ok(Report {
        status: ReportStatus::Created,
        track_id: Some(track_id),
        same_song_as,
        artists: resolver.report_artists,
        album: resolver.report_album,
        album_covers_added,
        warnings: resolver.warnings,
        added: Vec::new(),
        collectives: resolver.report_collectives,
    })
}

fn is_unique_violation(err: &anyhow::Error, constraint: &str) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|e| e.is_unique_violation() && e.constraint() == Some(constraint))
}

