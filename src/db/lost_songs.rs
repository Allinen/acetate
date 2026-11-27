//! Lost songs (migration 0072): extras of kind `lost_song`, made with their artists and
//! sources in one go (`create`, refused when the library holds the song or knows it's lost
//! already), listed (`list`), and taken off once a track turns out to be the song (`found`,
//! from a `lost_song_found` flag the database raises).

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::extras::{About, Extra, LOST_SONG, Link, Lost, LostStatus};
use super::revisions::EditRejected;
use crate::model::sources::{self, Kind, NewSource, SourceOwner};

#[derive(Debug)]
pub struct NewLostSong {
    /// Its title.
    pub name: String,
    /// The artists it's by (at least one): its page is on theirs.
    pub artist_ids: Vec<Uuid>,
    pub lost: Lost,
    pub description: String,
    /// Where it was (track-shaped), e.g. the SoundCloud upload that was deleted.
    pub sources: Vec<NewSource>,
}

/// Why a lost song wasn't made: something the user can see has one of its platform ids.
#[derive(Debug, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Known {
    /// A track has it: the song isn't lost.
    AlreadyHeld { track_id: Uuid, platform: String, external_id: String },
    /// A lost song (not taken off) has it already.
    AlreadyLost { extra_id: Uuid, platform: String, external_id: String },
}

impl Known {
    pub fn id(&self) -> Uuid {
        match self {
            Known::AlreadyHeld { track_id, .. } => *track_id,
            Known::AlreadyLost { extra_id, .. } => *extra_id,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Known::AlreadyHeld { platform, external_id, .. } => format!("a track has {platform} {external_id} already: the song isn't lost"),
            Known::AlreadyLost { platform, external_id, .. } => format!("a lost song has {platform} {external_id} already"),
        }
    }
}

/// Makes a lost song: the extra, linked to its artists, and its sources. `Err(Known)` (and
/// nothing made) when one of its platform ids is a track's or another lost song's.
pub async fn create(conn: &mut PgConnection, new: &NewLostSong, reason: &str) -> anyhow::Result<Result<Uuid, Known>> {
    if new.artist_ids.is_empty() {
        return Err(EditRejected::Invalid("a lost song needs an artist".into()).into());
    }
    let valid = new
        .sources
        .iter()
        .cloned()
        .map(|s| sources::validate(Kind::Track, s))
        .collect::<Result<Vec<_>, _>>()?;
    for source in &valid {
        let Some(external_id) = &source.external_id else { continue };
        if let Some(known) = known(conn, &source.platform, external_id).await? {
            return Ok(Err(known));
        }
    }
    for artist in &new.artist_ids {
        if !super::sources::owner_exists(conn, SourceOwner::Artist(*artist)).await? {
            return Err(EditRejected::NotFound("artist").into());
        }
    }
    let links: Vec<Link> = new.artist_ids.iter().map(|a| Link::Artist(*a)).collect();
    let about = About { description: new.description.clone(), source: String::new() };
    let (id, _) = super::extras::create(conn, &new.name, LOST_SONG, &about, &links, None, Some(&new.lost), reason).await?;
    for source in &valid {
        super::sources::add_snapshot(conn, SourceOwner::Extra(id), source).await?;
    }
    Ok(Ok(id))
}

/// A visible track or live lost song with this platform id.
async fn known(conn: &mut PgConnection, platform: &str, external_id: &str) -> sqlx::Result<Option<Known>> {
    let row = sqlx::query!(
        r#"SELECT final_track(s.track_id) AS track_id, s.extra_id
           FROM sources s
           LEFT JOIN extras e ON e.id = s.extra_id
           WHERE s.platform = $1 AND s.external_id = $2 AND s.trashed_at IS NULL
             AND (s.track_id IS NOT NULL OR (e.kind = 'lost_song' AND e.hidden_at IS NULL AND e.merged_into IS NULL))
           ORDER BY s.track_id IS NULL, s.id
           LIMIT 1"#,
        platform,
        external_id
    )
    .fetch_optional(conn)
    .await?;
    let (platform, external_id) = (platform.to_string(), external_id.to_string());
    Ok(row.and_then(|r| match (r.track_id, r.extra_id) {
        (Some(track_id), _) => Some(Known::AlreadyHeld { track_id, platform, external_id }),
        (None, Some(extra_id)) => Some(Known::AlreadyLost { extra_id, platform, external_id }),
        _ => None,
    }))
}

/// Lost songs the user can see (not taken off), by an artist (or those merged into it) and
/// of a status, by title.
pub async fn list(conn: &mut PgConnection, artist: Option<Uuid>, status: Option<LostStatus>, limit: i64) -> anyhow::Result<Vec<Extra>> {
    let status = status.map(|s| serde_json::to_value(s).map(|v| v.as_str().unwrap_or_default().to_string())).transpose()?;
    let ids = sqlx::query_scalar!(
        r#"SELECT e.id FROM extras e
           WHERE e.kind = 'lost_song' AND e.hidden_at IS NULL AND e.merged_into IS NULL
             AND ($2::text IS NULL OR e.lost ->> 'status' = $2)
             AND ($1::uuid IS NULL OR EXISTS (SELECT 1 FROM extra_links l WHERE l.extra_id = e.id
                                              AND l.artist_id IS NOT NULL AND final_artist(l.artist_id) = $1))
           ORDER BY lower(e.name), e.id
           LIMIT $3"#,
        artist,
        status,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(extra) = super::extras::get(conn, id).await? {
            out.push(extra);
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Found {
    pub track_id: Uuid,
    pub extra_id: Uuid,
    /// The lost song's sources the track didn't have.
    pub sources_added: usize,
}

/// Resolves the open `lost_song_found` flag `flag_id` as found: the track gets the lost
/// song's sources it lacks (each snapshot, so their history comes along), and the lost song
/// is taken off (kept). Other open flags pointing at the same lost song are settled too. None:
/// there's no such open flag the user can see.
pub async fn found(conn: &mut PgConnection, flag_id: Uuid) -> anyhow::Result<Option<Found>> {
    let Some(flag) = sqlx::query!("SELECT track_id, related FROM review_flags WHERE id = $1 AND kind = 'lost_song_found' AND resolved_at IS NULL", flag_id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };
    let extra_id: Uuid = flag
        .related
        .get("extra_id")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("lost_song_found flag {flag_id} names no lost song"))?;
    let extra = super::extras::get(conn, extra_id).await?.ok_or(EditRejected::NotFound("lost song"))?;
    let track_had: Vec<(String, Option<String>)> =
        super::sources::list_for(conn, flag.track_id).await?.into_iter().map(|s| (s.platform, s.external_id)).collect();
    let mut sources_added = 0;
    for source in &extra.sources {
        if track_had.contains(&(source.platform.clone(), source.external_id.clone())) {
            continue;
        }
        for snapshot in source.snapshots.iter().rev() {
            let given = NewSource { platform: source.platform.clone(), source_id: None, captured_at: Some(snapshot.captured_at), data: snapshot.data.clone() };
            let valid = sources::validate(Kind::Track, given)?;
            super::sources::add_snapshot(conn, SourceOwner::Track(flag.track_id), &valid).await?;
        }
        sources_added += 1;
    }
    let reason = format!("found: it's track {}", flag.track_id);
    super::extras::set_hidden(conn, extra_id, true, &reason).await?;
    super::review::resolve(conn, flag_id, "found").await?;
    sqlx::query!(
        "UPDATE review_flags SET resolved_at = now(), resolved_by = current_app_user(), resolution = $2
         WHERE kind = 'lost_song_found' AND resolved_at IS NULL AND related ->> 'extra_id' = $1",
        extra_id.to_string(),
        format!("the lost song was found on track {}", flag.track_id)
    )
    .execute(&mut *conn)
    .await?;
    Ok(Some(Found { track_id: flag.track_id, extra_id, sources_added }))
}
