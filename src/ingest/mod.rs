//! Turning uploaded files into tracks, and replacing a track's file.
//!
//! Flow: hash (done while receiving, by the blob store) -> skip if already stored ->
//! probe + read tags -> check the tags are good enough -> store blob -> one DB
//! transaction creating file, track, variant, revision 0, credits, album, covers, lyrics.
//!
//! Metadata is taken verbatim from the tags. Nothing is guessed: files without the
//! required tags are rejected instead of being imported with made-up values.

pub mod describe;
pub mod layouts;
pub mod probe;
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
use describe::{Hints, Report, ReportStatus, Resolver};
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
    /// Where the file was on disk (CLI), for the review list if it doesn't import.
    pub local_path: Option<String>,
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

/// Streams a file from disk into a pending blob (hashing it on the way). Used by the CLI.
pub async fn receive_path(store: &BlobStore, path: &Path) -> anyhow::Result<PendingBlob> {
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
    pub async fn new(store: &BlobStore, pending: PendingBlob, dry_run: bool) -> anyhow::Result<StagedFile> {
        let sha256 = pending.sha256.clone();
        let size = pending.size;
        if dry_run {
            let path = pending.path().to_path_buf();
            Ok(StagedFile { sha256, size, path, _temp: Some(pending) })
        } else {
            let path = store.commit(pending).await?;
            Ok(StagedFile { sha256, size, path, _temp: None })
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
    let file = StagedFile::new(store, pending, dry_run).await?;
    let sha256 = file.sha256.clone();
    let mut tx = db::begin(pool, options.user).await?;
    match ingest_in(&mut tx, store, &file, options, dry_run).await {
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
                if let Err(record_err) = record_problem(pool, &file, options, &err).await {
                    tracing::error!("couldn't list {} for review: {record_err:#}", options.uploaded_filename);
                }
            }
            Err(err)
        }
    }
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
    file: &StagedFile,
    options: &IngestOptions,
    err: &anyhow::Error,
) -> anyhow::Result<()> {
    let refused = is_refusal(err);
    let reason = if refused { err.to_string() } else { format!("{err:#}") };
    // The raw tags, to see what's missing. (Not there for files that aren't audio.)
    let path = file.path.clone();
    let extension = file_extension(&options.uploaded_filename);
    let tags = tokio::task::spawn_blocking(move || tags::read_tags(&path, extension.as_deref()))
        .await?
        .ok()
        .map(|t| json!({ "tags": t.raw }));
    let origin = &options.origin;
    let problem = db::problems::NewProblem {
        kind: if refused { "skipped" } else { "failed" },
        sha256: &file.sha256,
        size_bytes: file.size as i64,
        uploaded_filename: &options.uploaded_filename,
        path: options.local_path.as_deref(),
        reason: &reason,
        origin: Some(json!({ "platform": origin.platform, "source_id": origin.source_id,
                             "captured_at": origin.captured_at, "data": origin.data })),
        embedded_tags: tags,
    };
    let mut tx = db::begin(pool, options.user).await?;
    db::problems::record(&mut tx, &problem).await?;
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

    // 1. Same bytes already stored? Then there's nothing to do but note the sources.
    if let Some(mut report) = duplicate_in(conn, &file.sha256, options).await? {
        report.album_covers_added = covers_for_known_album(conn, store, options, dry_run).await?;
        return Ok(report);
    }

    // 2. Probe and read tags, and make sure the tags are good enough.
    let (probe, tags) = read_file_info(&file.path, &options.uploaded_filename).await?;
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
    let mut covers = Vec::new();
    for picture in &tags.pictures {
        covers.push((put_image(store, &picture.data, dry_run).await?, picture.mime.clone()));
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
    let new_file = NewFileInfo {
        sha256: &sha256,
        size_bytes,
        probe: &probe,
        tags: &tags,
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
    describe::add_album_covers(conn, album_id, &blobs).await
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
    if let Some(existing) = find_file(pool, options.user, &pending.sha256).await? {
        let track_id = match existing.track_id {
            Some(id) => visible(pool, options.user, id).await?,
            None => None,
        };
        return Err(Rejected::AlreadyStored { track_id }.into());
    }
    // Check before storing anything, so a typo in the id doesn't leave a stray blob.
    let mut tx = db::begin(pool, options.user).await?;
    let track_exists =
        db::sources::owner_exists(&mut tx, SourceOwner::Track(track_id)).await?;
    drop(tx);
    if !track_exists {
        return Err(Rejected::TrackNotFound(track_id).into());
    }

    let (probe, tags) = read_file_info(pending.path(), &options.uploaded_filename).await?;
    let sha256 = pending.sha256.clone();
    let size_bytes = pending.size as i64;
    store.commit(pending).await?;

    let mut tx = db::begin(pool, options.user).await?;
    // Locking the row means two replacements of the same track can't interleave.
    let Some(old_file_id) = db::ingest::lock_track_file(&mut tx, track_id).await? else {
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
        &sha256,
        size_bytes,
        &probe,
        &tags,
    )
    .await?;
    db::ingest::replace_track_file(&mut tx, track_id, old_file_id, new_file_id, &options.reason)
        .await?;
    tx.commit().await?;

    Ok(ReplaceOutcome {
        old_file_id,
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
) -> anyhow::Result<()> {
    // `original_filename` is set only for original files ("og files", the file as the
    // artist named it), and is their name. The old tag-based library kept that name in an
    // ORIGINALFILENAME-like tag, so that's where it's read from for now. It's never filled
    // in from the name a file was uploaded with (that's `uploaded_filename`).
    let original_filename = tags.original_filename.as_deref();
    let embedded_tags = json!({ "tags": tags.raw });

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
        },
    )
    .await?;
    Ok(())
}

/// The new file, as read and stored.
struct NewFileInfo<'a> {
    sha256: &'a str,
    size_bytes: i64,
    probe: &'a ProbeInfo,
    tags: &'a TagInfo,
}

/// Creates the track and everything around it. `covers`: the embedded pictures' blobs;
/// `album_covers`: the description's album covers.
async fn create_track_rows(
    conn: &mut PgConnection,
    options: &IngestOptions,
    file: &NewFileInfo<'_>,
    covers: &[(String, Option<String>)],
    album_covers: &[(String, ImageInfo)],
) -> anyhow::Result<Report> {
    let tags = file.tags;
    let hints = &options.hints;
    let mut resolver = Resolver::default();
    // The file row is written last (see the comment on tracks.file_id in the schema),
    // so its id is generated up front.
    let file_id = Uuid::now_v7();

    // Credits: one per artist in the ARTIST tag (see `tags::split_artists`). A value like
    // "A feat. B" is NOT split; it's one credit. Fixing that is an edit.
    //
    // On an album, primary are the artists also in ALBUM ARTIST (whose album it is); the
    // others are featured. If none are in both (e.g. ALBUM ARTIST "Various Artists"), the
    // album artist says nothing about this track, and all are primary. Names are compared
    // ignoring case, like artist matching.
    //
    // On a single (see `AlbumHint::single`), the first artist is primary and the others
    // featured, whatever ALBUM ARTIST says (NetEase lists everyone there on singles).
    let single = hints.album.as_ref().is_some_and(|a| a.single);
    let album_artist_names = if tags.album_artists.is_empty() {
        &tags.artists
    } else {
        &tags.album_artists
    };
    let on_album = |name: &String| album_artist_names.iter().any(|a| a.to_lowercase() == name.to_lowercase());
    let any_on_album = tags.artists.iter().any(on_album);
    let mut credits: Vec<Credit> = Vec::new();
    let mut ambiguous = Vec::new();
    for name in &tags.artists {
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
    let mut album_id = None;
    if let Some(album_title) = &tags.album {
        let mut album_artists = Vec::new();
        for name in album_artist_names {
            album_artists.push(resolver.artist(conn, hints, name).await?);
        }
        let id = resolver
            .album(conn, hints, album_title, &album_artists, tags.date.as_deref())
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
        "File tags",
        "imported_from_file",
    )
    .await?;

    for (position, (cover_sha, mime)) in covers.iter().enumerate() {
        db::ingest::insert_track_cover(
            conn,
            track_id,
            cover_sha,
            mime.as_deref(),
            position as i16,
            "embedded",
        )
        .await?;
    }
    let mut album_covers_added = 0;
    if let Some(album_id) = album_id {
        album_covers_added = describe::add_album_covers(conn, album_id, album_covers).await?;
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
    insert_file_row(
        conn,
        file_id,
        origin.snapshot_id,
        &options.uploaded_filename,
        file.sha256,
        file.size_bytes,
        file.probe,
        tags,
    )
    .await?;
    // If this file was listed as not imported before (e.g. its tags were fixed since), it
    // is now.
    db::problems::resolve_imported(conn, file.sha256, track_id, "imported").await?;

    // Things a person should look at: an artist name that matched several artists, the
    // same song id as another track, and tracks by the same artists with a similar title.
    // Nothing is merged automatically.
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

    Ok(Report {
        status: ReportStatus::Created,
        track_id: Some(track_id),
        same_song_as,
        artists: resolver.report_artists,
        album: resolver.report_album,
        album_covers_added,
        warnings: resolver.warnings,
    })
}

fn is_unique_violation(err: &anyhow::Error, constraint: &str) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|e| e.is_unique_violation() && e.constraint() == Some(constraint))
}

