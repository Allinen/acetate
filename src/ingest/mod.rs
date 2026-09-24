//! Turning uploaded files into tracks, and replacing a track's file.
//!
//! Flow: hash (done while receiving, by the blob store) -> skip if already stored ->
//! probe + read tags -> check the tags are good enough -> store blob -> one DB
//! transaction creating file, track, variant, revision 0, credits, album, covers, lyrics.
//!
//! Metadata is taken verbatim from the tags. Nothing is guessed: files without the
//! required tags are rejected instead of being imported with made-up values.

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
use crate::model::TrackMetadata;
use crate::model::sources::{SourceOwner, ValidSource};
use crate::storage::{BlobStore, PendingBlob};
use probe::ProbeInfo;
use tags::TagInfo;

#[derive(Debug, Clone)]
pub struct IngestOptions {
    /// The literal filename the file was uploaded / imported with.
    pub uploaded_filename: String,
    /// Where this file came from, as a source snapshot (e.g. the SoundCloud upload, or a
    /// custom "sent on Discord" source). It's added to the track's sources and the file
    /// points at it.
    pub origin: Option<ValidSource>,
    /// Initial track status, e.g. "released" or "unreleased".
    pub status: String,
    /// External source data for the track (already validated), supplied by the uploader.
    /// Also added when the file turns out to be a duplicate, to the existing track.
    pub sources: Vec<ValidSource>,
}

#[derive(Debug, Clone, Copy)]
pub enum IngestOutcome {
    Created(Uuid),
    /// The exact same file (same sha256) is already stored, for this track.
    Duplicate(Uuid),
}

/// Reasons a file is refused. These are the user's problem (bad input), not server
/// errors, so the API reports them as 4xx and the CLI as "skipped".
#[derive(Debug)]
pub enum Rejected {
    NotAudio(String),
    /// The tags lack fields we require. Lists the missing ones.
    MissingTags(Vec<&'static str>),
    TrackNotFound(Uuid),
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
    // 1. Same bytes already stored? Then there's nothing to do.
    //    Returning early drops `pending`, which deletes the temp file.
    if let Some(existing) = find_file(pool, &pending.sha256).await? {
        let Some(track_id) = existing.track_id else {
            return Err(Rejected::AlreadyStored { track_id: None }.into());
        };
        add_to_existing(pool, &existing, track_id, &options).await?;
        return Ok(IngestOutcome::Duplicate(track_id));
    }

    // 2. Probe and read tags, and make sure the tags are good enough.
    let (probe, tags) = read_file_info(&pending, &options.uploaded_filename).await?;
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

    // 3. Move the audio into the blob store, then store embedded pictures as blobs too.
    let sha256 = pending.sha256.clone();
    let size_bytes = pending.size as i64;
    store.commit(pending).await?;

    let mut covers = Vec::new();
    for picture in &tags.pictures {
        let cover_sha = store.put_bytes(&picture.data).await?;
        covers.push((cover_sha, picture.mime.clone()));
    }

    // 4. Everything in the database happens in one transaction: either the whole track
    //    shows up, or nothing does.
    let mut tx = pool.begin().await?;
    let result = create_track_rows(&mut tx, &options, &sha256, size_bytes, &probe, &tags, &covers)
        .await;
    match result {
        Ok(track_id) => {
            tx.commit().await?;
            Ok(IngestOutcome::Created(track_id))
        }
        Err(err) => {
            drop(tx); // rolls back
            // Two uploads of the same file at the same time: the second one loses the race on
            // the unique sha256 constraint. Treat that as a duplicate rather than an error.
            if is_unique_violation(&err, "files_sha256_key") {
                if let Some(existing) = find_file(pool, &sha256).await? {
                    if let Some(id) = existing.track_id {
                        add_to_existing(pool, &existing, id, &options).await?;
                        return Ok(IngestOutcome::Duplicate(id));
                    }
                }
            }
            Err(err)
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplaceOptions {
    pub uploaded_filename: String,
    /// Where the new file came from (see `IngestOptions::origin`).
    pub origin: Option<ValidSource>,
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
    if let Some(existing) = find_file(pool, &pending.sha256).await? {
        return Err(Rejected::AlreadyStored {
            track_id: existing.track_id,
        }
        .into());
    }
    // Check before storing anything, so a typo in the id doesn't leave a stray blob.
    let track_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1) AS "exists!""#,
        track_id
    )
    .fetch_one(pool)
    .await?;
    if !track_exists {
        return Err(Rejected::TrackNotFound(track_id).into());
    }

    let (probe, tags) = read_file_info(&pending, &options.uploaded_filename).await?;
    let sha256 = pending.sha256.clone();
    let size_bytes = pending.size as i64;
    store.commit(pending).await?;

    let mut tx = pool.begin().await?;
    // Locking the row means two replacements of the same track can't interleave.
    let Some(old_file_id) = db::ingest::lock_track_file(&mut tx, track_id).await? else {
        return Err(Rejected::TrackNotFound(track_id).into());
    };
    let ingest_options = IngestOptions {
        uploaded_filename: options.uploaded_filename.clone(),
        origin: None, // set below, once the file row exists
        status: String::new(), // not used for files
        sources: Vec::new(),
    };
    let new_file_id =
        insert_file_row(&mut tx, &ingest_options, &sha256, size_bytes, &probe, &tags).await?;
    if let Some(origin) = &options.origin {
        let added = db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), origin).await?;
        db::ingest::set_file_origin(&mut tx, new_file_id, added.snapshot_id).await?;
    }
    db::ingest::replace_track_file(&mut tx, track_id, old_file_id, new_file_id, &options.reason)
        .await?;
    tx.commit().await?;

    Ok(ReplaceOutcome {
        old_file_id,
        new_file_id,
    })
}

/// For a duplicate upload: adds the given sources to the existing track, and the origin
/// too. The origin is only recorded as the file's origin if it didn't have one yet;
/// an existing origin is never overwritten.
async fn add_to_existing(
    pool: &PgPool,
    existing: &StoredFile,
    track_id: Uuid,
    options: &IngestOptions,
) -> anyhow::Result<()> {
    if options.sources.is_empty() && options.origin.is_none() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for source in &options.sources {
        db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), source).await?;
    }
    if let Some(origin) = &options.origin {
        let added = db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), origin).await?;
        if existing.origin_snapshot_id.is_none() {
            db::ingest::set_file_origin(&mut tx, existing.file_id, added.snapshot_id).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

async fn find_file(pool: &PgPool, sha256: &str) -> anyhow::Result<Option<StoredFile>> {
    let mut conn = pool.acquire().await?;
    Ok(db::ingest::find_file_by_sha256(&mut conn, sha256).await?)
}

/// Probes and reads tags on tokio's blocking thread pool (both are blocking,
/// CPU/disk-heavy library calls that would otherwise stall the async runtime).
/// Fails only if neither symphonia nor lofty recognizes the file.
async fn read_file_info(
    pending: &PendingBlob,
    uploaded_filename: &str,
) -> anyhow::Result<(ProbeInfo, TagInfo)> {
    let path: PathBuf = pending.path().to_path_buf();
    let extension = Path::new(uploaded_filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

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

async fn insert_file_row(
    conn: &mut PgConnection,
    options: &IngestOptions,
    sha256: &str,
    size_bytes: i64,
    probe: &ProbeInfo,
    tags: &TagInfo,
) -> anyhow::Result<Uuid> {
    // The original filename comes from the file's own tags if it has one there,
    // otherwise it's the name it was uploaded with.
    let original_filename = tags
        .original_filename
        .as_deref()
        .unwrap_or(&options.uploaded_filename);
    let embedded_tags = json!({ "tags": tags.raw });

    let file_id = db::ingest::insert_file(
        conn,
        &db::ingest::NewFile {
            sha256,
            original_filename,
            uploaded_filename: &options.uploaded_filename,
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
    Ok(file_id)
}

async fn create_track_rows(
    conn: &mut PgConnection,
    options: &IngestOptions,
    sha256: &str,
    size_bytes: i64,
    probe: &ProbeInfo,
    tags: &TagInfo,
    covers: &[(String, Option<String>)],
) -> anyhow::Result<Uuid> {
    let file_id = insert_file_row(conn, options, sha256, size_bytes, probe, tags).await?;

    // Revision 0: exactly what the tags said. (Title presence was checked by the caller.)
    let metadata = TrackMetadata {
        title: tags.title.clone().context("title is required")?,
        release_date: tags.date.clone(),
        genres: tags.genres.clone(),
        comment: tags.comment.clone(),
    };
    let track_id = db::ingest::create_track(
        conn,
        file_id,
        &options.status,
        &metadata,
        "File tags",
        "imported_from_file",
    )
    .await?;

    // Credits: one primary credit per artist tag value, exactly as written.
    // A value like "A feat. B" is NOT split; it's one credit. Fixing that is an edit.
    for (position, name) in tags.artists.iter().enumerate() {
        let (artist_id, canonical_name) = db::ingest::find_or_create_artist(conn, name).await?;
        // Remember the exact spelling used on this track if it differs from the artist's name.
        let credited_as = (canonical_name != *name).then_some(name.as_str());
        db::ingest::insert_track_credit(
            conn,
            track_id,
            artist_id,
            "primary",
            credited_as,
            position as i16,
        )
        .await?;
    }
    db::ingest::set_artist_display(conn, track_id, &tags.artists.join(", ")).await?;

    // Album. Album artists come from the album artist tag, falling back to the track's artists.
    if let Some(album_title) = &tags.album {
        let album_artist_names = if tags.album_artists.is_empty() {
            &tags.artists
        } else {
            &tags.album_artists
        };
        let mut album_artist_ids = Vec::new();
        for name in album_artist_names {
            let (artist_id, _) = db::ingest::find_or_create_artist(conn, name).await?;
            album_artist_ids.push(artist_id);
        }
        let album_id = db::ingest::find_or_create_album(
            conn,
            album_title,
            &album_artist_ids,
            tags.date.as_deref(),
        )
        .await?;
        db::ingest::add_album_track(
            conn,
            album_id,
            track_id,
            tags.disc_number.and_then(|n| i16::try_from(n).ok()),
            tags.track_number.and_then(|n| i16::try_from(n).ok()),
        )
        .await?;
    }

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

    for body in &tags.lyrics {
        // Whether embedded lyrics are official is unknown, so they start as unofficial.
        db::ingest::insert_lyrics(conn, track_id, false, looks_synced(body), "embedded", body)
            .await?;
    }

    for source in &options.sources {
        db::sources::add_snapshot(conn, SourceOwner::Track(track_id), source).await?;
    }
    if let Some(origin) = &options.origin {
        let added = db::sources::add_snapshot(conn, SourceOwner::Track(track_id), origin).await?;
        db::ingest::set_file_origin(conn, file_id, added.snapshot_id).await?;
    }

    // TODO(Phase 2): fuzzy-match title/artists against existing tracks (pg_trgm) and flag
    // likely duplicates and artist matches for review.

    Ok(track_id)
}

/// LRC-style synced lyrics have lines like "[01:23.45] text".
fn looks_synced(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with('[') && line[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    })
}

fn is_unique_violation(err: &anyhow::Error, constraint: &str) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|e| e.is_unique_violation() && e.constraint() == Some(constraint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_detection() {
        assert!(looks_synced("[00:12.34] hello\n[00:15.00] world"));
        assert!(!looks_synced("hello\nworld"));
        assert!(!looks_synced("[Chorus]\nla la la"));
    }
}
