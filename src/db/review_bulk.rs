//! Review flags handled many at a time (the Review page's bulk tools):
//!
//! * `settle_ambiguous`: tracks whose artist name matched several artists (one flag each)
//!   all credit the one chosen, each as a revision, and their flags are settled saying so;
//! * `make_singles`: tracks on no album each get a single of their own (named after the
//!   track, credited to its primary artists, with its release date and covers; the track
//!   as disc 1, track 1), so the "missing album" flag settles itself;
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
