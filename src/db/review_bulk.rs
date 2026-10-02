//! Review flags handled many at a time (the Review page's bulk tools):
//!
//! * `settle_ambiguous`: tracks whose artist name matched several artists (one flag each)
//!   all credit the one chosen, each as a revision, and their flags are settled saying so;
//! * `make_singles`: tracks on no album each get a single of their own (named after the
//!   track, credited to its primary artists, with its release date and covers; the track
//!   as disc 1, track 1), so the "missing album" flag settles itself;
//! * `merge_duplicates`: possible duplicates with the same title and about the same
//!   length are merged (the `merge-duplicates` command too);
//! * resolving many flags at once is `review::resolve_many`.
//!
//! Everything goes through revisions, like a change made by hand, and only touches what
//! the user can see.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::{self, EditRejected, Source};
use crate::model::{AlbumData, AlbumMembership, Credit, TrackFacts};

/// What `settle_ambiguous` did.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Settled {
    /// Flags settled.
    pub settled: usize,
    /// Of those, tracks whose credit changed (the others credited the chosen artist already).
    pub changed: usize,
    /// Flags left open: none of their candidates is credited anymore (edited since), or
    /// already settled.
    pub skipped: usize,
}

/// Every flag in `flags` (ambiguous artist names) settles on `artist`: on each track, the
/// credit of the name (whichever candidate it credits now) becomes `artist`'s, under its
/// own name (a new revision of the shown version), and the flag is settled with what was
/// done. `artist` needn't be one of the candidates (the right one may not have been).
pub async fn settle_ambiguous(conn: &mut PgConnection, flags: &[Uuid], artist: Uuid, reason: &str) -> anyhow::Result<Settled> {
    let name = sqlx::query_scalar!("SELECT name FROM artists WHERE id = $1 AND merged_into IS NULL", artist)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("artist"))?;
    let reason = if reason.trim().is_empty() { format!("ambiguous artist name: \"{name}\"") } else { reason.trim().to_string() };
    let mut done = Settled::default();
    for &flag in flags {
        let Some(row) = sqlx::query!(
            "SELECT f.track_id, f.related, t.primary_variant_id FROM review_flags f JOIN tracks t ON t.id = f.track_id
             WHERE f.id = $1 AND f.kind = 'ambiguous_artist' AND f.resolved_at IS NULL",
            flag
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            done.skipped += 1;
            continue;
        };
        let candidates: Vec<Uuid> = row.related["artists"].as_array().into_iter().flatten().filter_map(|a| a.as_str()?.parse().ok()).collect();
        let Some(variant) = row.primary_variant_id else {
            done.skipped += 1;
            continue;
        };
        let Some((_, mut meta)) = revisions::latest_internal(conn, row.track_id, variant).await? else {
            done.skipped += 1;
            continue;
        };
        // The credit of the name: the candidate ingest chose, if still credited, else the
        // candidate credited now (edited since).
        let chosen = row.related["chosen"].as_str().and_then(|c| c.parse::<Uuid>().ok());
        let at = meta.credits.iter().position(|c| Some(c.artist_id) == chosen).or_else(|| meta.credits.iter().position(|c| candidates.contains(&c.artist_id)));
        let Some(at) = at else {
            done.skipped += 1;
            continue;
        };
        // Settled first, in these words: the edit below would settle it too, more vaguely.
        if meta.credits[at].artist_id == artist {
            super::review::resolve(conn, flag, &format!("\"{name}\" was right")).await?;
        } else {
            super::review::resolve(conn, flag, &format!("credited \"{name}\" ({artist}) instead")).await?;
            let role = meta.credits[at].role.clone();
            // Credited already in that role (under another name of the tags): once is enough.
            if meta.credits.iter().enumerate().any(|(i, c)| i != at && c.artist_id == artist && c.role == role) {
                meta.credits.remove(at);
            } else {
                meta.credits[at] = Credit { artist_id: artist, role, alias_id: None };
            }
            revisions::write_internal(conn, row.track_id, variant, None, &meta, Source::Edit, &reason, false).await?;
            done.changed += 1;
        }
        done.settled += 1;
    }
    Ok(done)
}

/// A single made for a track.
#[derive(Debug, Serialize, PartialEq)]
pub struct Made {
    pub track_id: Uuid,
    pub album_id: Uuid,
}

/// What `make_singles` did.
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Singles {
    pub made: Vec<Made>,
    /// Tracks left as they were: on an album already, or without the revisions to build on.
    pub skipped: Vec<Uuid>,
}

/// Each track that's on no album gets a single of its own: named after the track, of kind
/// `single`, credited to its primary artists (as credited), with its release date; the
/// track on it as disc 1, track 1 (a revision of its facts), and its covers copied onto it
/// (the same images). Tracks on an album already are left out.
pub async fn make_singles(conn: &mut PgConnection, tracks: &[Uuid], reason: &str) -> anyhow::Result<Singles> {
    let reason = if reason.trim().is_empty() { "a single of its own".to_string() } else { reason.trim().to_string() };
    let mut out = Singles::default();
    for &track in tracks {
        let Some(row) = sqlx::query!(
            r#"SELECT t.primary_variant_id,
                      EXISTS (SELECT 1 FROM album_tracks at JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
                              WHERE at.track_id = t.id) AS "on_album!"
               FROM tracks t WHERE t.id = $1 AND t.merged_into IS NULL"#,
            track
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            return Err(EditRejected::NotFound("track").into());
        };
        if row.on_album {
            out.skipped.push(track);
            continue;
        }
        match make_single(conn, track, row.primary_variant_id, &reason).await? {
            Some(album) => out.made.push(Made { track_id: track, album_id: album }),
            None => out.skipped.push(track),
        }
    }
    Ok(out)
}

/// A single for a track, whatever albums it's on: named after it (its shown version,
/// `variant`), of kind `single`, credited to its primary artists (as credited), with its
/// release date; the track on it as disc 1, track 1 (a revision of its facts), and its
/// covers copied onto it. None: it hasn't the revisions to build on.
pub async fn make_single(conn: &mut PgConnection, track: Uuid, variant: Option<Uuid>, reason: &str) -> anyhow::Result<Option<Uuid>> {
    let meta = match variant {
        Some(variant) => revisions::latest_internal(conn, track, variant).await?.map(|(_, m)| m),
        None => None,
    };
    let facts = revisions::latest_external(conn, track).await?.and_then(|(_, data)| serde_json::from_value::<TrackFacts>(data).ok());
    let (Some(meta), Some(mut facts)) = (meta, facts) else { return Ok(None) };
    let credits: Vec<Credit> = meta.credits.iter().filter(|c| c.role == "primary").cloned().collect();
    let data = AlbumData { title: meta.title.clone(), kind: "single".into(), release_date: meta.release_date.clone(), credits };
    let album = revisions::create_album(conn, &data, Source::Edit, reason).await?;
    facts.albums.push(AlbumMembership { album_id: album, disc_number: Some(1), track_number: Some(1) });
    revisions::write_external(conn, track, None, &facts, Source::Edit, reason).await?;
    let covers = sqlx::query_scalar!("SELECT id FROM covers WHERE track_id = $1 AND trashed_at IS NULL ORDER BY position, id", track)
        .fetch_all(&mut *conn)
        .await?;
    for cover in covers {
        super::album_merges::copy_if_new(conn, cover, album).await?;
    }
    Ok(Some(album))
}

/// How `merge_duplicates` tells the same track.
#[derive(Debug, Clone, Copy)]
pub struct SameTrack {
    /// How far apart the two lengths may be, in milliseconds. An unknown length is never
    /// close enough: not clear enough to merge without a look.
    pub max_length_difference_ms: i64,
    /// Titles that differ only in case ("Intro" and "INTRO") count as the same.
    pub ignore_case: bool,
}

/// One side of a pair `merge_duplicates` looked at.
#[derive(Debug, Clone, Serialize)]
pub struct PairTrack {
    pub id: Uuid,
    pub title: String,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub size_bytes: i64,
}

/// How good a file's format is, to keep the better of two: uncompressed (WAV, AIFF) over
/// lossless (FLAC, ALAC…) over lossy (MP3, AAC…) over unknown. Within one, the bigger file
/// is the better copy. The desktop client's merge view suggests a file the same way.
pub fn format_rank(codec: Option<&str>) -> u8 {
    let Some(codec) = codec.map(str::to_lowercase) else { return 0 };
    if codec.starts_with("pcm") || matches!(codec.as_str(), "wav" | "aiff") {
        3
    } else if matches!(codec.as_str(), "flac" | "alac" | "ape" | "wavpack") {
        2
    } else {
        1
    }
}

impl PairTrack {
    /// Which file to keep of two: the better format, then the bigger.
    fn better_file(&self) -> (u8, i64) {
        (format_rank(self.codec.as_deref()), self.size_bytes)
    }
}

/// A pair merged (or that failed to be), and how.
#[derive(Debug, Serialize)]
pub struct MergedPair {
    /// The older track: kept, with its metadata and its covers first.
    pub kept: PairTrack,
    pub merged: PairTrack,
    /// The merged track's file became the kept track's (the better: see `format_rank`).
    pub merged_file_kept: bool,
}

/// What `merge_duplicates` did.
#[derive(Debug, Default, Serialize)]
pub struct MergedDuplicates {
    pub merged: Vec<MergedPair>,
    /// Pairs whose titles or lengths differ: left as they are, flagged.
    pub different: usize,
    /// Pairs whose merge failed (e.g. one can't be seen): their flags stay open.
    pub failed: Vec<FailedPair>,
}

#[derive(Debug, Serialize)]
pub struct FailedPair {
    #[serde(flatten)]
    pub pair: MergedPair,
    pub error: String,
}

/// Merges the pairs the open "possible duplicate" `flags` name (`None`: all of them) that
/// are clearly the same track: the same title and about the same length (`same`). Each
/// is an ordinary merge (`revisions::merge_tracks`: revisioned, settles the pair's flags,
/// can be undone on the kept track's page), each in a savepoint: one that fails is left
/// out and the rest go on. The older track is kept; the better file stays (`format_rank`:
/// WAV over FLAC over MP3, then the bigger one).
///
/// A track merged by an earlier pair is looked at as the track it went into (A looks like
/// B, B like C: C is compared with A, once B is A). Rolled back, it's an exact preview.
pub async fn merge_duplicates(conn: &mut PgConnection, flags: Option<&[Uuid]>, same: SameTrack, reason: &str) -> anyhow::Result<MergedDuplicates> {
    // Every pair a flag names: (flagged track, one of the tracks it looks like).
    let pairs = sqlx::query!(
        r#"
        SELECT rf.track_id, listed.id::uuid AS "other!"
        FROM review_flags rf
        CROSS JOIN jsonb_array_elements_text(rf.related -> 'tracks') AS listed(id)
        WHERE rf.kind = 'possible_duplicate' AND rf.resolved_at IS NULL
          AND ($1::uuid[] IS NULL OR rf.id = ANY($1))
        ORDER BY rf.id, listed.id
        "#,
        flags
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut done = MergedDuplicates::default();
    let mut seen = std::collections::HashSet::new();
    for pair in pairs {
        // Earlier merges may have made either one a redirect: go where it leads.
        let (Some(a), Some(b)) = (current(conn, pair.track_id).await?, current(conn, pair.other).await?) else {
            continue;
        };
        if a.id == b.id {
            continue; // merged already (a chain)
        }
        // Flagged both ways (A looks like B, B looks like A): once is enough.
        if !seen.insert((a.id.min(b.id), a.id.max(b.id))) {
            continue;
        }
        let length_ok = match (a.duration_ms, b.duration_ms) {
            (Some(x), Some(y)) => (x - y).abs() <= same.max_length_difference_ms,
            _ => false,
        };
        let same_title = if same.ignore_case { a.title.to_lowercase() == b.title.to_lowercase() } else { a.title == b.title };
        if !same_title || !length_ok {
            done.different += 1;
            continue;
        }

        // UUIDv7: the smaller id was made first.
        let (kept, merged) = if a.id < b.id { (a, b) } else { (b, a) };
        let merged_file_kept = merged.better_file() > kept.better_file();
        let picks = revisions::MergePicks { file: if merged_file_kept { revisions::Side::Right } else { revisions::Side::Left }, ..Default::default() };
        let mut savepoint = sqlx::Connection::begin(&mut *conn).await?;
        let result = revisions::merge_tracks(&mut savepoint, kept.id, merged.id, &picks, None, None, reason).await;
        let pair = MergedPair { kept, merged, merged_file_kept };
        match result {
            Ok(_) => {
                savepoint.commit().await?;
                done.merged.push(pair);
            }
            Err(err) => {
                savepoint.rollback().await?;
                done.failed.push(FailedPair { pair, error: format!("{err:#}") });
            }
        }
    }
    Ok(done)
}

/// The track as it is now: following merge redirects to the track it ended up in.
async fn current(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<PairTrack>> {
    sqlx::query_as!(
        PairTrack,
        r#"
        SELECT t.id, t.title, f.duration_ms, f.codec, f.size_bytes
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id = COALESCE((SELECT merged_into FROM tracks WHERE id = $1), $1)
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

#[cfg(test)]
mod tests {
    use super::format_rank;

    #[test]
    fn wav_over_flac_over_mp3() {
        let order = [None, Some("mp3"), Some("aac"), Some("flac"), Some("pcm_s16le"), Some("pcm_s24le")].map(format_rank);
        assert_eq!(order, [0, 1, 1, 2, 3, 3]);
    }
}
