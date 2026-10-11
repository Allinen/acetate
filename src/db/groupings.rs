//! Groupings (migration 0034): tracks and albums gathered under a name, apart from albums
//! and credits. A `collective` is a platform account several artists post under; `era`,
//! `project` and other kinds work the same way. A track or album can be in several.
//!
//! A grouping has sources (the accounts it is): ingest puts a song whose artist id is one
//! of them into the grouping, instead of crediting an artist (see `ingest::describe`).
//! An artist that is really a collective (made by ingest before there were groupings) is
//! turned into one with `turn_into_collective`, after `collective_preview` says what it
//! would do.
//!
//! Nothing is deleted but an empty grouping (`delete`): every change is kept in `grouping_log`.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::{write_album, write_internal, EditRejected, Source};
use crate::model::{AlbumData, TrackMetadata};

fn invalid(message: impl Into<String>) -> anyhow::Error {
    EditRejected::Invalid(message.into()).into()
}

/// A kind is lowercase letters (and `_`): "collective", "era", "project".
fn check(kind: &str, name: &str) -> anyhow::Result<()> {
    let valid_kind = kind.starts_with(|c: char| c.is_ascii_lowercase()) && kind.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if !valid_kind {
        return Err(invalid(format!("\"{kind}\" isn't a kind of grouping (lowercase letters: collective, era, project…)")));
    }
    if name.trim().is_empty() {
        return Err(invalid("a grouping needs a name"));
    }
    Ok(())
}

async fn log(
    conn: &mut PgConnection,
    grouping: Uuid,
    action: &str,
    track: Option<Uuid>,
    album: Option<Uuid>,
    detail: serde_json::Value,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO grouping_log (grouping_id, action, track_id, album_id, detail, reason) VALUES ($1, $2, $3, $4, $5, $6)",
        grouping,
        action,
        track,
        album,
        detail,
        reason
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Makes a grouping. Returns its id.
pub async fn create(conn: &mut PgConnection, kind: &str, name: &str, description: &str, reason: &str) -> anyhow::Result<Uuid> {
    check(kind, name)?;
    let id = sqlx::query_scalar!(
        "INSERT INTO groupings (kind, name, description) VALUES ($1, $2, $3) RETURNING id",
        kind,
        name.trim(),
        description.trim()
    )
    .fetch_one(&mut *conn)
    .await?;
    log(conn, id, "created", None, None, serde_json::json!({ "kind": kind, "name": name.trim() }), reason).await?;
    Ok(id)
}

/// Changes its kind, name and description (what it was is kept in the log).
pub async fn edit(conn: &mut PgConnection, id: Uuid, kind: &str, name: &str, description: &str, reason: &str) -> anyhow::Result<()> {
    check(kind, name)?;
    let old = sqlx::query!("SELECT kind, name, description FROM groupings WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("grouping"))?;
    if (old.kind.as_str(), old.name.as_str(), old.description.as_str()) == (kind, name.trim(), description.trim()) {
        return Ok(());
    }
    sqlx::query!("UPDATE groupings SET kind = $2, name = $3, description = $4 WHERE id = $1", id, kind, name.trim(), description.trim())
        .execute(&mut *conn)
        .await?;
    let detail = serde_json::json!({
        "old": { "kind": old.kind, "name": old.name, "description": old.description },
        "new": { "kind": kind, "name": name.trim(), "description": description.trim() },
    });
    log(conn, id, "edited", None, None, detail, reason).await?;
    Ok(())
}

/// Deletes a grouping with nothing in it: no tracks, releases, members, sources or extras
/// (take those out first). Its log is kept, ending in a `deleted` entry. Making one again
/// with the same name is the way back.
pub async fn delete(conn: &mut PgConnection, id: Uuid, reason: &str) -> anyhow::Result<()> {
    let old = sqlx::query!("SELECT kind, name FROM groupings WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("grouping"))?;
    let counts = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM grouping_tracks WHERE grouping_id = $1) AS "tracks!",
                  (SELECT count(*) FROM grouping_albums WHERE grouping_id = $1) AS "albums!",
                  (SELECT count(*) FROM grouping_artists WHERE grouping_id = $1) AS "members!",
                  (SELECT count(*) FROM sources WHERE grouping_id = $1) AS "sources!",
                  (SELECT count(*) FROM extra_links WHERE grouping_id = $1) AS "extras!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    for (n, what) in [(counts.tracks, "tracks"), (counts.albums, "releases"), (counts.members, "members"), (counts.sources, "sources"), (counts.extras, "extras")] {
        if n > 0 {
            return Err(invalid(format!("\"{}\" isn't empty: it has {n} {what} (take them out first)", old.name)));
        }
    }
    log(conn, id, "deleted", None, None, serde_json::json!({ "kind": old.kind, "name": old.name }), reason).await?;
    sqlx::query!("DELETE FROM groupings WHERE id = $1", id).execute(&mut *conn).await?;
    Ok(())
}

async fn exists(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM groupings WHERE id = $1) AS "e!""#, id).fetch_one(conn).await
}

/// What `add` or `remove` changed (a single goes with its only track, and the other way).
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Changed {
    pub tracks: usize,
    pub albums: usize,
}

/// The singles `track` is the only track of: in a grouping, they go with it.
pub(crate) async fn singles_of(conn: &mut PgConnection, track: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!(
        r#"SELECT a.id FROM album_tracks at
           JOIN albums a ON a.id = at.album_id AND a.kind = 'single' AND a.merged_into IS NULL
           WHERE at.track_id = $1
             AND (SELECT count(*) FROM album_tracks x JOIN tracks t ON t.id = x.track_id AND t.merged_into IS NULL
                  WHERE x.album_id = a.id) = 1"#,
        track
    )
    .fetch_all(conn)
    .await
}

/// A single's only track: it goes with it.
pub(crate) async fn only_track_of(conn: &mut PgConnection, album: Uuid) -> sqlx::Result<Option<Uuid>> {
    let tracks = sqlx::query_scalar!(
        r#"SELECT x.track_id FROM album_tracks x
           JOIN albums a ON a.id = x.album_id AND a.kind = 'single'
           JOIN tracks t ON t.id = x.track_id AND t.merged_into IS NULL
           WHERE x.album_id = $1"#,
        album
    )
    .fetch_all(conn)
    .await?;
    Ok(if tracks.len() == 1 { tracks.first().copied() } else { None })
}

/// The tracks and albums meant: the ones given (each one the user can see, or it's
/// refused like one that doesn't exist), and what goes with them (a single and its only
/// track), each with what it goes with.
async fn with_singles(conn: &mut PgConnection, tracks: &[Uuid], albums: &[Uuid]) -> anyhow::Result<(Vec<(Uuid, Option<Uuid>)>, Vec<(Uuid, Option<Uuid>)>)> {
    let mut out_tracks: Vec<(Uuid, Option<Uuid>)> = Vec::new();
    let mut out_albums: Vec<(Uuid, Option<Uuid>)> = Vec::new();
    for &track in tracks {
        let visible = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1 AND merged_into IS NULL) AS "e!""#, track)
            .fetch_one(&mut *conn)
            .await?;
        if !visible {
            return Err(EditRejected::NotFound("track").into());
        }
        out_tracks.push((track, None));
        for single in singles_of(conn, track).await? {
            out_albums.push((single, Some(track)));
        }
    }
    for &album in albums {
        let visible = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM albums WHERE id = $1 AND merged_into IS NULL) AS "e!""#, album)
            .fetch_one(&mut *conn)
            .await?;
        if !visible {
            return Err(EditRejected::NotFound("album").into());
        }
        out_albums.push((album, None));
        if let Some(track) = only_track_of(conn, album).await? {
            out_tracks.push((track, Some(album)));
        }
    }
    // Given explicitly wins over "went along" (first one stays).
    let mut seen = std::collections::HashSet::new();
    out_tracks.retain(|(id, _)| seen.insert(*id));
    let mut seen = std::collections::HashSet::new();
    out_albums.retain(|(id, _)| seen.insert(*id));
    Ok((out_tracks, out_albums))
}

fn along(with: Option<Uuid>) -> serde_json::Value {
    match with {
        Some(id) => serde_json::json!({ "along_with": id }),
        None => serde_json::json!({}),
    }
}

/// Puts tracks and albums in the grouping (those already in it stay as they are). A track
/// that's the only one on a single brings the single along, and a single its only track:
/// they're one release. Returns how many were added.
pub async fn add(conn: &mut PgConnection, id: Uuid, tracks: &[Uuid], albums: &[Uuid], reason: &str) -> anyhow::Result<Changed> {
    if !exists(conn, id).await? {
        return Err(EditRejected::NotFound("grouping").into());
    }
    let (tracks, albums) = with_singles(conn, tracks, albums).await?;
    let mut changed = Changed::default();
    for (track, with) in tracks {
        let inserted = sqlx::query!("INSERT INTO grouping_tracks (grouping_id, track_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, track)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if inserted > 0 {
            log(conn, id, "added", Some(track), None, along(with), reason).await?;
            changed.tracks += 1;
        }
    }
    for (album, with) in albums {
        let inserted = sqlx::query!("INSERT INTO grouping_albums (grouping_id, album_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, album)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if inserted > 0 {
            log(conn, id, "added", None, Some(album), along(with), reason).await?;
            changed.albums += 1;
        }
    }
    Ok(changed)
}

/// Takes tracks and albums out of the grouping (a single and its only track together, as
/// they went in). Returns how many were in it.
pub async fn remove(conn: &mut PgConnection, id: Uuid, tracks: &[Uuid], albums: &[Uuid], reason: &str) -> anyhow::Result<Changed> {
    if !exists(conn, id).await? {
        return Err(EditRejected::NotFound("grouping").into());
    }
    let (tracks, albums) = with_singles(conn, tracks, albums).await?;
    let mut changed = Changed::default();
    for (track, with) in tracks {
        let gone = sqlx::query!("DELETE FROM grouping_tracks WHERE grouping_id = $1 AND track_id = $2", id, track)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if gone > 0 {
            log(conn, id, "removed", Some(track), None, along(with), reason).await?;
            changed.tracks += 1;
        }
    }
    for (album, with) in albums {
        let gone = sqlx::query!("DELETE FROM grouping_albums WHERE grouping_id = $1 AND album_id = $2", id, album)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if gone > 0 {
            log(conn, id, "removed", None, Some(album), along(with), reason).await?;
            changed.albums += 1;
        }
    }
    Ok(changed)
}

/// A member added to or taken from the log.
async fn log_member(conn: &mut PgConnection, grouping: Uuid, action: &str, artist: Uuid, reason: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO grouping_log (grouping_id, action, artist_id, reason) VALUES ($1, $2, $3, $4)",
        grouping,
        action,
        artist,
        reason
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Makes artists members (who's in it, said by hand: being credited on one of its songs
/// isn't enough). Returns how many weren't already.
pub async fn add_members(conn: &mut PgConnection, id: Uuid, artists: &[Uuid], reason: &str) -> anyhow::Result<usize> {
    if !exists(conn, id).await? {
        return Err(EditRejected::NotFound("grouping").into());
    }
    let mut added = 0;
    for &artist in artists {
        let usable = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM artists WHERE id = $1 AND merged_into IS NULL) AS "e!""#, artist)
            .fetch_one(&mut *conn)
            .await?;
        if !usable {
            return Err(EditRejected::NotFound("artist").into());
        }
        let inserted = sqlx::query!("INSERT INTO grouping_artists (grouping_id, artist_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, artist)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if inserted > 0 {
            log_member(conn, id, "member_added", artist, reason).await?;
            added += 1;
        }
    }
    Ok(added)
}

/// Takes artists out of the members. Returns how many were.
pub async fn remove_members(conn: &mut PgConnection, id: Uuid, artists: &[Uuid], reason: &str) -> anyhow::Result<usize> {
    if !exists(conn, id).await? {
        return Err(EditRejected::NotFound("grouping").into());
    }
    let mut removed = 0;
    for &artist in artists {
        let gone = sqlx::query!("DELETE FROM grouping_artists WHERE grouping_id = $1 AND artist_id = $2", id, artist)
            .execute(&mut *conn)
            .await?
            .rows_affected();
        if gone > 0 {
            log_member(conn, id, "member_removed", artist, reason).await?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// A merged track's or album's groupings go with it: the one it went into joins them
/// (`from` stays in them too; it's hidden, and there if the merge is undone).
pub async fn follow_merge(conn: &mut PgConnection, from: Uuid, into: Uuid, reason: &str) -> sqlx::Result<()> {
    let tracks = sqlx::query_scalar!(
        "INSERT INTO grouping_tracks (grouping_id, track_id)
         SELECT grouping_id, $2 FROM grouping_tracks WHERE track_id = $1
         ON CONFLICT DO NOTHING RETURNING grouping_id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    for grouping in tracks {
        log(conn, grouping, "added", Some(into), None, serde_json::json!({ "merged_from": from }), reason).await?;
    }
    let albums = sqlx::query_scalar!(
        "INSERT INTO grouping_albums (grouping_id, album_id)
         SELECT grouping_id, $2 FROM grouping_albums WHERE album_id = $1
         ON CONFLICT DO NOTHING RETURNING grouping_id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    for grouping in albums {
        log(conn, grouping, "added", None, Some(into), serde_json::json!({ "merged_from": from }), reason).await?;
    }
    Ok(())
}

/// Undoing a merge: the kept track or album (`into`) leaves the groupings the merge put it
/// in (`follow_merge`), where that's still how it got there: the merge's "added" is the
/// latest word on it (someone taking it out and putting it back since, say, is left as
/// it is). Returns the groupings it left.
pub async fn undo_follow(conn: &mut PgConnection, from: Uuid, into: Uuid, reason: &str) -> sqlx::Result<Vec<Uuid>> {
    let added = follow_to_undo(conn, from, into).await?;
    let mut left = Vec::new();
    for entry in added {
        let gone = if entry.track_id == Some(into) {
            sqlx::query!("DELETE FROM grouping_tracks WHERE grouping_id = $1 AND track_id = $2", entry.grouping_id, into).execute(&mut *conn).await?
        } else {
            sqlx::query!("DELETE FROM grouping_albums WHERE grouping_id = $1 AND album_id = $2", entry.grouping_id, into).execute(&mut *conn).await?
        };
        if gone.rows_affected() > 0 {
            log(conn, entry.grouping_id, "removed", entry.track_id, entry.album_id, serde_json::json!({ "merge_undone": from }), reason).await?;
            left.push(entry.grouping_id);
        }
    }
    Ok(left)
}

/// A grouping a merge put the kept one in, still only because of it (see `undo_follow`).
pub struct Followed {
    pub grouping_id: Uuid,
    pub name: String,
    track_id: Option<Uuid>,
    album_id: Option<Uuid>,
}

/// What `undo_follow` would take the kept one out of (for an undo's preview).
pub async fn follow_to_undo(conn: &mut PgConnection, from: Uuid, into: Uuid) -> sqlx::Result<Vec<Followed>> {
    sqlx::query_as!(
        Followed,
        r#"SELECT l.grouping_id, g.name, l.track_id, l.album_id FROM grouping_log l
           JOIN groupings g ON g.id = l.grouping_id
           WHERE l.action = 'added' AND l.detail->>'merged_from' = $1::text
             AND (l.track_id = $2 OR l.album_id = $2)
             AND NOT EXISTS (SELECT 1 FROM grouping_log later
                             WHERE later.grouping_id = l.grouping_id AND later.id > l.id
                               AND (later.track_id = $2 OR later.album_id = $2))"#,
        from.to_string(),
        into
    )
    .fetch_all(conn)
    .await
}

/// The groupings with this platform id among their sources (ingest: a song's artist id
/// that is a collective's account). Not trashed ones. Oldest first.
pub async fn with_platform_id(conn: &mut PgConnection, platform: &str, external_id: &str) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!(
        r#"SELECT grouping_id AS "id!" FROM sources
           WHERE grouping_id IS NOT NULL AND platform = $1 AND external_id = $2 AND trashed_at IS NULL
           ORDER BY grouping_id"#,
        platform,
        external_id
    )
    .fetch_all(conn)
    .await
}

/// An artist credited on a grouping's tracks, with on how many.
#[derive(Debug, Serialize)]
pub struct Member {
    pub artist_id: Uuid,
    pub name: String,
    pub tracks: i64,
}

/// One change, as the log has it.
#[derive(Debug, Serialize)]
pub struct Change {
    pub at: chrono::DateTime<chrono::Utc>,
    pub by: Option<String>,
    pub action: String,
    pub track_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
    /// A member added or taken out.
    pub artist_id: Option<Uuid>,
    pub detail: serde_json::Value,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct Detail {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Its tracks and albums the user can see (not merged ones).
    pub tracks: Vec<Uuid>,
    pub albums: Vec<Uuid>,
    /// Who's in it (said by hand), with how many of its tracks each is credited on.
    pub members: Vec<Member>,
    /// Others credited on its tracks (a feature, a producer…), most tracks first: not
    /// members unless someone says so.
    pub credited: Vec<Member>,
    pub sources: Vec<super::sources::Source>,
    /// Extras linked to it (vocals, stems…; taken-off ones too, flagged).
    pub extras: Vec<super::extras::Extra>,
    /// Newest first.
    pub changes: Vec<Change>,
}

pub async fn detail(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<Detail>> {
    let Some(g) = sqlx::query!("SELECT id, kind, name, description, created_at FROM groupings WHERE id = $1", id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };
    let tracks = sqlx::query_scalar!(
        "SELECT gt.track_id FROM grouping_tracks gt JOIN tracks t ON t.id = gt.track_id
         WHERE gt.grouping_id = $1 AND t.merged_into IS NULL ORDER BY gt.added_at, gt.track_id",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let albums = sqlx::query_scalar!(
        "SELECT ga.album_id FROM grouping_albums ga JOIN albums a ON a.id = ga.album_id
         WHERE ga.grouping_id = $1 AND a.merged_into IS NULL ORDER BY ga.added_at, ga.album_id",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Everyone credited on its tracks, with on how many; the members among them (and the
    // members credited on none) apart.
    let credited_all = sqlx::query_as!(
        Member,
        r#"SELECT c.artist_id, a.name, count(DISTINCT c.track_id) AS "tracks!"
           FROM grouping_tracks gt
           JOIN tracks t ON t.id = gt.track_id AND t.merged_into IS NULL
           JOIN track_credits c ON c.track_id = gt.track_id
           JOIN artists a ON a.id = c.artist_id
           WHERE gt.grouping_id = $1
           GROUP BY c.artist_id, a.name
           ORDER BY 3 DESC, a.name"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let member_rows = sqlx::query!(
        "SELECT a.id, a.name FROM grouping_artists ga JOIN artists a ON a.id = ga.artist_id WHERE ga.grouping_id = $1 ORDER BY a.name",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut members: Vec<Member> = member_rows
        .into_iter()
        .map(|m| Member { tracks: credited_all.iter().find(|c| c.artist_id == m.id).map_or(0, |c| c.tracks), artist_id: m.id, name: m.name })
        .collect();
    members.sort_by(|a, b| b.tracks.cmp(&a.tracks).then(a.name.cmp(&b.name)));
    let credited: Vec<Member> = credited_all.into_iter().filter(|c| !members.iter().any(|m| m.artist_id == c.artist_id)).collect();
    let sources = super::sources::list_for(conn, id).await?;
    let extras = super::extras::linked_to(conn, super::extras::Link::Grouping(id)).await?;
    let changes = sqlx::query_as!(
        Change,
        r#"SELECT l.at, u.username AS "by?", l.action, l.track_id, l.album_id, l.artist_id, l.detail, l.reason
           FROM grouping_log l LEFT JOIN users u ON u.id = l.by_user
           WHERE l.grouping_id = $1 ORDER BY l.id DESC"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(Detail { id: g.id, kind: g.kind, name: g.name, description: g.description, created_at: g.created_at, tracks, albums, members, credited, sources, extras, changes }))
}

// ---- An artist that is really a collective ----

/// Something named, for a preview.
#[derive(Debug, Serialize)]
pub struct Named {
    pub id: Uuid,
    pub title: String,
}

/// What `turn_into_collective` would do, changing nothing (what the user can see of it).
#[derive(Debug, Serialize)]
pub struct CollectivePreview {
    pub artist_id: Uuid,
    pub name: String,
    /// The collective it goes into (None: a new one, with its name).
    pub into: Option<Named>,
    /// Its sources (the accounts), which move to the collective: ingest then puts new
    /// songs from them into it.
    pub sources: Vec<super::sources::Source>,
    /// Tracks and albums it's credited on: they go in the collective, and lose its credit.
    pub tracks: i64,
    pub albums: i64,
    /// Of those, the ones no one else is credited on as a primary artist: the server flags
    /// those with nothing left as missing artists, for Review.
    pub tracks_left_without_primary: Vec<Named>,
    pub albums_left_without_artists: Vec<Named>,
    /// Extras linked to it (taken-off ones too): they're linked to the collective instead.
    pub extras: Vec<Named>,
}

/// The latest data of each version (and album) crediting `artist`: what's rewritten.
async fn crediting(conn: &mut PgConnection, artist: Uuid) -> sqlx::Result<(Vec<(Uuid, Uuid, TrackMetadata)>, Vec<(Uuid, AlbumData)>)> {
    let pattern = serde_json::json!({ "credits": [{ "artist_id": artist }] });
    let variants = sqlx::query!(
        r#"
        SELECT r.track_id, r.variant_id AS "variant_id!", r.data
        FROM track_revisions r
        JOIN tracks t ON t.id = r.track_id AND t.merged_into IS NULL
        WHERE r.data @> $1
          AND r.id = (SELECT r2.id FROM track_revisions r2 WHERE r2.variant_id = r.variant_id ORDER BY r2.id DESC LIMIT 1)
        "#,
        pattern
    )
    .fetch_all(&mut *conn)
    .await?;
    let albums = sqlx::query!(
        r#"
        SELECT r.album_id, r.data
        FROM album_revisions r
        JOIN albums a ON a.id = r.album_id AND a.merged_into IS NULL
        WHERE r.data @> $1
          AND r.id = (SELECT r2.id FROM album_revisions r2 WHERE r2.album_id = r.album_id ORDER BY r2.id DESC LIMIT 1)
        "#,
        pattern
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut tracks = Vec::new();
    for v in variants {
        tracks.push((v.track_id, v.variant_id, serde_json::from_value(v.data).map_err(|e| sqlx::Error::Decode(Box::new(e)))?));
    }
    let mut out_albums = Vec::new();
    for a in albums {
        out_albums.push((a.album_id, serde_json::from_value(a.data).map_err(|e| sqlx::Error::Decode(Box::new(e)))?));
    }
    Ok((tracks, out_albums))
}

async fn artist_name(conn: &mut PgConnection, artist: Uuid) -> anyhow::Result<String> {
    let row = sqlx::query!("SELECT name, merged_into FROM artists WHERE id = $1", artist)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("artist"))?;
    if row.merged_into.is_some() {
        return Err(invalid("that artist was merged into another"));
    }
    Ok(row.name)
}

/// The grouping an artist goes into, if one is given: it must exist.
async fn target(conn: &mut PgConnection, into: Option<Uuid>) -> anyhow::Result<Option<Named>> {
    let Some(id) = into else { return Ok(None) };
    let name = sqlx::query_scalar!("SELECT name FROM groupings WHERE id = $1", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("grouping"))?;
    Ok(Some(Named { id, title: name }))
}

/// `into`: an existing collective it goes into (the same collective on another platform,
/// say); None: a new one.
pub async fn collective_preview(conn: &mut PgConnection, artist: Uuid, into: Option<Uuid>) -> anyhow::Result<CollectivePreview> {
    let name = artist_name(conn, artist).await?;
    let into = target(conn, into).await?;
    let (variants, albums) = crediting(conn, artist).await?;
    let mut tracks: Vec<Uuid> = variants.iter().map(|v| v.0).collect();
    tracks.sort();
    tracks.dedup();
    let mut left: Vec<Named> = Vec::new();
    for (track, _, data) in &variants {
        let others = data.credits.iter().any(|c| c.artist_id != artist && c.role == "primary");
        if !others && !left.iter().any(|l| l.id == *track) {
            left.push(Named { id: *track, title: data.title.clone() });
        }
    }
    let albums_left = albums
        .iter()
        .filter(|(_, data)| !data.credits.iter().any(|c| c.artist_id != artist))
        .map(|(id, data)| Named { id: *id, title: data.title.clone() })
        .collect();
    Ok(CollectivePreview {
        artist_id: artist,
        name,
        into,
        sources: super::sources::list_for(conn, artist).await?,
        tracks: tracks.len() as i64,
        albums: albums.len() as i64,
        tracks_left_without_primary: left,
        albums_left_without_artists: albums_left,
        extras: sqlx::query_as!(
            Named,
            r#"SELECT e.id, e.name AS title FROM extras e
               WHERE EXISTS (SELECT 1 FROM extra_links l WHERE l.extra_id = e.id AND l.artist_id = $1)
               ORDER BY lower(e.name), e.id"#,
            artist
        )
        .fetch_all(&mut *conn)
        .await?,
    })
}

/// Turns an artist that is really a collective into one: a grouping of kind `collective`
/// with its name; its sources move there (so ingest puts new songs from those accounts in
/// it); every track and album crediting it goes in, and its credit comes off them (a new
/// revision each, restorable); its extras are linked to the collective instead. The artist
/// stays, credited nowhere. Returns the grouping.
///
/// Tracks, albums and extras the user can't see change too (as for an artist merge), or
/// they'd stay with it: that part runs as the system.
///
/// `into`: an existing collective it goes into instead of a new one (the same collective,
/// as another platform's account made another artist of it): its sources join that one's
/// (one with a platform id the collective already has joins that source).
pub async fn turn_into_collective(conn: &mut PgConnection, artist: Uuid, into: Option<Uuid>, reason: &str) -> anyhow::Result<Uuid> {
    let name = artist_name(conn, artist).await?;
    let into = target(conn, into).await?;
    let reason = match (&into, reason.trim()) {
        (_, given) if !given.is_empty() => given.to_string(),
        (Some(into), _) => format!("\"{name}\" is the collective \"{}\"", into.title),
        (None, _) => format!("\"{name}\" is a collective, not an artist"),
    };
    let grouping = match &into {
        Some(into) => into.id,
        None => create(conn, "collective", &name, "", &reason).await?,
    };

    let system = super::as_system(conn).await?;
    let (variants, albums) = crediting(conn, artist).await?;
    let mut tracks: Vec<Uuid> = Vec::new();
    for (track, variant, mut data) in variants {
        data.credits.retain(|c| c.artist_id != artist);
        write_internal(conn, track, variant, None, &data, Source::Edit, &reason, false).await?;
        if !tracks.contains(&track) {
            tracks.push(track);
        }
    }
    let mut album_ids = Vec::new();
    for (album, mut data) in albums {
        data.credits.retain(|c| c.artist_id != artist);
        write_album(conn, album, None, &data, Source::Edit, &reason).await?;
        album_ids.push(album);
    }
    for &track in &tracks {
        sqlx::query!("INSERT INTO grouping_tracks (grouping_id, track_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", grouping, track)
            .execute(&mut *conn)
            .await?;
    }
    for &album in &album_ids {
        sqlx::query!("INSERT INTO grouping_albums (grouping_id, album_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", grouping, album)
            .execute(&mut *conn)
            .await?;
    }
    // Its accounts are the collective's now (one it has already joins that one).
    let sources = super::sources::move_all(conn, crate::model::sources::SourceOwner::Artist(artist), crate::model::sources::SourceOwner::Grouping(grouping)).await?;
    let extras = super::extras::move_to_grouping(conn, artist, grouping, &name, &reason).await?;
    let detail = serde_json::json!({ "artist_id": artist, "name": name, "sources": sources, "tracks": tracks, "albums": album_ids, "extras": extras, "into_existing": into.is_some() });
    log(conn, grouping, "from_artist", None, None, detail, &reason).await?;
    system.end(conn).await?;
    Ok(grouping)
}
