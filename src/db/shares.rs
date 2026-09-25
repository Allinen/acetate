//! Share links: creating and revoking them, and everything a share link can show.
//!
//! The queries that serve a share only ever return rows that belong to the share's target
//! (the shared track, or the tracks of the shared album). Run them in a transaction from
//! `db::begin_share`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// A share that currently works: exists, not revoked, not expired, and its creator may
/// still share.
pub struct ActiveShare {
    pub id: Uuid,
    pub track_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
}

/// Looks up a working share by token hash. Takes the pool: this is how we find out which
/// share a request is for, before there's a transaction for it.
pub async fn find_active(pool: &PgPool, token_sha256: &str) -> sqlx::Result<Option<ActiveShare>> {
    sqlx::query_as!(
        ActiveShare,
        r#"
        SELECT s.id, s.track_id, s.album_id
        FROM shares s
        WHERE s.token_sha256 = $1
          AND s.revoked_at IS NULL
          AND (s.expires_at IS NULL OR s.expires_at > now())
          -- Losing the permission to share ends your shares.
          AND EXISTS (
              SELECT 1 FROM user_groups ug
              JOIN group_permissions gp ON gp.group_id = ug.group_id
              WHERE ug.user_id = s.created_by AND gp.permission IN ('share', 'admin')
          )
        "#,
        token_sha256
    )
    .fetch_optional(pool)
    .await
}

/// Creates a share of a track or an album (exactly one of the two). The creator is the
/// transaction's user (`created_by` defaults to it).
pub async fn create(
    conn: &mut PgConnection,
    token_sha256: &str,
    track_id: Option<Uuid>,
    album_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        r#"
        INSERT INTO shares (token_sha256, track_id, album_id, expires_at)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
        token_sha256,
        track_id,
        album_id,
        expires_at
    )
    .fetch_one(conn)
    .await
}

#[derive(Debug, Serialize)]
pub struct ShareInfo {
    pub id: Uuid,
    pub track_id: Option<Uuid>,
    pub album_id: Option<Uuid>,
    /// The track's or album's title.
    pub title: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Shares made by `created_by`, or everyone's if None. Newest first.
pub async fn list(conn: &mut PgConnection, created_by: Option<Uuid>) -> sqlx::Result<Vec<ShareInfo>> {
    sqlx::query_as!(
        ShareInfo,
        r#"
        SELECT s.id, s.track_id, s.album_id,
               COALESCE(t.title, al.title) AS "title!",
               u.username AS created_by,
               s.created_at, s.expires_at, s.revoked_at
        FROM shares s
        JOIN users u ON u.id = s.created_by
        LEFT JOIN tracks t ON t.id = s.track_id
        LEFT JOIN albums al ON al.id = s.album_id
        WHERE s.created_by = $1 OR $1 IS NULL
        ORDER BY s.id DESC
        "#,
        created_by
    )
    .fetch_all(conn)
    .await
}

/// Revokes a share. With `created_by`, only if it's theirs. Returns false if there was no
/// such (unrevoked) share.
pub async fn revoke(conn: &mut PgConnection, id: Uuid, created_by: Option<Uuid>) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE shares SET revoked_at = now()
        WHERE id = $1 AND revoked_at IS NULL AND (created_by = $2 OR $2 IS NULL)
        "#,
        id,
        created_by
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ---- What a share shows ----

#[derive(Debug, Serialize)]
pub struct SharedTrack {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
    pub duration_ms: Option<i64>,
    pub cover_id: Option<Uuid>,
}

/// The tracks a share opens up: the shared track, or the shared album's tracks in order.
pub async fn tracks(conn: &mut PgConnection, share: &ActiveShare) -> sqlx::Result<Vec<SharedTrack>> {
    sqlx::query_as!(
        SharedTrack,
        r#"
        SELECT t.id, t.title, t.artist_display, f.duration_ms,
               (SELECT c.id FROM covers c WHERE c.track_id = t.id
                ORDER BY c.position, c.id LIMIT 1) AS cover_id
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        LEFT JOIN album_tracks at ON at.track_id = t.id AND at.album_id = $2
        -- The ids first, then look them up: stays an index lookup at any library size
        -- (`t.id = $1 OR <joined album>` could make Postgres scan all tracks).
        WHERE t.id IN (
            SELECT $1::uuid
            UNION ALL
            SELECT track_id FROM album_tracks WHERE album_id = $2
        )
        ORDER BY at.disc_number NULLS FIRST, at.track_number NULLS LAST, t.title
        "#,
        share.track_id,
        share.album_id
    )
    .fetch_all(conn)
    .await
}

pub struct SharedAlbum {
    pub title: String,
    /// The album's artists, as credited, joined with ", ".
    pub artist: String,
}

pub async fn album(conn: &mut PgConnection, album_id: Uuid) -> sqlx::Result<Option<SharedAlbum>> {
    sqlx::query_as!(
        SharedAlbum,
        r#"
        SELECT al.title,
               COALESCE((
                   SELECT string_agg(COALESCE(aa.alias, a.name), ', ' ORDER BY ac.position)
                   FROM album_credits ac
                   JOIN artists a ON a.id = ac.artist_id
                   LEFT JOIN artist_aliases aa ON aa.id = ac.alias_id
                   WHERE ac.album_id = al.id
               ), '') AS "artist!"
        FROM albums al
        WHERE al.id = $1
        "#,
        album_id
    )
    .fetch_optional(conn)
    .await
}

pub struct SharedBlob {
    pub sha256: String,
    pub container: Option<String>,
    pub codec: Option<String>,
}

/// The *current* file of a track, if the track is part of the share. (Current, so an
/// upgraded file reaches existing links.)
pub async fn file(
    conn: &mut PgConnection,
    share: &ActiveShare,
    track_id: Uuid,
) -> sqlx::Result<Option<SharedBlob>> {
    sqlx::query_as!(
        SharedBlob,
        r#"
        SELECT f.sha256, f.container, f.codec
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id = $1
          AND (t.id = $2 OR EXISTS (
              SELECT 1 FROM album_tracks at WHERE at.track_id = t.id AND at.album_id = $3
          ))
        "#,
        track_id,
        share.track_id,
        share.album_id
    )
    .fetch_optional(conn)
    .await
}

pub struct SharedCover {
    pub sha256: String,
    pub mime: Option<String>,
}

/// A cover, if it belongs to a track of the share (or to the shared album).
pub async fn cover(
    conn: &mut PgConnection,
    share: &ActiveShare,
    cover_id: Uuid,
) -> sqlx::Result<Option<SharedCover>> {
    sqlx::query_as!(
        SharedCover,
        r#"
        SELECT c.blob_sha256 AS sha256, c.mime
        FROM covers c
        WHERE c.id = $1
          AND (c.track_id = $2
               OR c.album_id = $3
               OR EXISTS (
                   SELECT 1 FROM album_tracks at WHERE at.track_id = c.track_id AND at.album_id = $3
               ))
        "#,
        cover_id,
        share.track_id,
        share.album_id
    )
    .fetch_optional(conn)
    .await
}
