//! The trash: covers, sources and old audio files that are no longer wanted.
//!
//! Nothing is deleted directly. Things go to the trash first (`trashed_at`), where they're
//! hidden but can be taken back out. After `TRASH_GRACE_DAYS` an admin can purge them,
//! the only way anything is ever deleted:
//! * a cover: its row is deleted;
//! * a source: deleted with its snapshots;
//! * a file: its bytes are deleted, its row stays as a tombstone (tags, origin, and its
//!   place in the track's history), marked `purged_at`.
//!
//! A blob (image or audio) is deleted only once nothing uses it anymore: covers are
//! copied by pointing at the same blob, so purging one copy keeps it for the others.
//! Every purge is logged in `purge_log`.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;
use crate::storage::BlobStore;

/// How long things stay in the trash before they can be purged.
pub const TRASH_GRACE_DAYS: i32 = 7;

/// A purge of one item needs at least this much of its sha256 (or id) typed in.
const CONFIRM_PREFIX_LEN: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Cover,
    Source,
    File,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Cover => "cover",
            Kind::Source => "source",
            Kind::File => "file",
        }
    }
}

/// One thing in the trash.
#[derive(Debug, Serialize)]
pub struct TrashItem {
    pub kind: String,
    pub id: Uuid,
    /// What it is: a file's name, a source's platform and id, "cover".
    pub what: String,
    /// What it belongs to (a track, album or artist) and its title/name.
    pub owner_id: Option<Uuid>,
    /// "track", "album" or "artist".
    pub owner_kind: Option<String>,
    pub owner: Option<String>,
    /// Files and covers.
    pub sha256: Option<String>,
    /// Files (covers: filled in by the API from the blob store).
    pub size_bytes: Option<i64>,
    pub trashed_at: DateTime<Utc>,
    pub trashed_by: Option<String>,
    pub purgeable_at: DateTime<Utc>,
}

/// Everything in the trash that the user can see, oldest first (those are purgeable first).
pub async fn list(conn: &mut PgConnection) -> sqlx::Result<Vec<TrashItem>> {
    sqlx::query_as!(
        TrashItem,
        r#"
        SELECT items.kind AS "kind!", items.id AS "id!", items.what AS "what!", items.owner_id,
               items.owner_kind, items.owner, items.sha256, items.size_bytes, items.trashed_at AS "trashed_at!",
               u.username AS "trashed_by?",
               items.trashed_at + make_interval(days => $1) AS "purgeable_at!"
        FROM (
            SELECT 'cover' AS kind, c.id, 'cover' AS what,
                   COALESCE(c.track_id, c.album_id) AS owner_id,
                   CASE WHEN c.track_id IS NOT NULL THEN 'track' ELSE 'album' END AS owner_kind,
                   COALESCE(t.title, al.title) AS owner,
                   c.blob_sha256 AS sha256, NULL::bigint AS size_bytes, c.trashed_at, c.trashed_by
            FROM covers c
            LEFT JOIN tracks t ON t.id = c.track_id
            LEFT JOIN albums al ON al.id = c.album_id
            WHERE c.trashed_at IS NOT NULL
            UNION ALL
            SELECT 'source', s.id, s.platform || COALESCE(' ' || s.external_id, ''),
                   COALESCE(s.track_id, s.album_id, s.artist_id),
                   CASE WHEN s.track_id IS NOT NULL THEN 'track'
                        WHEN s.album_id IS NOT NULL THEN 'album' ELSE 'artist' END,
                   COALESCE(t.title, al.title, ar.name),
                   NULL, NULL, s.trashed_at, s.trashed_by
            FROM sources s
            LEFT JOIN tracks t ON t.id = s.track_id
            LEFT JOIN albums al ON al.id = s.album_id
            LEFT JOIN artists ar ON ar.id = s.artist_id
            WHERE s.trashed_at IS NOT NULL
            UNION ALL
            SELECT 'file', f.id, f.original_filename, h.track_id, 'track', t.title,
                   f.sha256, f.size_bytes, f.trashed_at, f.trashed_by
            FROM files f
            LEFT JOIN track_file_history h ON h.old_file_id = f.id
            LEFT JOIN tracks t ON t.id = h.track_id
            WHERE f.trashed_at IS NOT NULL AND f.purged_at IS NULL
        ) items
        LEFT JOIN users u ON u.id = items.trashed_by
        ORDER BY items.trashed_at, items.id
        "#,
        TRASH_GRACE_DAYS
    )
    .fetch_all(conn)
    .await
}

/// Takes something back out of the trash. False if it isn't there (or not visible).
pub async fn restore(conn: &mut PgConnection, kind: Kind, id: Uuid) -> sqlx::Result<bool> {
    let result = match kind {
        Kind::Cover => {
            sqlx::query!(
                "UPDATE covers SET trashed_at = NULL, trashed_by = NULL WHERE id = $1 AND trashed_at IS NOT NULL",
                id
            )
            .execute(&mut *conn)
            .await?
        }
        Kind::Source => {
            sqlx::query!(
                "UPDATE sources SET trashed_at = NULL, trashed_by = NULL WHERE id = $1 AND trashed_at IS NOT NULL",
                id
            )
            .execute(&mut *conn)
            .await?
        }
        Kind::File => {
            sqlx::query!(
                r#"
                UPDATE files SET trashed_at = NULL, trashed_by = NULL
                WHERE id = $1 AND trashed_at IS NOT NULL AND purged_at IS NULL
                "#,
                id
            )
            .execute(&mut *conn)
            .await?
        }
    };
    Ok(result.rows_affected() > 0)
}

// ---- Old audio files ----

/// A previous file of a track: replaced, so nothing plays it anymore.
#[derive(Debug, Serialize)]
pub struct ReplacedFile {
    pub id: Uuid,
    pub track_id: Uuid,
    pub track_title: String,
    pub original_filename: String,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub size_bytes: i64,
    pub replaced_at: DateTime<Utc>,
}

/// Previous files that aren't in the trash (or purged) yet, newest first, a page at a
/// time (`before` = the last id seen). Candidates for cleaning up after upgrades.
pub async fn replaced_files(
    conn: &mut PgConnection,
    before: Option<Uuid>,
    limit: i64,
) -> sqlx::Result<Vec<ReplacedFile>> {
    sqlx::query_as!(
        ReplacedFile,
        r#"
        SELECT f.id, h.track_id, t.title AS track_title, f.original_filename, f.codec,
               f.bitrate, f.size_bytes, h.replaced_at
        FROM track_file_history h
        JOIN files f ON f.id = h.old_file_id
        JOIN tracks t ON t.id = h.track_id
        WHERE f.trashed_at IS NULL
          AND NOT EXISTS (SELECT 1 FROM tracks c WHERE c.file_id = f.id)
          AND f.id < COALESCE($1, 'ffffffff-ffff-ffff-ffff-ffffffffffff'::uuid)
        ORDER BY f.id DESC
        LIMIT $2
        "#,
        before,
        limit
    )
    .fetch_all(conn)
    .await
}

/// Moves audio files to the trash. Refused (for all of them) if any is some track's
/// current file, or doesn't exist, or is in the trash already. Returns how many.
pub async fn trash_files(conn: &mut PgConnection, ids: &[Uuid]) -> anyhow::Result<usize> {
    // Current files of any track, hidden ones included.
    let system = super::as_system(conn).await?;
    let current = sqlx::query_scalar!("SELECT file_id FROM tracks WHERE file_id = ANY($1)", ids)
        .fetch_all(&mut *conn)
        .await?;
    system.end(conn).await?;
    if !current.is_empty() {
        let list: Vec<String> = current.iter().map(Uuid::to_string).collect();
        return Err(EditRejected::Invalid(format!(
            "a track currently plays these, so they can't be trashed: {}",
            list.join(", ")
        ))
        .into());
    }
    let result = sqlx::query!(
        r#"
        UPDATE files SET trashed_at = now(), trashed_by = current_app_user()
        WHERE id = ANY($1) AND trashed_at IS NULL AND purged_at IS NULL
        "#,
        ids
    )
    .execute(&mut *conn)
    .await?;
    let trashed = result.rows_affected() as usize;
    if trashed != ids.len() {
        return Err(EditRejected::Invalid(format!(
            "only {trashed} of {} files could be trashed (the others don't exist or are in the \
             trash already); nothing was changed",
            ids.len()
        ))
        .into());
    }
    Ok(trashed)
}

// ---- Purging ----

/// The result of a purge. `delete_blobs`: blobs nothing uses anymore, to delete once the
/// transaction has committed (not before: if it rolled back, they'd still be needed).
#[derive(Debug)]
pub struct Purged {
    pub items: usize,
    pub delete_blobs: Vec<String>,
    pub bytes_freed: i64,
}

/// What we know about an item before purging it.
struct Target {
    kind: Kind,
    id: Uuid,
    trashed_at: Option<DateTime<Utc>>,
    /// Typed to confirm a purge of this one item: its sha256 (covers, files) or id.
    confirm_key: String,
    blob: Option<String>,
    description: serde_json::Value,
    size_bytes: i64,
}

/// Purges items from the trash, for good. All or nothing: if any item isn't in the
/// trash, isn't past the grace period, or `confirm` doesn't match, nothing happens.
///
/// `confirm`: for one item, at least the first 6 characters of its sha256 (for a source,
/// of its id); for several, how many there are (e.g. "12"). Enough that it can't happen
/// by accident, without making bulk cleanups tedious.
///
/// `store` is only used to measure blobs; deleting them is up to the caller, after commit.
pub async fn purge(
    conn: &mut PgConnection,
    store: &BlobStore,
    items: &[(Kind, Uuid)],
    confirm: &str,
) -> anyhow::Result<Purged> {
    if items.is_empty() {
        return Err(EditRejected::Invalid("nothing to purge".to_string()).into());
    }
    let unique: BTreeSet<(Kind, Uuid)> = items.iter().copied().collect();
    if unique.len() != items.len() {
        return Err(EditRejected::Invalid("an item is listed twice".to_string()).into());
    }

    let mut targets = Vec::new();
    let mut problems = Vec::new();
    for (kind, id) in items {
        match load_target(conn, *kind, *id).await? {
            None => problems.push(format!("{} {id}: not found", kind.as_str())),
            Some(t) if t.trashed_at.is_none() => {
                problems.push(format!("{} {id}: not in the trash", kind.as_str()))
            }
            Some(t) => {
                let purgeable_at = t.trashed_at.unwrap() + chrono::Duration::days(TRASH_GRACE_DAYS.into());
                if purgeable_at > Utc::now() {
                    problems.push(format!(
                        "{} {id}: can be purged from {}",
                        kind.as_str(),
                        purgeable_at.format("%Y-%m-%d %H:%M UTC")
                    ));
                } else {
                    targets.push(t);
                }
            }
        }
    }
    if !problems.is_empty() {
        return Err(EditRejected::Invalid(format!("nothing was purged: {}", problems.join("; "))).into());
    }

    let confirm = confirm.trim().to_lowercase();
    let confirmed = if targets.len() == 1 {
        confirm.len() >= CONFIRM_PREFIX_LEN && targets[0].confirm_key.starts_with(&confirm)
    } else {
        confirm == targets.len().to_string()
    };
    if !confirmed {
        let expected = if targets.len() == 1 {
            format!("the first {CONFIRM_PREFIX_LEN}+ characters of {}", targets[0].confirm_key)
        } else {
            format!("the number of items, {}", targets.len())
        };
        return Err(EditRejected::Invalid(format!("to confirm, type {expected}")).into());
    }

    for t in &targets {
        match t.kind {
            Kind::Cover => {
                sqlx::query!("DELETE FROM covers WHERE id = $1", t.id)
                    .execute(&mut *conn)
                    .await?;
            }
            Kind::Source => {
                // Its snapshots go with it (ON DELETE CASCADE).
                sqlx::query!("DELETE FROM sources WHERE id = $1", t.id)
                    .execute(&mut *conn)
                    .await?;
            }
            Kind::File => {
                sqlx::query!(
                    "UPDATE files SET purged_at = now(), purged_by = current_app_user() WHERE id = $1",
                    t.id
                )
                .execute(&mut *conn)
                .await?;
            }
        }
    }

    // Which blobs nothing uses anymore. As the system: a hidden track's cover may share
    // the image. (A trashed cover still counts: it can be taken back out.)
    let mut freed_by: std::collections::HashMap<Uuid, i64> = std::collections::HashMap::new();
    let mut delete_blobs = Vec::new();
    let blobs: BTreeSet<&String> = targets.iter().filter_map(|t| t.blob.as_ref()).collect();
    for sha in blobs {
        let system = super::as_system(conn).await?;
        let used = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (SELECT 1 FROM covers WHERE blob_sha256 = $1)
                OR EXISTS (SELECT 1 FROM files WHERE sha256 = $1 AND purged_at IS NULL)
               AS "used!"
            "#,
            sha
        )
        .fetch_one(&mut *conn)
        .await?;
        system.end(conn).await?;
        if !used {
            let size = store.size(sha).await.unwrap_or(0) as i64;
            // Credited to the first purged item that used it.
            if let Some(t) = targets.iter().find(|t| t.blob.as_ref() == Some(sha)) {
                *freed_by.entry(t.id).or_default() += size;
            }
            delete_blobs.push(sha.clone());
        }
    }

    let mut bytes_freed = 0;
    for t in &targets {
        let freed = freed_by.get(&t.id).copied().unwrap_or(0);
        bytes_freed += freed;
        let mut description = t.description.clone();
        description["size_bytes"] = json!(t.size_bytes);
        sqlx::query!(
            r#"
            INSERT INTO purge_log (kind, item_id, description, bytes_freed)
            VALUES ($1, $2, $3, $4)
            "#,
            t.kind.as_str(),
            t.id,
            description,
            freed
        )
        .execute(&mut *conn)
        .await?;
    }

    Ok(Purged {
        items: targets.len(),
        delete_blobs,
        bytes_freed,
    })
}

async fn load_target(conn: &mut PgConnection, kind: Kind, id: Uuid) -> sqlx::Result<Option<Target>> {
    Ok(match kind {
        Kind::Cover => sqlx::query!(
            r#"
            SELECT c.trashed_at, c.blob_sha256 AS "blob_sha256!", c.mime, c.source, c.track_id, c.album_id,
                   COALESCE(t.title, al.title) AS owner
            FROM covers c
            LEFT JOIN tracks t ON t.id = c.track_id
            LEFT JOIN albums al ON al.id = c.album_id
            WHERE c.id = $1
            "#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| Target {
            kind,
            id,
            trashed_at: r.trashed_at,
            confirm_key: r.blob_sha256.clone(),
            description: json!({
                "sha256": r.blob_sha256, "mime": r.mime, "source": r.source,
                "track_id": r.track_id, "album_id": r.album_id, "owner": r.owner,
            }),
            blob: Some(r.blob_sha256),
            size_bytes: 0,
        }),
        Kind::Source => sqlx::query!(
            r#"
            SELECT s.trashed_at, s.platform, s.external_id, s.track_id, s.album_id, s.artist_id,
                   (SELECT jsonb_agg(ss.data ORDER BY ss.captured_at) FROM source_snapshots ss
                    WHERE ss.source_id = s.id) AS snapshots
            FROM sources s WHERE s.id = $1
            "#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| Target {
            kind,
            id,
            trashed_at: r.trashed_at,
            confirm_key: id.to_string(),
            description: json!({
                "platform": r.platform, "external_id": r.external_id,
                "track_id": r.track_id, "album_id": r.album_id, "artist_id": r.artist_id,
                "snapshots": r.snapshots,
            }),
            blob: None,
            size_bytes: 0,
        }),
        Kind::File => sqlx::query!(
            r#"
            SELECT f.trashed_at, f.purged_at, f.sha256, f.original_filename, f.size_bytes,
                   f.codec, f.bitrate, h.track_id AS "track_id?", t.title AS "track_title?"
            FROM files f
            LEFT JOIN track_file_history h ON h.old_file_id = f.id
            LEFT JOIN tracks t ON t.id = h.track_id
            WHERE f.id = $1
            "#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        // Already purged: as good as not there.
        .filter(|r| r.purged_at.is_none())
        .map(|r| Target {
            kind,
            id,
            trashed_at: r.trashed_at,
            confirm_key: r.sha256.clone(),
            description: json!({
                "sha256": r.sha256, "original_filename": r.original_filename,
                "codec": r.codec, "bitrate": r.bitrate,
                "track_id": r.track_id, "track_title": r.track_title,
            }),
            blob: Some(r.sha256),
            size_bytes: r.size_bytes,
        }),
    })
}
