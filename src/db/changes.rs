//! The changelist: the newest revisions of tracks, albums and artists together, with what
//! each came after, so the change can be told in words (see `clients/core`'s `diff`).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

/// One revision of a track (a version's metadata, or its facts), album or artist.
#[derive(Debug, Serialize)]
pub struct Change {
    pub id: Uuid,
    /// "track", "album" or "artist".
    pub entity: &'static str,
    pub entity_id: Uuid,
    /// What it's called now.
    pub name: String,
    /// A track's: "internal" (metadata, per version) or "external" (status, albums…).
    pub kind: String,
    pub variant_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub author: Option<String>,
    pub source: String,
    pub reason: String,
    pub data: serde_json::Value,
    /// The revision before it (of the same kind and version): None for the first.
    pub before: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct Changes {
    pub changes: Vec<Change>,
    /// The names of the aliases the track changes credit, by id.
    pub alias_names: HashMap<Uuid, String>,
}

/// The newest `limit` changes older than `before` (a change's id; None: from the newest),
/// newest first. Imports (a file's first metadata) are left out unless `imports`.
pub async fn recent(conn: &mut PgConnection, before: Option<Uuid>, limit: i64, imports: bool) -> sqlx::Result<Changes> {
    // Revision ids are UUIDv7, so they order by time across the three tables: each gives
    // its newest `limit`, and the newest `limit` of those are the answer.
    let tracks = sqlx::query!(
        r#"
        SELECT r.id, r.track_id, t.title AS name, r.kind, r.variant_id, r.created_at,
               u.username AS "author?", r.source, r.reason, r.data,
               (SELECT p.data FROM track_revisions p
                WHERE p.track_id = r.track_id AND p.kind = r.kind
                  AND p.variant_id IS NOT DISTINCT FROM r.variant_id AND p.id < r.id
                ORDER BY p.id DESC LIMIT 1) AS before
        FROM track_revisions r
        JOIN tracks t ON t.id = r.track_id
        LEFT JOIN users u ON u.id = r.author_id
        WHERE ($1::uuid IS NULL OR r.id < $1) AND ($2 OR r.source <> 'import')
        ORDER BY r.id DESC
        LIMIT $3
        "#,
        before,
        imports,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let albums = sqlx::query!(
        r#"
        SELECT r.id, r.album_id, a.title AS name, r.created_at, u.username AS "author?", r.source, r.reason, r.data,
               (SELECT p.data FROM album_revisions p WHERE p.album_id = r.album_id AND p.id < r.id
                ORDER BY p.id DESC LIMIT 1) AS before
        FROM album_revisions r
        JOIN albums a ON a.id = r.album_id
        LEFT JOIN users u ON u.id = r.author_id
        WHERE ($1::uuid IS NULL OR r.id < $1) AND ($2 OR r.source <> 'import')
        ORDER BY r.id DESC
        LIMIT $3
        "#,
        before,
        imports,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let artists = sqlx::query!(
        r#"
        SELECT r.id, r.artist_id, a.name, r.created_at, u.username AS "author?", r.source, r.reason, r.data,
               (SELECT p.data FROM artist_revisions p WHERE p.artist_id = r.artist_id AND p.id < r.id
                ORDER BY p.id DESC LIMIT 1) AS before
        FROM artist_revisions r
        JOIN artists a ON a.id = r.artist_id
        LEFT JOIN users u ON u.id = r.author_id
        WHERE ($1::uuid IS NULL OR r.id < $1) AND ($2 OR r.source <> 'import')
        ORDER BY r.id DESC
        LIMIT $3
        "#,
        before,
        imports,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut changes: Vec<Change> = Vec::new();
    for r in tracks {
        changes.push(Change { id: r.id, entity: "track", entity_id: r.track_id, name: r.name, kind: r.kind, variant_id: r.variant_id, created_at: r.created_at, author: r.author, source: r.source, reason: r.reason, data: r.data, before: r.before });
    }
    for r in albums {
        changes.push(Change { id: r.id, entity: "album", entity_id: r.album_id, name: r.name, kind: String::new(), variant_id: None, created_at: r.created_at, author: r.author, source: r.source, reason: r.reason, data: r.data, before: r.before });
    }
    for r in artists {
        changes.push(Change { id: r.id, entity: "artist", entity_id: r.artist_id, name: r.name, kind: String::new(), variant_id: None, created_at: r.created_at, author: r.author, source: r.source, reason: r.reason, data: r.data, before: r.before });
    }
    changes.sort_by_key(|c| std::cmp::Reverse(c.id));
    changes.truncate(limit as usize);

    // What the data mentions that this user can't see is taken out, like in a history.
    let mut datas: Vec<&mut serde_json::Value> = Vec::new();
    for c in changes.iter_mut() {
        datas.push(&mut c.data);
        if let Some(before) = c.before.as_mut() {
            datas.push(before);
        }
    }
    super::tracks::strip_hidden(conn, datas.into_iter()).await?;
    let mut seen: Vec<&serde_json::Value> = Vec::new();
    for c in &changes {
        seen.push(&c.data);
        seen.extend(c.before.as_ref());
    }
    let alias_names = super::tracks::alias_names(conn, seen.into_iter()).await?;
    Ok(Changes { changes, alias_names })
}
