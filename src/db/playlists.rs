//! Playlists (see migration 0025): a user's ordered lists of tracks, private or public,
//! trashed rather than deleted, and every change to their entries logged so it can be
//! undone.
//!
//! What a user sees of a playlist is filtered by row-level security: someone else's
//! private playlist doesn't exist for them, and neither do entries of tracks they can't
//! see. Changes that move entries around (inserting, removing, reordering) keep such
//! hidden entries in their place: positions are worked out over all entries, as the
//! system, after checking that the user owns the playlist.

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// A playlist in a list.
#[derive(Debug, Serialize)]
pub struct PlaylistSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub public: bool,
    /// The owner's username.
    pub owner: String,
    /// The current user owns it (and can change it).
    pub mine: bool,
    /// Entries the current user can see, and their total length.
    pub entries: i64,
    pub duration_ms: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Only the owner sees their playlists in the trash.
    pub trashed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A playlist with its entries: the summary's fields, but `entries` is the list (the
/// summary's count of them isn't repeated: one key can't be both).
#[derive(Debug, Serialize)]
pub struct PlaylistDetail {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub public: bool,
    pub owner: String,
    pub mine: bool,
    pub duration_ms: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub trashed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub entries: Vec<Entry>,
}

/// One entry: a track at a place in the playlist.
#[derive(Debug, Serialize)]
pub struct Entry {
    pub id: Uuid,
    pub position: i32,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub track_id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub file_id: Uuid,
    /// The track's main cover, if it has one.
    pub cover_id: Option<Uuid>,
}

/// An entry as the log keeps it, enough to put it back exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedEntry {
    pub id: Uuid,
    pub track_id: Uuid,
    pub position: i32,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub added_by: Option<Uuid>,
}

/// One change in a playlist's log.
#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub id: Uuid,
    pub at: chrono::DateTime<chrono::Utc>,
    pub by: Option<String>,
    /// "add", "remove", "reorder", "undo" or "import".
    pub action: String,
    pub detail: serde_json::Value,
    pub undone_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// "1 entry", "3 entries".
fn entries_text(n: usize) -> String {
    if n == 1 { "1 entry".to_string() } else { format!("{n} entries") }
}

fn invalid(message: impl Into<String>) -> anyhow::Error {
    EditRejected::Invalid(message.into()).into()
}

fn not_found() -> anyhow::Error {
    EditRejected::NotFound("playlist").into()
}

/// The playlists the user sees: their own and public ones, not in the trash. With
/// `trash`, instead their own that are in the trash.
pub async fn list(conn: &mut PgConnection, trash: bool) -> sqlx::Result<Vec<PlaylistSummary>> {
    sqlx::query_as!(
        PlaylistSummary,
        r#"
        SELECT p.id, p.name, p.description, p.public, u.username AS owner,
               p.owner_id = current_app_user() AS "mine!",
               (SELECT count(*) FROM playlist_entries e WHERE e.playlist_id = p.id) AS "entries!",
               COALESCE((SELECT sum(f.duration_ms) FROM playlist_entries e
                         JOIN tracks t ON t.id = e.track_id JOIN files f ON f.id = t.file_id
                         WHERE e.playlist_id = p.id), 0)::bigint AS "duration_ms!",
               p.updated_at, p.trashed_at
        FROM playlists p JOIN users u ON u.id = p.owner_id
        WHERE (p.trashed_at IS NOT NULL) = $1
          AND (NOT $1 OR p.owner_id = current_app_user())
        ORDER BY p.owner_id = current_app_user() DESC, lower(p.name), p.id
        "#,
        trash
    )
    .fetch_all(conn)
    .await
}

pub async fn create(conn: &mut PgConnection, name: &str, description: &str, public: bool) -> anyhow::Result<Uuid> {
    if name.trim().is_empty() {
        return Err(invalid("a playlist needs a name"));
    }
    Ok(sqlx::query_scalar!(
        "INSERT INTO playlists (name, description, public) VALUES ($1, $2, $3) RETURNING id",
        name.trim(),
        description,
        public
    )
    .fetch_one(conn)
    .await?)
}

/// A playlist with the entries the user can see, in order. None if it doesn't exist
/// for them.
pub async fn get(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<PlaylistDetail>> {
    let summary = sqlx::query_as!(
        PlaylistSummary,
        r#"
        SELECT p.id, p.name, p.description, p.public, u.username AS owner,
               p.owner_id = current_app_user() AS "mine!",
               (SELECT count(*) FROM playlist_entries e WHERE e.playlist_id = p.id) AS "entries!",
               COALESCE((SELECT sum(f.duration_ms) FROM playlist_entries e
                         JOIN tracks t ON t.id = e.track_id JOIN files f ON f.id = t.file_id
                         WHERE e.playlist_id = p.id), 0)::bigint AS "duration_ms!",
               p.updated_at, p.trashed_at
        FROM playlists p JOIN users u ON u.id = p.owner_id
        WHERE p.id = $1
        "#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(summary) = summary else {
        return Ok(None);
    };
    let entries = sqlx::query_as!(
        Entry,
        r#"
        SELECT e.id, e.position, e.added_at, t.id AS track_id, t.title, t.artist_display,
               f.duration_ms, f.codec, f.id AS file_id,
               (SELECT c.id FROM covers c WHERE c.track_id = t.id AND c.trashed_at IS NULL
                ORDER BY c.position, c.id LIMIT 1) AS cover_id
        FROM playlist_entries e
        JOIN tracks t ON t.id = e.track_id
        JOIN files f ON f.id = t.file_id
        WHERE e.playlist_id = $1
        ORDER BY e.position
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let PlaylistSummary { id, name, description, public, owner, mine, entries: _, duration_ms, updated_at, trashed_at } = summary;
    Ok(Some(PlaylistDetail { id, name, description, public, owner, mine, duration_ms, updated_at, trashed_at, entries }))
}

/// Refuses unless the user owns the playlist: 404 if they can't see it, 403-like
/// "only the owner" otherwise.
async fn check_owner(conn: &mut PgConnection, id: Uuid) -> anyhow::Result<()> {
    let mine = sqlx::query_scalar!(r#"SELECT owner_id = current_app_user() AS "mine!" FROM playlists WHERE id = $1"#, id)
        .fetch_optional(&mut *conn)
        .await?;
    match mine {
        None => Err(not_found()),
        Some(false) => Err(invalid("only its owner can change a playlist")),
        Some(true) => Ok(()),
    }
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    public: Option<bool>,
) -> anyhow::Result<()> {
    check_owner(conn, id).await?;
    if name.is_some_and(|n| n.trim().is_empty()) {
        return Err(invalid("a playlist needs a name"));
    }
    sqlx::query!(
        r#"
        UPDATE playlists SET name = COALESCE($2, name), description = COALESCE($3, description),
                             public = COALESCE($4, public), updated_at = now()
        WHERE id = $1
        "#,
        id,
        name.map(str::trim),
        description,
        public
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Into the trash (true) or out of it (false).
pub async fn set_trashed(conn: &mut PgConnection, id: Uuid, trashed: bool) -> anyhow::Result<()> {
    check_owner(conn, id).await?;
    sqlx::query!(
        "UPDATE playlists SET trashed_at = CASE WHEN $2 THEN now() END, updated_at = now() WHERE id = $1",
        id,
        trashed
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// All entries (hidden ones too), in order, as the system sees them.
async fn all_entries(conn: &mut PgConnection, id: Uuid) -> anyhow::Result<Vec<LoggedEntry>> {
    let system = super::as_system(conn).await?;
    let entries = sqlx::query_as!(
        LoggedEntry,
        "SELECT id, track_id, position, added_at, added_by FROM playlist_entries WHERE playlist_id = $1 ORDER BY position",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(entries)
}

/// The entries' ids the user sees, in order.
async fn visible_ids(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!("SELECT id FROM playlist_entries WHERE playlist_id = $1 ORDER BY position", id)
        .fetch_all(conn)
        .await
}

/// Numbers all entries 0, 1, 2... in this order (as the system: hidden ones too).
async fn renumber(conn: &mut PgConnection, playlist: Uuid, order: &[Uuid]) -> anyhow::Result<()> {
    let system = super::as_system(conn).await?;
    sqlx::query!(
        r#"
        UPDATE playlist_entries e SET position = (o.n - 1)::integer
        FROM unnest($2::uuid[]) WITH ORDINALITY AS o(id, n)
        WHERE e.id = o.id AND e.playlist_id = $1
        "#,
        playlist,
        order
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("UPDATE playlists SET updated_at = now() WHERE id = $1", playlist).execute(&mut *conn).await?;
    system.end(conn).await?;
    Ok(())
}

async fn log(conn: &mut PgConnection, playlist: Uuid, action: &str, detail: serde_json::Value) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        "INSERT INTO playlist_log (playlist_id, action, detail) VALUES ($1, $2, $3) RETURNING id",
        playlist,
        action,
        detail
    )
    .fetch_one(conn)
    .await
}

/// Where a new entry goes for "before the `at`-th entry the user sees" (None or past the
/// end: at the end), as an index into all entries.
fn insert_index(all: &[LoggedEntry], visible: &[Uuid], at: Option<usize>) -> usize {
    match at.and_then(|at| visible.get(at)) {
        Some(before) => all.iter().position(|e| e.id == *before).unwrap_or(all.len()),
        None => all.len(),
    }
}

/// Adds tracks (in this order) before the `at`-th entry the user sees, or at the end.
/// Returns the new entries' ids.
pub async fn add(conn: &mut PgConnection, id: Uuid, track_ids: &[Uuid], at: Option<usize>) -> anyhow::Result<Vec<Uuid>> {
    check_owner(conn, id).await?;
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let all = all_entries(conn, id).await?;
    let visible = visible_ids(conn, id).await?;
    let index = insert_index(&all, &visible, at);
    let mut added = Vec::new();
    for (n, track) in track_ids.iter().enumerate() {
        // As the user: the policy refuses tracks they can't see.
        let entry = sqlx::query_as!(
            LoggedEntry,
            r#"
            INSERT INTO playlist_entries (playlist_id, track_id, position)
            VALUES ($1, $2, $3)
            RETURNING id, track_id, position, added_at, added_by
            "#,
            id,
            track,
            (all.len() + n) as i32 // placeholder, renumbered below
        )
        .fetch_one(&mut *conn)
        .await
        .map_err(|err| match err {
            sqlx::Error::Database(e) if e.message().contains("row-level security") || e.message().contains("foreign key") => {
                invalid(format!("track {track} doesn't exist"))
            }
            other => other.into(),
        })?;
        added.push(entry);
    }
    let mut order: Vec<Uuid> = all.iter().map(|e| e.id).collect();
    for (n, entry) in added.iter().enumerate() {
        order.insert(index + n, entry.id);
    }
    renumber(conn, id, &order).await?;
    let entries: Vec<LoggedEntry> = added
        .iter()
        .enumerate()
        .map(|(n, e)| LoggedEntry { position: (index + n) as i32, ..e.clone() })
        .collect();
    log(conn, id, "add", serde_json::json!({ "entries": entries })).await?;
    Ok(added.into_iter().map(|e| e.id).collect())
}

/// Removes these entries (ones the user sees).
pub async fn remove(conn: &mut PgConnection, id: Uuid, entry_ids: &[Uuid]) -> anyhow::Result<usize> {
    check_owner(conn, id).await?;
    let visible = visible_ids(conn, id).await?;
    if let Some(unknown) = entry_ids.iter().find(|e| !visible.contains(e)) {
        return Err(invalid(format!("entry {unknown} isn't in this playlist")));
    }
    let all = all_entries(conn, id).await?;
    let removed: Vec<LoggedEntry> = all.iter().filter(|e| entry_ids.contains(&e.id)).cloned().collect();
    sqlx::query!("DELETE FROM playlist_entries WHERE playlist_id = $1 AND id = ANY($2)", id, entry_ids)
        .execute(&mut *conn)
        .await?;
    let order: Vec<Uuid> = all.iter().filter(|e| !entry_ids.contains(&e.id)).map(|e| e.id).collect();
    renumber(conn, id, &order).await?;
    log(conn, id, "remove", serde_json::json!({ "entries": removed })).await?;
    Ok(removed.len())
}

/// Puts the entries the user sees in this order (all of them, each once). Entries they
/// can't see keep their places.
pub async fn reorder(conn: &mut PgConnection, id: Uuid, entry_ids: &[Uuid]) -> anyhow::Result<()> {
    check_owner(conn, id).await?;
    let visible = visible_ids(conn, id).await?;
    let mut given = entry_ids.to_vec();
    given.sort();
    let mut expected = visible.clone();
    expected.sort();
    if given != expected {
        return Err(invalid("send every entry of the playlist, each once, in the new order"));
    }
    let all = all_entries(conn, id).await?;
    let before: Vec<Uuid> = all.iter().map(|e| e.id).collect();
    // The visible entries' slots, filled in the new order; hidden ones stay put.
    let mut new_visible = entry_ids.iter();
    let order: Vec<Uuid> = all
        .iter()
        .map(|e| if visible.contains(&e.id) { *new_visible.next().expect("same count") } else { e.id })
        .collect();
    if order == before {
        return Ok(());
    }
    renumber(conn, id, &order).await?;
    log(conn, id, "reorder", serde_json::json!({ "before": before })).await?;
    Ok(())
}

/// The playlist's log, newest first (the owner's only).
pub async fn history(conn: &mut PgConnection, id: Uuid) -> anyhow::Result<Vec<LogEntry>> {
    check_owner(conn, id).await?;
    Ok(sqlx::query_as!(
        LogEntry,
        r#"
        SELECT l.id, l.at, u.username AS "by?", l.action, l.detail, l.undone_at
        FROM playlist_log l LEFT JOIN users u ON u.id = l.by_user
        WHERE l.playlist_id = $1
        ORDER BY l.id DESC
        "#,
        id
    )
    .fetch_all(conn)
    .await?)
}

/// Undoes one logged change, as far as what it touched is still there:
/// * an add: those entries are removed (ones removed since stay removed);
/// * a remove: the entries come back where they were (same ids);
/// * a reorder: the order before it, for entries still there; ones added since keep
///   their order after those.
///
/// It's logged too (as "undo"), and the change is marked undone. Returns what was done,
/// in words.
pub async fn undo(conn: &mut PgConnection, id: Uuid, log_id: Uuid) -> anyhow::Result<String> {
    check_owner(conn, id).await?;
    let row = sqlx::query!(
        "SELECT action, detail, undone_at FROM playlist_log WHERE id = $1 AND playlist_id = $2",
        log_id,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(EditRejected::NotFound("change"))?;
    if row.undone_at.is_some() {
        return Err(invalid("that change was undone already"));
    }
    #[derive(Deserialize)]
    struct Entries {
        entries: Vec<LoggedEntry>,
    }
    #[derive(Deserialize)]
    struct Before {
        before: Vec<Uuid>,
    }
    let all = all_entries(conn, id).await?;
    let summary = match row.action.as_str() {
        "add" => {
            let added: Entries = serde_json::from_value(row.detail)?;
            let ids: Vec<Uuid> = added.entries.iter().map(|e| e.id).filter(|e| all.iter().any(|a| a.id == *e)).collect();
            let system = super::as_system(conn).await?;
            sqlx::query!("DELETE FROM playlist_entries WHERE playlist_id = $1 AND id = ANY($2)", id, &ids)
                .execute(&mut *conn)
                .await?;
            system.end(conn).await?;
            let order: Vec<Uuid> = all.iter().map(|e| e.id).filter(|e| !ids.contains(e)).collect();
            renumber(conn, id, &order).await?;
            format!("took out the {} that change added", entries_text(ids.len()))
        }
        "remove" => {
            let removed: Entries = serde_json::from_value(row.detail)?;
            let mut order: Vec<Uuid> = all.iter().map(|e| e.id).collect();
            let system = super::as_system(conn).await?;
            // Back in their old places, lowest first (so later ones land right too).
            let mut entries = removed.entries.clone();
            entries.sort_by_key(|e| e.position);
            let mut back = 0;
            for entry in &entries {
                let exists = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1) AS "e!""#, entry.track_id)
                    .fetch_one(&mut *conn)
                    .await?;
                if !exists {
                    continue;
                }
                sqlx::query!(
                    r#"
                    INSERT INTO playlist_entries (id, playlist_id, track_id, position, added_at, added_by)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                    entry.id,
                    id,
                    entry.track_id,
                    order.len() as i32,
                    entry.added_at,
                    entry.added_by
                )
                .execute(&mut *conn)
                .await?;
                order.insert((entry.position as usize).min(order.len()), entry.id);
                back += 1;
            }
            system.end(conn).await?;
            renumber(conn, id, &order).await?;
            format!("put back the {} that change removed", entries_text(back))
        }
        "reorder" => {
            let before: Before = serde_json::from_value(row.detail)?;
            let present: Vec<Uuid> = all.iter().map(|e| e.id).collect();
            let mut order: Vec<Uuid> = before.before.into_iter().filter(|e| present.contains(e)).collect();
            order.extend(present.iter().filter(|e| !order.contains(e)).copied().collect::<Vec<_>>());
            renumber(conn, id, &order).await?;
            "put the order back as it was".to_string()
        }
        other => return Err(invalid(format!("a change of kind \"{other}\" can't be undone"))),
    };
    sqlx::query!("UPDATE playlist_log SET undone_at = now() WHERE id = $1", log_id).execute(&mut *conn).await?;
    log(conn, id, "undo", serde_json::json!({ "of": log_id, "summary": summary })).await?;
    Ok(summary)
}

/// What `import` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imported {
    /// A new playlist.
    Made,
    /// Imported before and not changed since (its log has only the import): its entries
    /// were replaced by these, e.g. after an earlier run matched fewer files.
    Refreshed,
    /// Imported before and changed since by someone: left as it is.
    Kept,
}

/// For imports (as the system): the owner's playlist with this `imported_from`, made
/// with these tracks if it doesn't exist yet, or brought up to date if nobody changed it
/// since it was imported (see `Imported`). Returns its id.
pub async fn import(
    conn: &mut PgConnection,
    owner: Uuid,
    imported_from: &str,
    name: &str,
    description: &str,
    public: bool,
    track_ids: &[Uuid],
) -> anyhow::Result<(Uuid, Imported)> {
    let existing = sqlx::query_scalar!(
        "SELECT id FROM playlists WHERE owner_id = $1 AND imported_from = $2",
        owner,
        imported_from
    )
    .fetch_optional(&mut *conn)
    .await?;
    let (id, result) = match existing {
        Some(id) => {
            let changed = sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM playlist_log WHERE playlist_id = $1 AND action <> 'import') AS "e!""#,
                id
            )
            .fetch_one(&mut *conn)
            .await?;
            if changed {
                return Ok((id, Imported::Kept));
            }
            sqlx::query!("DELETE FROM playlist_entries WHERE playlist_id = $1", id).execute(&mut *conn).await?;
            sqlx::query!("DELETE FROM playlist_log WHERE playlist_id = $1", id).execute(&mut *conn).await?;
            (id, Imported::Refreshed)
        }
        None => {
            let name = if name.trim().is_empty() { "Untitled" } else { name.trim() };
            let id = sqlx::query_scalar!(
                r#"
                INSERT INTO playlists (owner_id, name, description, public, imported_from)
                VALUES ($1, $2, $3, $4, $5) RETURNING id
                "#,
                owner,
                name,
                description,
                public,
                imported_from
            )
            .fetch_one(&mut *conn)
            .await?;
            (id, Imported::Made)
        }
    };
    for (n, track) in track_ids.iter().enumerate() {
        sqlx::query!(
            "INSERT INTO playlist_entries (playlist_id, track_id, position, added_by) VALUES ($1, $2, $3, $4)",
            id,
            track,
            n as i32,
            owner
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        "INSERT INTO playlist_log (playlist_id, action, detail, by_user) VALUES ($1, 'import', $2, NULL)",
        id,
        serde_json::json!({ "from": imported_from, "entries": track_ids.len() })
    )
    .execute(&mut *conn)
    .await?;
    Ok((id, result))
}
