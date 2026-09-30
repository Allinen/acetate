//! Extras (migration 0040): audio and other files that aren't tracks (vocals, stems,
//! samples, clips, project files), with a name, a kind, their files, and links to the
//! tracks, albums and artists they belong to. Restricted like tracks. Nothing is deleted:
//! an extra or a file is taken off (hidden) and brought back; links are made and undone;
//! everything is logged (`extra_log`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// What an extra is linked to.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum Link {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Link {
    fn ids(self) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
        match self {
            Link::Track(id) => (Some(id), None, None),
            Link::Album(id) => (None, Some(id), None),
            Link::Artist(id) => (None, None, Some(id)),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Link::Track(_) => "track",
            Link::Album(_) => "album",
            Link::Artist(_) => "artist",
        }
    }
}

/// What's said about an extra besides its name and kind: a description, and where it came
/// from (a link, or in words).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct About {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
}

impl About {
    fn checked(&self) -> anyhow::Result<About> {
        let description = self.description.trim().to_string();
        let source = self.source.trim().to_string();
        if description.len() > 100_000 || source.len() > 2_000 {
            return Err(EditRejected::Invalid("that description or source is too long".into()).into());
        }
        Ok(About { description, source })
    }
}

/// Who sees an extra: everyone, or (restricted) admins and these groups.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Access {
    pub restricted: bool,
    #[serde(default)]
    pub groups: Vec<Uuid>,
}

/// One of an extra's files.
#[derive(Debug, Serialize)]
pub struct ExtraFile {
    pub id: Uuid,
    pub name: String,
    pub size_bytes: i64,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub bit_depth: Option<i16>,
    pub duration_ms: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f32>,
    pub video_codec: Option<String>,
    /// A video's thumbnail: the start of its image's hash (`GET /api/images/{hash}`).
    pub thumb: Option<String>,
    pub hidden: bool,
    pub added_at: DateTime<Utc>,
}

/// Something it's linked to, named.
#[derive(Debug, Serialize, PartialEq)]
pub struct Linked {
    pub kind: String,
    pub id: Uuid,
    pub title: String,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
}

/// An extra, as its page (or a page it's linked to) shows it.
#[derive(Debug, Serialize)]
pub struct Extra {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub description: String,
    pub source: String,
    /// Made from this track (hidden while it's an extra; `back_to_track` undoes it).
    pub from_track: Option<Uuid>,
    pub is_restricted: bool,
    /// Who sees it when restricted (besides admins).
    pub groups: Vec<Group>,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub files: Vec<ExtraFile>,
    pub links: Vec<Linked>,
}

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub what: String,
    pub details: serde_json::Value,
    pub reason: String,
    pub at: DateTime<Utc>,
    pub by: Option<String>,
}

/// A file's technical facts (audio, video), as probed, and a video's thumbnail (stored
/// already: its hash and volume).
#[derive(Debug, Default)]
pub struct FileFacts {
    pub container: Option<String>,
    /// The sound's codec.
    pub codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub bit_depth: Option<i16>,
    pub duration_ms: Option<i64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f32>,
    pub video_codec: Option<String>,
    pub thumb: Option<(String, String)>,
    /// Where it was imported from (the server's disk).
    pub imported_from: Option<String>,
}

fn checked_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EditRejected::Invalid("an extra needs a name".into()).into());
    }
    Ok(name.chars().take(500).collect())
}

fn checked_kind(kind: &str) -> String {
    let kind = kind.trim().to_lowercase();
    if kind.is_empty() { "other".into() } else { kind.chars().take(60).collect() }
}

async fn log(conn: &mut PgConnection, extra: Uuid, what: &str, details: serde_json::Value, reason: &str) -> sqlx::Result<()> {
    sqlx::query!("INSERT INTO extra_log (extra_id, what, details, reason) VALUES ($1, $2, $3, $4)", extra, what, details, reason.trim())
        .execute(conn)
        .await?;
    Ok(())
}

/// Who sees a track: the access a new extra linked to it starts with.
pub async fn access_of_track(conn: &mut PgConnection, track: Uuid) -> anyhow::Result<Access> {
    let restricted = sqlx::query_scalar!("SELECT is_restricted FROM tracks WHERE id = $1 AND merged_into IS NULL", track)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("track"))?;
    let groups = if restricted {
        sqlx::query_scalar!("SELECT group_id FROM track_access WHERE track_id = $1 ORDER BY group_id", track).fetch_all(&mut *conn).await?
    } else {
        Vec::new()
    };
    Ok(Access { restricted, groups })
}

/// A new extra, linked to `links` (each must be there and not merged away). `access`: who
/// sees it; None: like the first linked track (everyone, without one). Returns its id and
/// the access it got.
pub async fn create(conn: &mut PgConnection, name: &str, kind: &str, about: &About, links: &[Link], access: Option<Access>, reason: &str) -> anyhow::Result<(Uuid, Access)> {
    let name = checked_name(name)?;
    let kind = checked_kind(kind);
    let about = about.checked()?;
    let access = match access {
        Some(access) => access,
        None => match links.iter().find_map(|l| if let Link::Track(t) = l { Some(*t) } else { None }) {
            Some(track) => access_of_track(conn, track).await?,
            None => Access::default(),
        },
    };
    // Made open, its groups added, then restricted (in this transaction: nobody sees it in
    // between). Restricted from the start, the visibility check would refuse the new row
    // (no group sees it yet).
    let id = sqlx::query_scalar!(
        "INSERT INTO extras (name, kind, description, source) VALUES ($1, $2, $3, $4) RETURNING id",
        name,
        kind,
        about.description,
        about.source
    )
    .fetch_one(&mut *conn)
    .await?;
    log(conn, id, "made", json!({ "name": name, "kind": kind, "about": about, "access": access }), reason).await?;
    for link in links {
        self::link(conn, id, *link, reason).await?;
    }
    write_access(conn, id, &access).await?;
    if access.restricted {
        set_restricted(conn, id, true).await?;
    }
    Ok((id, access))
}

async fn write_access(conn: &mut PgConnection, extra: Uuid, access: &Access) -> anyhow::Result<()> {
    if access.restricted {
        let known = sqlx::query_scalar!("SELECT count(*) AS \"n!\" FROM groups WHERE id = ANY($1)", &access.groups).fetch_one(&mut *conn).await?;
        if known as usize != access.groups.len() {
            return Err(EditRejected::Invalid("one of those groups isn't there".into()).into());
        }
    }
    sqlx::query!("DELETE FROM extra_access WHERE extra_id = $1", extra).execute(&mut *conn).await?;
    if access.restricted {
        for group in &access.groups {
            sqlx::query!("INSERT INTO extra_access (extra_id, group_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", extra, group).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

/// Who sees it, changed. (Before the change: restricting it to groups the user isn't in
/// would hide it from them, which is allowed; the change is logged.)
pub async fn set_access(conn: &mut PgConnection, extra: Uuid, access: &Access, reason: &str) -> anyhow::Result<bool> {
    let now = sqlx::query_scalar!("SELECT is_restricted FROM extras WHERE id = $1 FOR UPDATE", extra)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("extra"))?;
    let groups = sqlx::query_scalar!("SELECT group_id FROM extra_access WHERE extra_id = $1 ORDER BY group_id", extra).fetch_all(&mut *conn).await?;
    let mut wanted = access.groups.clone();
    wanted.sort();
    wanted.dedup();
    let wanted = Access { restricted: access.restricted, groups: if access.restricted { wanted } else { Vec::new() } };
    if now == wanted.restricted && groups == wanted.groups {
        return Ok(false);
    }
    // The log first: once it's restricted away from the user, they can't write it.
    log(conn, extra, "access", json!({ "from": Access { restricted: now, groups }, "to": wanted }), reason).await?;
    // In the order that keeps it visible to the user until the last step: opening it up,
    // unrestricted first; restricting it, its groups first. (The other way round, the
    // user loses sight of it halfway, and the rest silently changes nothing.)
    if wanted.restricted {
        write_access(conn, extra, &wanted).await?;
        set_restricted(conn, extra, true).await?;
    } else {
        set_restricted(conn, extra, false).await?;
        write_access(conn, extra, &wanted).await?;
    }
    Ok(true)
}

async fn set_restricted(conn: &mut PgConnection, extra: Uuid, restricted: bool) -> anyhow::Result<()> {
    let hidden_from_you = || EditRejected::Invalid("that would hide it from you: restrict it to a group you're in".into());
    let done = match sqlx::query!("UPDATE extras SET is_restricted = $2 WHERE id = $1", extra, restricted).execute(&mut *conn).await {
        Ok(done) => done.rows_affected(),
        // The row it would become isn't one the user may see.
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("42501") => return Err(hidden_from_you().into()),
        Err(err) => return Err(err.into()),
    };
    if done == 0 {
        return Err(hidden_from_you().into());
    }
    Ok(())
}

/// Its name, kind, description and source.
pub async fn edit(conn: &mut PgConnection, extra: Uuid, name: &str, kind: &str, about: &About, reason: &str) -> anyhow::Result<bool> {
    let (name, kind, about) = (checked_name(name)?, checked_kind(kind), about.checked()?);
    let now = sqlx::query!("SELECT name, kind, description, source FROM extras WHERE id = $1 FOR UPDATE", extra)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("extra"))?;
    if now.name == name && now.kind == kind && now.description == about.description && now.source == about.source {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE extras SET name = $2, kind = $3, description = $4, source = $5 WHERE id = $1",
        extra,
        name,
        kind,
        about.description,
        about.source
    )
    .execute(&mut *conn)
    .await?;
    let from = json!({ "name": now.name, "kind": now.kind, "description": now.description, "source": now.source });
    let to = json!({ "name": name, "kind": kind, "description": about.description, "source": about.source });
    log(conn, extra, "edited", json!({ "from": from, "to": to }), reason).await?;
    Ok(true)
}

/// Taken off (kept) or brought back.
pub async fn set_hidden(conn: &mut PgConnection, extra: Uuid, hidden: bool, reason: &str) -> anyhow::Result<bool> {
    let now = sqlx::query_scalar!("SELECT hidden_at IS NOT NULL AS \"h!\" FROM extras WHERE id = $1 FOR UPDATE", extra)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("extra"))?;
    if now == hidden {
        return Ok(false);
    }
    sqlx::query!("UPDATE extras SET hidden_at = CASE WHEN $2 THEN now() END WHERE id = $1", extra, hidden).execute(&mut *conn).await?;
    log(conn, extra, if hidden { "taken off" } else { "brought back" }, json!({}), reason).await?;
    Ok(true)
}

/// What goes along with a link: a track alone on a single and that single are one release
/// (as for groupings, credits, covers…): linking (or unlinking) one does the other too.
async fn along(conn: &mut PgConnection, with: Link) -> sqlx::Result<Vec<Link>> {
    Ok(match with {
        Link::Track(track) => super::groupings::singles_of(conn, track).await?.into_iter().map(Link::Album).collect(),
        Link::Album(album) => super::groupings::only_track_of(conn, album).await?.map(Link::Track).into_iter().collect(),
        Link::Artist(_) => Vec::new(),
    })
}

/// Links it to a track, album or artist (there, and not merged away), and what goes along
/// (a track's single it's alone on, a single's only track). False: linked already.
pub async fn link(conn: &mut PgConnection, extra: Uuid, to: Link, reason: &str) -> anyhow::Result<bool> {
    let made = link_one(conn, extra, to, reason).await?;
    for other in along(conn, to).await? {
        link_one(conn, extra, other, reason).await?;
    }
    Ok(made)
}

async fn link_one(conn: &mut PgConnection, extra: Uuid, to: Link, reason: &str) -> anyhow::Result<bool> {
    let merged = match to {
        Link::Track(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM tracks WHERE id = $1", id).fetch_optional(&mut *conn).await?,
        Link::Album(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM albums WHERE id = $1", id).fetch_optional(&mut *conn).await?,
        Link::Artist(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM artists WHERE id = $1", id).fetch_optional(&mut *conn).await?,
    };
    match merged {
        None => return Err(EditRejected::NotFound(to.name()).into()),
        Some(true) => return Err(EditRejected::Invalid(format!("that {} was merged into another: link that one", to.name())).into()),
        Some(false) => {}
    }
    let (track, album, artist) = to.ids();
    let made = sqlx::query!(
        "INSERT INTO extra_links (extra_id, track_id, album_id, artist_id) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        extra,
        track,
        album,
        artist
    )
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    if made {
        log(conn, extra, "linked", json!({ "to": to }), reason).await?;
    }
    Ok(made)
}

/// Undoes a link, and those that went along (a track's single it's alone on, a single's
/// only track). False: it wasn't linked (or the other side isn't seen).
pub async fn unlink(conn: &mut PgConnection, extra: Uuid, from: Link, reason: &str) -> anyhow::Result<bool> {
    let gone = unlink_one(conn, extra, from, reason).await?;
    for other in along(conn, from).await? {
        unlink_one(conn, extra, other, reason).await?;
    }
    Ok(gone)
}

async fn unlink_one(conn: &mut PgConnection, extra: Uuid, from: Link, reason: &str) -> anyhow::Result<bool> {
    let (track, album, artist) = from.ids();
    // A track's link: also one made to a track merged into it since.
    let gone = sqlx::query!(
        r#"DELETE FROM extra_links WHERE extra_id = $1
           AND (($2::uuid IS NOT NULL AND track_id IS NOT NULL AND (track_id = $2 OR final_track(track_id) = $2))
                OR ($3::uuid IS NOT NULL AND album_id = $3)
                OR ($4::uuid IS NOT NULL AND artist_id = $4))"#,
        extra,
        track,
        album,
        artist
    )
    .execute(&mut *conn)
    .await?
    .rows_affected()
        > 0;
    if gone {
        log(conn, extra, "unlinked", json!({ "from": from }), reason).await?;
    }
    Ok(gone)
}

/// A file (its bytes stored already, and registered) added to an extra.
pub async fn add_file(conn: &mut PgConnection, extra: Uuid, sha256: &str, name: &str, size_bytes: i64, facts: &FileFacts, reason: &str) -> anyhow::Result<Uuid> {
    sqlx::query_scalar!("SELECT id FROM extras WHERE id = $1", extra).fetch_optional(&mut *conn).await?.ok_or(EditRejected::NotFound("extra"))?;
    if let Some((thumb, volume)) = &facts.thumb {
        super::blobs::register(conn, thumb, volume, None).await?;
    }
    let id = sqlx::query_scalar!(
        r#"INSERT INTO extra_files (extra_id, sha256, name, size_bytes, container, codec, sample_rate, channels, bit_depth, duration_ms,
                                    width, height, frame_rate, video_codec, thumb_sha256, imported_from)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING id"#,
        extra,
        sha256,
        name,
        size_bytes,
        facts.container,
        facts.codec,
        facts.sample_rate,
        facts.channels,
        facts.bit_depth,
        facts.duration_ms,
        facts.width,
        facts.height,
        facts.frame_rate,
        facts.video_codec,
        facts.thumb.as_ref().map(|t| t.0.clone()),
        facts.imported_from
    )
    .fetch_one(&mut *conn)
    .await?;
    log(conn, extra, "file added", json!({ "file": id, "name": name, "sha256": sha256 }), reason).await?;
    Ok(id)
}

/// A file's facts (a video's, read after it was added), and its thumbnail.
pub async fn set_file_facts(conn: &mut PgConnection, file: Uuid, facts: &FileFacts) -> anyhow::Result<()> {
    if let Some((thumb, volume)) = &facts.thumb {
        super::blobs::register(conn, thumb, volume, None).await?;
    }
    sqlx::query!(
        r#"UPDATE extra_files SET container = COALESCE($2, container), codec = COALESCE($3, codec), sample_rate = COALESCE($4, sample_rate),
                  channels = COALESCE($5, channels), duration_ms = COALESCE($6, duration_ms), width = $7, height = $8, frame_rate = $9,
                  video_codec = $10, thumb_sha256 = $11
           WHERE id = $1"#,
        file,
        facts.container,
        facts.codec,
        facts.sample_rate,
        facts.channels,
        facts.duration_ms,
        facts.width,
        facts.height,
        facts.frame_rate,
        facts.video_codec,
        facts.thumb.as_ref().map(|t| t.0.clone())
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// A file taken off (kept) or brought back.
pub async fn set_file_hidden(conn: &mut PgConnection, file: Uuid, hidden: bool, reason: &str) -> anyhow::Result<bool> {
    let row = sqlx::query!("SELECT extra_id, name, hidden_at IS NOT NULL AS \"h!\" FROM extra_files WHERE id = $1 FOR UPDATE", file)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("file"))?;
    if row.h == hidden {
        return Ok(false);
    }
    sqlx::query!("UPDATE extra_files SET hidden_at = CASE WHEN $2 THEN now() END WHERE id = $1", file, hidden).execute(&mut *conn).await?;
    log(conn, row.extra_id, if hidden { "file taken off" } else { "file brought back" }, json!({ "file": file, "name": row.name }), reason).await?;
    Ok(true)
}

/// Where a file's bytes are, and its name (to stream it), if the user sees it.
pub async fn file_blob(conn: &mut PgConnection, file: Uuid) -> sqlx::Result<Option<(String, String)>> {
    Ok(sqlx::query!("SELECT sha256, name FROM extra_files WHERE id = $1", file).fetch_optional(conn).await?.map(|r| (r.sha256, r.name)))
}

/// One extra.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<Extra>> {
    let Some(row) = sqlx::query!(
        r#"SELECT e.id, e.name, e.kind, e.description, e.source, e.from_track_id, e.is_restricted,
                  e.hidden_at IS NOT NULL AS "hidden!", e.created_at, u.username AS "created_by?"
           FROM extras e LEFT JOIN users u ON u.id = e.created_by WHERE e.id = $1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(Extra {
        id: row.id,
        name: row.name,
        kind: row.kind,
        description: row.description,
        source: row.source,
        from_track: row.from_track_id,
        is_restricted: row.is_restricted,
        groups: groups(conn, id).await?,
        hidden: row.hidden,
        created_at: row.created_at,
        created_by: row.created_by,
        files: files(conn, id).await?,
        links: links(conn, id).await?,
    }))
}

/// The extras linked to a track, album or artist (taken-off ones too, flagged), by name.
/// A track's include those linked to tracks merged into it (links stay where they were
/// made: undoing the merge needs nothing).
pub async fn linked_to(conn: &mut PgConnection, to: Link) -> sqlx::Result<Vec<Extra>> {
    let (track, album, artist) = to.ids();
    let ids = sqlx::query_scalar!(
        r#"SELECT e.id FROM extras e
           WHERE EXISTS (SELECT 1 FROM extra_links l WHERE l.extra_id = e.id
                         AND (($1::uuid IS NOT NULL AND l.track_id IS NOT NULL AND final_track(l.track_id) = $1)
                              OR ($2::uuid IS NOT NULL AND l.album_id = $2)
                              OR ($3::uuid IS NOT NULL AND l.artist_id = $3)))
           ORDER BY lower(e.name), e.id"#,
        track,
        album,
        artist
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(extra) = get(conn, id).await? {
            out.push(extra);
        }
    }
    Ok(out)
}

/// An extra's log, newest first.
pub async fn history(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Vec<LogEntry>> {
    sqlx::query_as!(
        LogEntry,
        r#"SELECT l.what, l.details, l.reason, l.at, u.username AS "by?"
           FROM extra_log l LEFT JOIN users u ON u.id = l.by_user WHERE l.extra_id = $1 ORDER BY l.id DESC"#,
        id
    )
    .fetch_all(conn)
    .await
}

async fn groups(conn: &mut PgConnection, extra: Uuid) -> sqlx::Result<Vec<Group>> {
    sqlx::query_as!(Group, "SELECT g.id, g.name FROM extra_access a JOIN groups g ON g.id = a.group_id WHERE a.extra_id = $1 ORDER BY g.name", extra)
        .fetch_all(conn)
        .await
}

async fn files(conn: &mut PgConnection, extra: Uuid) -> sqlx::Result<Vec<ExtraFile>> {
    sqlx::query_as!(
        ExtraFile,
        r#"SELECT id, name, size_bytes, container, codec, sample_rate, channels, bit_depth, duration_ms,
                  width, height, frame_rate, video_codec, left(thumb_sha256, 16) AS thumb,
                  hidden_at IS NOT NULL AS "hidden!", added_at
           FROM extra_files WHERE extra_id = $1 ORDER BY lower(name), id"#,
        extra
    )
    .fetch_all(conn)
    .await
}

async fn links(conn: &mut PgConnection, extra: Uuid) -> sqlx::Result<Vec<Linked>> {
    sqlx::query_as!(
        Linked,
        r#"SELECT s.kind AS "kind!", s.id AS "id!", s.title AS "title!" FROM (
               SELECT DISTINCT ON (COALESCE(t.id, l.album_id, l.artist_id))
                      CASE WHEN l.track_id IS NOT NULL THEN 'track' WHEN l.album_id IS NOT NULL THEN 'album' ELSE 'artist' END AS kind,
                      COALESCE(t.id, l.album_id, l.artist_id) AS id,
                      COALESCE(t.title, al.title, a.name) AS title, l.id AS link
               FROM extra_links l
               -- A track merged since: the one it went into.
               LEFT JOIN tracks t ON t.id = final_track(l.track_id)
               LEFT JOIN albums al ON al.id = l.album_id
               LEFT JOIN artists a ON a.id = l.artist_id
               WHERE l.extra_id = $1 AND COALESCE(t.id, al.id, a.id) IS NOT NULL
               ORDER BY COALESCE(t.id, l.album_id, l.artist_id), l.id
           ) s ORDER BY s.link"#,
        extra
    )
    .fetch_all(conn)
    .await
}

// ---- A track becoming an extra, and back ----

/// An album a track was on, and whether it had no other track (then it's hidden with it).
#[derive(Debug, Serialize, PartialEq)]
pub struct AlbumLeft {
    pub id: Uuid,
    pub title: String,
    pub alone: bool,
}

/// What turning a track into an extra does (or, previewed, would).
#[derive(Debug, Serialize)]
pub struct FromTrack {
    pub extra_id: Uuid,
    pub name: String,
    pub kind: String,
    pub source: String,
    /// The file it takes (the track's, the same bytes).
    pub file: String,
    pub file_id: Uuid,
    /// Its bytes (to examine it: a video gets its facts and thumbnail after).
    #[serde(skip)]
    pub sha256: String,
    pub links: Vec<Linked>,
    pub access: Access,
    pub albums: Vec<AlbumLeft>,
}

/// Turns a track into an extra: an extra named after it (or `name`), of `kind`, with its
/// file (the same bytes), seen by those who see the track, linked to `belongs_to` (the song
/// it's part of, if said) and to the track's primary artists, its source said in words
/// (unless `about` says one). The track stays, with everything on it, but nobody sees it
/// anymore (see migration 0042); `back_to_track` undoes it.
pub async fn track_to_extra(conn: &mut PgConnection, track: Uuid, kind: &str, name: Option<&str>, about: &About, belongs_to: Option<Uuid>, reason: &str) -> anyhow::Result<FromTrack> {
    let row = sqlx::query!(
        r#"SELECT t.title, f.sha256, COALESCE(f.original_filename, f.uploaded_filename) AS "file_name!", f.size_bytes,
                  f.container, f.codec, f.sample_rate, f.channels, f.bit_depth, f.duration_ms,
                  src.platform AS "platform?", src.external_id AS "external_id?"
           FROM tracks t
           JOIN files f ON f.id = t.file_id
           LEFT JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id
           LEFT JOIN sources src ON src.id = ss.source_id
           WHERE t.id = $1 AND t.merged_into IS NULL"#,
        track
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(EditRejected::NotFound("track"))?;
    if belongs_to == Some(track) {
        return Err(EditRejected::Invalid("it can't belong to itself: choose the song it's part of".into()).into());
    }
    let artists = sqlx::query_scalar!("SELECT artist_id FROM track_credits WHERE track_id = $1 AND role = 'primary' ORDER BY position", track)
        .fetch_all(&mut *conn)
        .await?;
    let albums = sqlx::query_as!(
        AlbumLeft,
        r#"SELECT a.id, a.title,
                  NOT EXISTS (SELECT 1 FROM album_tracks x JOIN tracks o ON o.id = x.track_id
                              WHERE x.album_id = a.id AND x.track_id <> $1) AS "alone!"
           FROM album_tracks at JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
           WHERE at.track_id = $1 ORDER BY a.title"#,
        track
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut about = about.clone();
    if about.source.trim().is_empty()
        && let Some(platform) = &row.platform
    {
        about.source = match &row.external_id {
            Some(id) => format!("{platform} {id} (the track's source)"),
            None => format!("{platform} (the track's source)"),
        };
    }
    let mut links: Vec<Link> = belongs_to.map(Link::Track).into_iter().collect();
    links.extend(artists.into_iter().map(Link::Artist));
    let access = access_of_track(conn, track).await?;
    let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&row.title).to_string();
    let (extra, access) = create(conn, &name, kind, &about, &links, Some(access), reason).await?;
    let facts = FileFacts {
        container: row.container,
        codec: row.codec,
        sample_rate: row.sample_rate,
        channels: row.channels,
        bit_depth: row.bit_depth,
        duration_ms: row.duration_ms,
        ..Default::default()
    };
    let file_id = add_file(conn, extra, &row.sha256, &row.file_name, row.size_bytes, &facts, reason).await?;
    log(conn, extra, "made from a track", json!({ "track": track, "title": row.title }), reason).await?;
    sqlx::query!("SELECT track_became_extra($1, $2)", track, extra).execute(&mut *conn).await?;
    let made = get(conn, extra).await?.ok_or(EditRejected::NotFound("extra"))?;
    Ok(FromTrack {
        extra_id: extra,
        name: made.name,
        kind: made.kind,
        source: made.source,
        file: row.file_name,
        file_id,
        sha256: row.sha256,
        links: made.links,
        access,
        albums,
    })
}

/// Undoes `track_to_extra`: the track is seen again (and its albums), the extra taken off
/// (kept). Returns the track.
pub async fn back_to_track(conn: &mut PgConnection, extra: Uuid, reason: &str) -> anyhow::Result<Uuid> {
    // Logged first: it's there to write to either way.
    log(conn, extra, "made a track again", json!({}), reason).await?;
    match sqlx::query_scalar!(r#"SELECT extra_back_to_track($1) AS "track!""#, extra).fetch_one(&mut *conn).await {
        Ok(track) => Ok(track),
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some("P0002") => Err(EditRejected::NotFound("extra made from a track").into()),
        Err(err) => Err(err.into()),
    }
}

/// The extra a track became (made into an extra, so hidden), if the user can see it.
pub async fn became(conn: &mut PgConnection, track: Uuid) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar!(r#"SELECT id AS "id!" FROM extras WHERE id = track_became_extra($1)"#, track)
        .fetch_optional(conn)
        .await
}
