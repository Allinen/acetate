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
use crate::db::revisions::Source;
use crate::model::{AlbumMembership, Credit, LyricsData, TrackFacts, TrackMetadata};
use crate::model::sources::{SourceOwner, ValidSource};
use crate::storage::{BlobStore, PendingBlob};
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
    // 1. Same bytes already stored? Then there's nothing to do.
    //    Returning early drops `pending`, which deletes the temp file.
    if let Some(existing) = find_file(pool, options.user, &pending.sha256).await? {
        let Some(track_id) = existing.track_id else {
            return Err(Rejected::AlreadyStored { track_id: None }.into());
        };
        add_to_existing(pool, track_id, &options).await?;
        return Ok(IngestOutcome::Duplicate(visible(pool, options.user, track_id).await?));
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
    let mut tx = db::begin(pool, options.user).await?;
    // Creating a track only adds rows, but it looks across the whole library: for artists
    // and albums that already exist (also ones only on tracks the uploader can't see), and
    // for similar titles to flag. So it runs as the system (see `db::as_system`); all the
    // uploader gets back is the new track's id.
    let system = db::as_system(&mut tx).await?;
    let result = create_track_rows(&mut tx, &options, &sha256, size_bytes, &probe, &tags, &covers)
        .await;
    match result {
        Ok(track_id) => {
            system.end(&mut tx).await?;
            tx.commit().await?;
            Ok(IngestOutcome::Created(track_id))
        }
        Err(err) => {
            drop(tx); // rolls back
            if let Some(db::revisions::EditRejected::Invalid(msg)) = err.downcast_ref() {
                return Err(Rejected::Invalid(msg.clone()).into());
            }
            // Two uploads of the same file at the same time: the second one loses the race on
            // the unique sha256 constraint. Treat that as a duplicate rather than an error.
            if is_unique_violation(&err, "files_sha256_key") {
                if let Some(existing) = find_file(pool, options.user, &sha256).await? {
                    if let Some(id) = existing.track_id {
                        add_to_existing(pool, id, &options).await?;
                        return Ok(IngestOutcome::Duplicate(visible(pool, options.user, id).await?));
                    }
                }
            }
            Err(err)
        }
    }
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

    let (probe, tags) = read_file_info(&pending, &options.uploaded_filename).await?;
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

/// For a duplicate upload: adds the origin and the given sources to the existing track's
/// sources. The stored file keeps the origin it was first uploaded with.
///
/// As the system: where a file came from is worth keeping even when the track is one the
/// uploader can't see. Nothing about it is returned.
async fn add_to_existing(
    pool: &PgPool,
    track_id: Uuid,
    options: &IngestOptions,
) -> anyhow::Result<()> {
    let mut tx = db::begin(pool, options.user).await?;
    let system = db::as_system(&mut tx).await?;
    db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), &options.origin).await?;
    for source in &options.sources {
        db::sources::add_snapshot(&mut tx, SourceOwner::Track(track_id), source).await?;
    }
    system.end(&mut tx).await?;
    tx.commit().await?;
    Ok(())
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
    // The original filename comes from the file's own tags if it has one there,
    // otherwise it's the name it was uploaded with.
    let original_filename = tags
        .original_filename
        .as_deref()
        .unwrap_or(uploaded_filename);
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

async fn create_track_rows(
    conn: &mut PgConnection,
    options: &IngestOptions,
    sha256: &str,
    size_bytes: i64,
    probe: &ProbeInfo,
    tags: &TagInfo,
    covers: &[(String, Option<String>)],
) -> anyhow::Result<Uuid> {
    // The file row is written last (see the comment on tracks.file_id in the schema),
    // so its id is generated up front.
    let file_id = Uuid::now_v7();

    // Credits: one primary credit per artist tag value, exactly as written.
    // A value like "A feat. B" is NOT split; it's one credit. Fixing that is an edit.
    let mut credits: Vec<Credit> = Vec::new();
    let mut ambiguous = Vec::new();
    for name in &tags.artists {
        let artist = db::ingest::find_or_create_artist(conn, name).await?;
        // The same artist twice in the tag would be a duplicate credit.
        if credits.iter().any(|c| c.artist_id == artist.id) {
            continue;
        }
        if !artist.ambiguous.is_empty() {
            ambiguous.push((name.clone(), artist.id, artist.ambiguous));
        }
        // Credited under the alias with the tag's exact spelling, if it's not the name.
        credits.push(Credit {
            artist_id: artist.id,
            role: "primary".to_string(),
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

    // Album. Album artists come from the album artist tag, falling back to the track's artists.
    let mut albums = Vec::new();
    if let Some(album_title) = &tags.album {
        let album_artist_names = if tags.album_artists.is_empty() {
            &tags.artists
        } else {
            &tags.album_artists
        };
        let mut album_artists = Vec::new();
        for name in album_artist_names {
            album_artists.push(db::ingest::find_or_create_artist(conn, name).await?);
        }
        let album_id = db::ingest::find_or_create_album(
            conn,
            album_title,
            &album_artists,
            tags.date.as_deref(),
        )
        .await?;
        albums.push(AlbumMembership {
            album_id,
            disc_number: tags.disc_number.and_then(|n| i16::try_from(n).ok()),
            track_number: tags.track_number.and_then(|n| i16::try_from(n).ok()),
        });
    }
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

    for body in &tags.lyrics {
        // Whether embedded lyrics are official is unknown, so they start as unofficial.
        let lyrics = LyricsData {
            body: body.clone(),
            ..Default::default()
        };
        db::revisions::create_lyrics(conn, track_id, "embedded", &lyrics, Source::Import, "imported_from_file")
            .await?;
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
        sha256,
        size_bytes,
        probe,
        tags,
    )
    .await?;

    // Things a person should look at: an artist name that matched several artists, and
    // tracks by the same artists with a similar title. Nothing is merged automatically.
    for (name, chosen, artists) in &ambiguous {
        db::review::flag_ambiguous_artist(conn, track_id, name, *chosen, artists).await?;
    }
    let artist_ids: Vec<Uuid> = metadata.credits.iter().map(|c| c.artist_id).collect();
    db::review::flag_possible_duplicates(conn, track_id, &metadata.title, &artist_ids).await?;

    if let Some(groups) = &options.restrict_to {
        db::access::set(conn, track_id, true, groups, "restricted when uploaded").await?;
    }

    Ok(track_id)
}

fn is_unique_violation(err: &anyhow::Error, constraint: &str) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .is_some_and(|e| e.is_unique_violation() && e.constraint() == Some(constraint))
}

