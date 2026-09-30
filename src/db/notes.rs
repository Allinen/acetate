//! Notes on tracks, albums and artists (migration 0039): any number each, a title and
//! free text. Every change is a revision; a note is never deleted, only taken off (hidden)
//! and brought back. An item's notes include those of what was merged into it.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// What a note is written on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Owner {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Owner {
    fn kind(self) -> &'static str {
        match self {
            Owner::Track(_) => "track",
            Owner::Album(_) => "album",
            Owner::Artist(_) => "artist",
        }
    }

    fn id(self) -> Uuid {
        match self {
            Owner::Track(id) | Owner::Album(id) | Owner::Artist(id) => id,
        }
    }
}

/// A note, as a page shows it.
#[derive(Debug, Serialize)]
pub struct Note {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    /// Taken off: shown only to those who can bring it back.
    pub hidden: bool,
    /// The latest revision: the base for editing it.
    pub revision_id: Uuid,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    /// Written on something merged into this one since: its id.
    pub merged_from: Option<Uuid>,
}

/// A note's revision.
#[derive(Debug, Serialize)]
pub struct Revision {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub hidden: bool,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<String>,
    pub reason: String,
}

/// How long a note may be (a long story fits; a whole book doesn't).
const MAX_BODY: usize = 1_000_000;
const MAX_TITLE: usize = 500;

fn checked(title: &str, body: &str) -> anyhow::Result<(String, String)> {
    let title = title.trim().to_string();
    // The body as written (its own spacing kept), without blank lines around it.
    let body = body.trim_matches(|c| c == '\n' || c == '\r').trim_end().to_string();
    if title.is_empty() && body.trim().is_empty() {
        return Err(EditRejected::Invalid("a note needs a title or some text".into()).into());
    }
    if title.chars().count() > MAX_TITLE {
        return Err(EditRejected::Invalid(format!("a note's title is at most {MAX_TITLE} characters")).into());
    }
    if body.len() > MAX_BODY {
        return Err(EditRejected::Invalid("a note is at most 1 MB of text".into()).into());
    }
    Ok((title, body))
}

/// The notes on `owner` and on what was merged into it (hidden ones too, flagged), oldest
/// first.
pub async fn list(conn: &mut PgConnection, owner: Owner) -> sqlx::Result<Vec<Note>> {
    sqlx::query_as!(
        Note,
        r#"
        WITH RECURSIVE ids(id) AS (
            SELECT $2::uuid
            UNION
            SELECT x.id
            FROM (SELECT id, merged_into FROM tracks WHERE $1 = 'track'
                  UNION ALL SELECT id, merged_into FROM albums WHERE $1 = 'album'
                  UNION ALL SELECT id, merged_into FROM artists WHERE $1 = 'artist') x
            JOIN ids ON x.merged_into = ids.id
        )
        SELECT n.id, n.title, n.body, n.hidden_at IS NOT NULL AS "hidden!",
               r.id AS "revision_id!", r.created_at AS "updated_at!", ru.username AS "updated_by?",
               n.created_at, cu.username AS "created_by?",
               NULLIF(COALESCE(n.track_id, n.album_id, n.artist_id), $2) AS merged_from
        FROM notes n
        JOIN LATERAL (SELECT id, created_at, created_by FROM note_revisions WHERE note_id = n.id ORDER BY id DESC LIMIT 1) r ON true
        LEFT JOIN users ru ON ru.id = r.created_by
        LEFT JOIN users cu ON cu.id = n.created_by
        WHERE CASE $1 WHEN 'track' THEN n.track_id WHEN 'album' THEN n.album_id ELSE n.artist_id END IN (SELECT id FROM ids)
        ORDER BY n.id
        "#,
        owner.kind(),
        owner.id()
    )
    .fetch_all(conn)
    .await
}

/// A new note on `owner` (not merged into another: write on that one). Returns (the note,
/// its first revision).
pub async fn create(conn: &mut PgConnection, owner: Owner, title: &str, body: &str, reason: &str) -> anyhow::Result<(Uuid, Uuid)> {
    let (title, body) = checked(title, body)?;
    let merged = match owner {
        Owner::Track(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM tracks WHERE id = $1", id).fetch_optional(&mut *conn).await?,
        Owner::Album(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM albums WHERE id = $1", id).fetch_optional(&mut *conn).await?,
        Owner::Artist(id) => sqlx::query_scalar!("SELECT merged_into IS NOT NULL AS \"m!\" FROM artists WHERE id = $1", id).fetch_optional(&mut *conn).await?,
    };
    match merged {
        None => return Err(EditRejected::NotFound(owner.kind()).into()),
        Some(true) => return Err(EditRejected::Invalid(format!("that {} was merged into another: write on that one", owner.kind())).into()),
        Some(false) => {}
    }
    let (track, album, artist) = match owner {
        Owner::Track(id) => (Some(id), None, None),
        Owner::Album(id) => (None, Some(id), None),
        Owner::Artist(id) => (None, None, Some(id)),
    };
    let note = sqlx::query_scalar!(
        "INSERT INTO notes (track_id, album_id, artist_id, title, body) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        track,
        album,
        artist,
        title,
        body
    )
    .fetch_one(&mut *conn)
    .await?;
    let revision = add_revision(conn, note, &title, &body, false, reason).await?;
    Ok((note, revision))
}

/// A note's new title and text. `base`: the revision it was edited from (a stale one is
/// refused). Returns the latest revision, and whether anything changed.
pub async fn edit(conn: &mut PgConnection, note: Uuid, base: Option<Uuid>, title: &str, body: &str, reason: &str) -> anyhow::Result<(Uuid, bool)> {
    let (title, body) = checked(title, body)?;
    let (now, latest) = current(conn, note).await?;
    if let Some(base) = base
        && base != latest
    {
        return Err(EditRejected::Stale { latest: Some(latest) }.into());
    }
    if now.title == title && now.body == body {
        return Ok((latest, false));
    }
    sqlx::query!("UPDATE notes SET title = $2, body = $3 WHERE id = $1", note, title, body).execute(&mut *conn).await?;
    let revision = add_revision(conn, note, &title, &body, now.hidden, reason).await?;
    Ok((revision, true))
}

/// Takes a note off (hides it) or brings it back. Returns whether it changed.
pub async fn set_hidden(conn: &mut PgConnection, note: Uuid, hidden: bool, reason: &str) -> anyhow::Result<bool> {
    let (now, _) = current(conn, note).await?;
    if now.hidden == hidden {
        return Ok(false);
    }
    sqlx::query!("UPDATE notes SET hidden_at = CASE WHEN $2 THEN now() END WHERE id = $1", note, hidden).execute(&mut *conn).await?;
    add_revision(conn, note, &now.title, &now.body, hidden, reason).await?;
    Ok(true)
}

/// A note's revisions, newest first.
pub async fn revisions(conn: &mut PgConnection, note: Uuid) -> sqlx::Result<Vec<Revision>> {
    sqlx::query_as!(
        Revision,
        r#"
        SELECT r.id, r.title, r.body, r.hidden, r.created_at, u.username AS "created_by?", r.reason
        FROM note_revisions r LEFT JOIN users u ON u.id = r.created_by
        WHERE r.note_id = $1
        ORDER BY r.id DESC
        "#,
        note
    )
    .fetch_all(conn)
    .await
}

struct Now {
    title: String,
    body: String,
    hidden: bool,
}

/// A note as it is (locked for the change) and its latest revision.
async fn current(conn: &mut PgConnection, note: Uuid) -> anyhow::Result<(Now, Uuid)> {
    let row = sqlx::query!(
        r#"SELECT n.title, n.body, n.hidden_at IS NOT NULL AS "hidden!",
                  (SELECT r.id FROM note_revisions r WHERE r.note_id = n.id ORDER BY r.id DESC LIMIT 1) AS "latest!"
           FROM notes n WHERE n.id = $1 FOR UPDATE OF n"#,
        note
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(EditRejected::NotFound("note"))?;
    Ok((Now { title: row.title, body: row.body, hidden: row.hidden }, row.latest))
}

async fn add_revision(conn: &mut PgConnection, note: Uuid, title: &str, body: &str, hidden: bool, reason: &str) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        "INSERT INTO note_revisions (note_id, title, body, hidden, reason) VALUES ($1, $2, $3, $4, $5) RETURNING id",
        note,
        title,
        body,
        hidden,
        reason.trim()
    )
    .fetch_one(conn)
    .await
}
