//! `acetate reread-dates`: release dates of tracks already in the library, read again from
//! their files' tags with today's rules (`ingest::tags::resolve_date`).
//!
//! Imports used to take a date tag as it was, so a file saying only "2021" in its date tag
//! got 2021, even if the rest of the date was elsewhere in the same file: ID3v2.3's TDAT
//! (day and month), a RELEASETIME field, a WAV file's RIFF INFO ICRD. This finds tracks
//! whose date is only a year (or none), and gives them the whole date where their file has
//! it.
//!
//! Only dates that are still what the import took from the tags are changed: never one
//! given with the upload, or changed by anyone since. The new date is a new revision of
//! the "File tags" version (source "import", restorable like any other), so the history
//! says what it was before. A track's single (its only album, with only this track) gets
//! the same date. Everything else is reported, not changed.

use std::collections::{BTreeMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::db::revisions::{self, EditRejected, Source};
use crate::ingest::tags::{self, RawTag};
use crate::model::{AlbumData, TrackMetadata};
use crate::storage::BlobStore;

/// Tracks changed per transaction, so a long run doesn't hold locks the server waits on.
const WRITE_BATCH: usize = 200;
/// Track ids listed per kind of outcome.
const SAMPLES: usize = 10;

pub struct Options {
    /// Work everything out and print it, but change nothing.
    pub dry_run: bool,
}

/// A track whose "File tags" version's date is a year or nothing.
struct Candidate {
    track_id: Uuid,
    variant_id: Uuid,
    first_data: serde_json::Value,
    first_reason: String,
    latest_id: Uuid,
    latest_data: serde_json::Value,
    sha256: String,
    container: Option<String>,
    embedded_tags: serde_json::Value,
}

/// A date to write.
struct Change {
    track_id: Uuid,
    variant_id: Uuid,
    latest_id: Uuid,
    data: TrackMetadata,
    old: Option<String>,
    from: String,
}

/// Why a track keeps its date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kept {
    /// The first revision's date isn't what the tags' date tag says: it was given with the
    /// upload (or read from a copy's tags, or by lofty's generic view).
    NotFromTags,
    /// Changed since the import (an edit, a restore).
    ChangedSince,
    /// The file has nothing more precise.
    NothingBetter,
    /// The file has full dates, but only in another year.
    OtherYear,
    /// The blob to read RIFF INFO from isn't there (e.g. purged).
    BlobMissing,
}

impl Kept {
    fn describe(self) -> &'static str {
        match self {
            Kept::NotFromTags => "date not from the date tags (given with the upload, or from a copy's tags)",
            Kept::ChangedSince => "date changed since the import",
            Kept::NothingBetter => "the file has no more precise date",
            Kept::OtherYear => "the file's full dates are in another year",
            Kept::BlobMissing => "the file's blob isn't there to read RIFF INFO from",
        }
    }
}

#[derive(Default)]
struct Report {
    candidates: usize,
    kept: BTreeMap<Kept, Vec<Uuid>>,
    /// Tracks with nothing better that have a SoundCloud source (their date can come from
    /// there instead).
    nothing_better_soundcloud: usize,
    /// Changes by where the date was read from, and by container.
    changed_from: BTreeMap<String, usize>,
    changed_container: BTreeMap<String, usize>,
    changed_samples: Vec<(Uuid, Option<String>, String)>,
    other_year_samples: Vec<(Uuid, String)>,
    /// TYER+TDAT against a whole date elsewhere in the same file: (agree, disagree samples).
    tdat_agree: usize,
    tdat_disagree: Vec<(Uuid, String, String, String)>,
    tdat_disagree_count: usize,
    written: usize,
    albums_written: usize,
    /// Changed by someone while this ran, so left alone.
    stale: Vec<Uuid>,
}

pub async fn run(pool: &PgPool, store: &BlobStore, options: Options) -> anyhow::Result<()> {
    let candidates = find_candidates(pool).await?;
    let mut report = Report { candidates: candidates.len(), ..Default::default() };
    println!("{} tracks whose \"File tags\" date is a year or none; reading their tags", candidates.len());

    let mut changes = Vec::new();
    let mut nothing_better = Vec::new();
    for candidate in candidates {
        match decide(store, &candidate, &mut report).await? {
            Ok(change) => {
                *report.changed_from.entry(change.from.clone()).or_default() += 1;
                let container = candidate.container.clone().unwrap_or_else(|| "?".to_string());
                *report.changed_container.entry(container).or_default() += 1;
                if report.changed_samples.len() < SAMPLES {
                    report.changed_samples.push((change.track_id, change.old.clone(), change.data.release_date.clone().unwrap_or_default()));
                }
                changes.push(change);
            }
            Err(kept) => {
                if kept == Kept::NothingBetter {
                    nothing_better.push(candidate.track_id);
                }
                report.kept.entry(kept).or_default().push(candidate.track_id);
            }
        }
    }
    report.nothing_better_soundcloud = with_soundcloud_source(pool, &nothing_better).await?;

    for batch in changes.chunks(WRITE_BATCH) {
        let mut tx = db::begin(pool, None).await?;
        for change in batch {
            match write(&mut tx, change).await {
                Ok(album_written) => {
                    report.written += 1;
                    report.albums_written += usize::from(album_written);
                }
                Err(err) if matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::Stale { .. })) => {
                    report.stale.push(change.track_id);
                }
                Err(err) => return Err(err.context(format!("track {}", change.track_id))),
            }
        }
        if options.dry_run {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
    }
    print_report(&report, options.dry_run);
    Ok(())
}

async fn find_candidates(pool: &PgPool) -> anyhow::Result<Vec<Candidate>> {
    // The import's version is the one labelled "File tags" (a version merged in from
    // another track gets " (from ...)" added to its label, so it isn't this track's own),
    // and its file the one the track was imported with: the first one replaced, if any.
    let rows = sqlx::query!(
        r#"
        SELECT t.id AS "track_id!", v.id AS "variant_id!",
               first_rev.data AS "first_data!", first_rev.reason AS "first_reason!",
               latest.id AS "latest_id!", latest.data AS "latest_data!",
               f.sha256 AS "sha256!", f.container, f.embedded_tags AS "embedded_tags!"
        FROM tracks t
        JOIN track_variants v ON v.track_id = t.id AND v.label = 'File tags'
        CROSS JOIN LATERAL (
            SELECT r.data, r.reason FROM track_revisions r WHERE r.variant_id = v.id ORDER BY r.id LIMIT 1
        ) first_rev
        CROSS JOIN LATERAL (
            SELECT r.id, r.data FROM track_revisions r WHERE r.variant_id = v.id ORDER BY r.id DESC LIMIT 1
        ) latest
        JOIN files f ON f.id = COALESCE(
            (SELECT h.old_file_id FROM track_file_history h WHERE h.track_id = t.id ORDER BY h.id LIMIT 1),
            t.file_id
        )
        WHERE t.merged_into IS NULL
          AND (latest.data->>'release_date' IS NULL OR latest.data->>'release_date' ~ '^\d{4}$')
        ORDER BY t.id
        "#
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Candidate {
            track_id: r.track_id,
            variant_id: r.variant_id,
            first_data: r.first_data,
            first_reason: r.first_reason,
            latest_id: r.latest_id,
            latest_data: r.latest_data,
            sha256: r.sha256,
            container: r.container,
            embedded_tags: r.embedded_tags,
        })
        .collect())
}

/// What to do with one track: the change, or why it keeps its date.
async fn decide(store: &BlobStore, c: &Candidate, report: &mut Report) -> anyhow::Result<Result<Change, Kept>> {
    let date_of = |data: &serde_json::Value| data.get("release_date").and_then(|d| d.as_str()).map(str::to_string);
    let first_date = date_of(&c.first_data);
    let latest_date = date_of(&c.latest_data);

    // The first revision's reason says where its values came from (see `ingest::new_track`):
    // only a plain import read its date from the stored file's own tags.
    let from_own_tags = c.first_reason.starts_with("imported_from_file")
        && !c.first_reason.contains("metadata from the tags of a copy")
        && !c.first_reason.contains("release date from the description");
    let mut raw = tags::raw_from_stored(&c.embedded_tags);
    let primary = c.container.as_deref().and_then(tags::primary_format_of_container);
    if !from_own_tags || tags::find_date(&raw, primary) != first_date {
        return Ok(Err(Kept::NotFromTags));
    }
    if latest_date != first_date {
        return Ok(Err(Kept::ChangedSince));
    }

    let mut resolved = tags::resolve_date(&raw, primary);
    // WAV files stored before RIFF INFO was dumped: read it from the blob.
    let is_wav = c.container.as_deref() == Some("wave");
    if resolved.from.is_none() && is_wav && !raw.iter().any(|t| t.format == "riff_info") {
        let Some((_, path)) = store.find(&c.sha256).await else {
            return Ok(Err(Kept::BlobMissing));
        };
        let riff: Option<RawTag> = tokio::task::spawn_blocking(move || tags::read_riff_info(&path)).await??;
        raw.extend(riff);
        resolved = tags::resolve_date(&raw, primary);
    }

    if let Some(year) = &first_date
        && let Some((composed, other, from)) = tags::tdat_and_other_date(&raw, year)
        && let Some(other_day) = tags::day_of(&other)
    {
        if composed == other_day {
            report.tdat_agree += 1;
        } else {
            report.tdat_disagree_count += 1;
            if report.tdat_disagree.len() < SAMPLES {
                report.tdat_disagree.push((c.track_id, composed, from, other));
            }
        }
    }

    let (Some(date), Some(from)) = (resolved.date, resolved.from) else {
        if !resolved.conflicts.is_empty() {
            if report.other_year_samples.len() < SAMPLES {
                report.other_year_samples.push((c.track_id, resolved.conflicts.join(", ")));
            }
            return Ok(Err(Kept::OtherYear));
        }
        return Ok(Err(Kept::NothingBetter));
    };
    let mut data: TrackMetadata = serde_json::from_value(c.latest_data.clone())?;
    data.release_date = Some(date);
    Ok(Ok(Change { track_id: c.track_id, variant_id: c.variant_id, latest_id: c.latest_id, data, old: first_date, from }))
}

/// Writes the new date, and the same to the track's single if it is on one. Returns
/// whether the single was changed.
async fn write(conn: &mut sqlx::PgConnection, change: &Change) -> anyhow::Result<bool> {
    let reason = format!("release date re-read from the file's tags ({})", change.from);
    revisions::write_internal(
        conn,
        change.track_id,
        change.variant_id,
        Some(change.latest_id),
        &change.data,
        Source::Import,
        &reason,
        false,
    )
    .await?;

    // Its single: the track's only album, with no other track, whose date is still the
    // same year the import gave both.
    let album = sqlx::query!(
        r#"
        SELECT a.id
        FROM album_tracks at JOIN albums a ON a.id = at.album_id
        WHERE at.track_id = $1 AND a.merged_into IS NULL
          AND (SELECT count(*) FROM album_tracks o WHERE o.track_id = $1) = 1
          AND (SELECT count(*) FROM album_tracks o WHERE o.album_id = a.id) = 1
        "#,
        change.track_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(album) = album else {
        return Ok(false);
    };
    let album_revisions = sqlx::query!(
        r#"SELECT id, data FROM album_revisions WHERE album_id = $1 ORDER BY id"#,
        album.id
    )
    .fetch_all(&mut *conn)
    .await?;
    let (Some(first), Some(latest)) = (album_revisions.first(), album_revisions.last()) else {
        return Ok(false);
    };
    let date_of = |data: &serde_json::Value| data.get("release_date").and_then(|d| d.as_str()).map(str::to_string);
    if date_of(&first.data) != change.old || date_of(&latest.data) != change.old {
        return Ok(false);
    }
    let mut data: AlbumData = serde_json::from_value(latest.data.clone())?;
    data.release_date = change.data.release_date.clone();
    let reason = format!("release date re-read from its track's tags ({})", change.from);
    Ok(revisions::write_album(conn, album.id, Some(latest.id), &data, Source::Import, &reason).await?.changed)
}

/// How many of these tracks have a SoundCloud source.
async fn with_soundcloud_source(pool: &PgPool, track_ids: &[Uuid]) -> anyhow::Result<usize> {
    let mut found = HashSet::new();
    for chunk in track_ids.chunks(10_000) {
        let ids = sqlx::query_scalar!(
            r#"SELECT DISTINCT track_id AS "track_id!" FROM sources
               WHERE platform = 'soundcloud' AND trashed_at IS NULL AND track_id = ANY($1)"#,
            chunk
        )
        .fetch_all(pool)
        .await?;
        found.extend(ids);
    }
    Ok(found.len())
}

fn print_report(report: &Report, dry_run: bool) {
    let would = if dry_run { "would be " } else { "" };
    println!();
    println!("{} tracks looked at", report.candidates);
    println!("{} {}given a whole date ({} singles too)", report.written, would, report.albums_written);
    for (from, n) in &report.changed_from {
        println!("  {n:>7}  from {from}");
    }
    for (container, n) in &report.changed_container {
        println!("  {n:>7}  {container} files");
    }
    for (track, old, new) in &report.changed_samples {
        println!("           {track}: {} -> {new}", old.as_deref().unwrap_or("(none)"));
    }
    if !report.stale.is_empty() {
        println!("{} changed by someone while this ran, left alone: {:?}", report.stale.len(), report.stale);
    }
    println!("Kept as they are:");
    for (kept, ids) in &report.kept {
        println!("  {:>7}  {}", ids.len(), kept.describe());
        if *kept == Kept::NothingBetter {
            println!("           ({} of them have a SoundCloud source)", report.nothing_better_soundcloud);
        }
        for id in ids.iter().take(SAMPLES) {
            println!("           {id}");
        }
    }
    for (track, conflicts) in &report.other_year_samples {
        println!("           {track}: {conflicts}");
    }
    println!(
        "TYER+TDAT against a whole date elsewhere in the same file: {} agree, {} disagree",
        report.tdat_agree, report.tdat_disagree_count
    );
    for (track, composed, from, other) in &report.tdat_disagree {
        println!("           {track}: TYER+TDAT {composed}, {from} {other}");
    }
    if dry_run {
        println!("(dry run: nothing was changed)");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use super::*;
    use crate::model::sources::{self, Kind, NewSource, SourceOwner};
    use crate::model::{AlbumMembership, Credit, TrackFacts};

    /// An imported track as ingest leaves it: the file's tags kept with the file, revision 0
    /// with `date` and `reason`. Returns its id.
    #[allow(clippy::too_many_arguments)]
    async fn imported(
        conn: &mut sqlx::PgConnection,
        title: &str,
        date: Option<&str>,
        reason: &str,
        container: &str,
        sha256: &str,
        tags: serde_json::Value,
        album: Option<Uuid>,
    ) -> anyhow::Result<Uuid> {
        let artist = db::ingest::find_or_create_artist(conn, "Artist").await?.id;
        let file_id = Uuid::now_v7();
        let meta = TrackMetadata {
            title: title.into(),
            release_date: date.map(str::to_string),
            credits: vec![Credit { artist_id: artist, role: "primary".into(), alias_id: None }],
            ..Default::default()
        };
        let facts = TrackFacts {
            albums: album.into_iter().map(|album_id| AlbumMembership { album_id, disc_number: None, track_number: None }).collect(),
            ..Default::default()
        };
        let track_id = revisions::create_track(conn, file_id, &meta, &facts, "File tags", reason).await?;
        let origin = sources::validate(
            Kind::Track,
            NewSource {
                platform: "custom".into(),
                source_id: None,
                captured_at: None,
                data: json!({ "label": "test", "description": "made by the tests" }),
            },
        )?;
        let snapshot = db::sources::add_snapshot(conn, SourceOwner::Track(track_id), &origin).await?;
        db::ingest::insert_file(
            conn,
            &db::ingest::NewFile {
                id: file_id,
                origin_snapshot_id: snapshot.snapshot_id,
                sha256,
                original_filename: None,
                uploaded_filename: title,
                size_bytes: 1,
                container: Some(container),
                codec: None,
                bitrate: None,
                sample_rate: None,
                channels: None,
                bit_depth: None,
                duration_ms: None,
                embedded_tags: &json!({ "tags": tags }),
                imported_from: None,
            },
        )
        .await?;
        Ok(track_id)
    }

    fn id3(items: serde_json::Value) -> serde_json::Value {
        json!([{ "format": "id3v2", "items": items }])
    }

    fn year_and_tdat(year: &str, tdat: &str) -> serde_json::Value {
        id3(json!([{ "key": "TYER", "value": year }, { "key": "TDAT", "value": tdat }]))
    }

    fn sha(n: u8) -> String {
        format!("{n:064x}")
    }

    /// A WAV file whose RIFF INFO list has ICRD = `icrd`, and a moment of silence.
    fn wav_with_icrd(icrd: &str) -> Vec<u8> {
        let chunk = |id: &[u8], body: &[u8]| {
            let mut c = id.to_vec();
            c.extend((body.len() as u32).to_le_bytes());
            c.extend(body);
            if body.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        // PCM, mono, 8 kHz, 16 bit.
        let mut fmt = Vec::new();
        for (value, bytes) in [(1u32, 2), (1, 2), (8000, 4), (16000, 4), (2, 2), (16, 2)] {
            fmt.extend(&value.to_le_bytes()[..bytes]);
        }
        let mut text = icrd.as_bytes().to_vec();
        text.push(0);
        let mut info = b"INFO".to_vec();
        info.extend(chunk(b"ICRD", &text));
        let mut body = b"WAVE".to_vec();
        body.extend(chunk(b"fmt ", &fmt));
        body.extend(chunk(b"LIST", &info));
        body.extend(chunk(b"data", &[0; 1600]));
        chunk(b"RIFF", &body)
    }

    async fn track_date(pool: &PgPool, track: Uuid) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT release_date FROM tracks WHERE id = $1").bind(track).fetch_one(pool).await?)
    }

    #[sqlx::test]
    async fn dates_still_as_the_tags_gave_them_are_completed(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
        let pool = PgPoolOptions::new().connect_with(connect).await?;
        let blobs = tempfile::tempdir()?;
        let store = BlobStore::open(blobs.path())?;
        let wav_sha = store.put_bytes(&wav_with_icrd("2019-04-05")).await?;

        let mut tx = db::begin(&pool, None).await?;
        let single = revisions::create_album(
            &mut tx,
            &AlbumData { title: "Single".into(), kind: "single".into(), release_date: Some("2021".into()), ..Default::default() },
            Source::Import,
            "imported_from_file",
        )
        .await?;
        let plain = imported(&mut tx, "Plain", Some("2021"), "imported_from_file", "mp3", &sha(1), year_and_tdat("2021", "2512"), Some(single)).await?;
        // The date given with the upload, over the tags'.
        let hinted = imported(
            &mut tx,
            "Hinted",
            Some("2021"),
            "imported_from_file; release date from the description (the tags say 2020)",
            "mp3",
            &sha(2),
            year_and_tdat("2020", "2512"),
            None,
        )
        .await?;
        // Changed by hand since (to another year).
        let edited = imported(&mut tx, "Edited", Some("2021"), "imported_from_file", "mp3", &sha(3), year_and_tdat("2021", "2512"), None).await?;
        let variant: Uuid = sqlx::query_scalar("SELECT primary_variant_id FROM tracks WHERE id = $1").bind(edited).fetch_one(&mut *tx).await?;
        let mut data: TrackMetadata = serde_json::from_value(
            sqlx::query_scalar("SELECT data FROM track_revisions WHERE variant_id = $1").bind(variant).fetch_one(&mut *tx).await?,
        )?;
        data.release_date = Some("2018".into());
        revisions::write_internal(&mut tx, edited, variant, None, &data, Source::Edit, "the right year", false).await?;
        // A full date of another year only.
        let other_year = imported(
            &mut tx,
            "Other year",
            Some("2021"),
            "imported_from_file",
            "mp3",
            &sha(4),
            id3(json!([{ "key": "TYER", "value": "2021" }, { "key": "TXXX", "description": "RELEASETIME", "value": "2020-08-01" }])),
            None,
        )
        .await?;
        // A WAV stored before RIFF INFO was dumped: only its ID3v2 chunk is in the tags.
        let wav = imported(&mut tx, "Wav", Some("2019"), "imported_from_file", "wave", &wav_sha, id3(json!([{ "key": "TYER", "value": "2019" }])), None).await?;
        // Nothing better anywhere.
        let bare = imported(&mut tx, "Bare", Some("2017"), "imported_from_file", "mp3", &sha(5), id3(json!([{ "key": "TYER", "value": "2017" }])), None).await?;
        tx.commit().await?;

        run(&pool, &store, Options { dry_run: true }).await?;
        assert_eq!(track_date(&pool, plain).await?.as_deref(), Some("2021"), "a dry run changes nothing");

        run(&pool, &store, Options { dry_run: false }).await?;
        assert_eq!(track_date(&pool, plain).await?.as_deref(), Some("2021-12-25"));
        assert_eq!(track_date(&pool, wav).await?.as_deref(), Some("2019-04-05"), "from the blob's RIFF INFO");
        assert_eq!(track_date(&pool, hinted).await?.as_deref(), Some("2021"));
        assert_eq!(track_date(&pool, edited).await?.as_deref(), Some("2018"));
        assert_eq!(track_date(&pool, other_year).await?.as_deref(), Some("2021"));
        assert_eq!(track_date(&pool, bare).await?.as_deref(), Some("2017"));

        // A new revision of the import's version, saying why; the old one is history.
        let (source, reason): (String, String) =
            sqlx::query_as("SELECT source, reason FROM track_revisions WHERE track_id = $1 AND kind = 'internal' ORDER BY id DESC LIMIT 1")
                .bind(plain)
                .fetch_one(&pool)
                .await?;
        assert_eq!((source.as_str(), reason.as_str()), ("import", "release date re-read from the file's tags (TYER+TDAT)"));
        let album_date: Option<String> = sqlx::query_scalar("SELECT release_date FROM albums WHERE id = $1").bind(single).fetch_one(&pool).await?;
        assert_eq!(album_date.as_deref(), Some("2021-12-25"), "its single too");

        // Again: nothing left to do.
        let revisions_before: i64 = sqlx::query_scalar("SELECT count(*) FROM track_revisions").fetch_one(&pool).await?;
        run(&pool, &store, Options { dry_run: false }).await?;
        let revisions_after: i64 = sqlx::query_scalar("SELECT count(*) FROM track_revisions").fetch_one(&pool).await?;
        assert_eq!(revisions_before, revisions_after);
        Ok(())
    }
}
