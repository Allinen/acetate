//! Covers of tracks and albums: adding (uploaded, embedded or copied), ordering, and
//! moving to the trash. The first cover (lowest position) is the main one.
//!
//! Covers aren't revisioned: no revision refers to them, and the trash is the undo.
//! Purging (deleting for good) is in `db::trash`.

use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// What a cover belongs to.
#[derive(Debug, Clone, Copy)]
pub enum CoverOwner {
    Track(Uuid),
    Album(Uuid),
}

impl CoverOwner {
    fn ids(self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            CoverOwner::Track(id) => (Some(id), None),
            CoverOwner::Album(id) => (None, Some(id)),
        }
    }

    fn id(self) -> Uuid {
        match self {
            CoverOwner::Track(id) | CoverOwner::Album(id) => id,
        }
    }
}

/// The image data of a new cover (the blob is already stored).
pub struct NewCover<'a> {
    pub blob_sha256: &'a str,
    pub mime: Option<&'a str>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// "upload", "copied", "embedded".
    pub source: &'a str,
}

/// Adds a cover after the owner's existing ones. Fails with NotFound if the owner isn't
/// visible to the user.
pub async fn add(conn: &mut PgConnection, owner: CoverOwner, cover: &NewCover<'_>) -> anyhow::Result<Uuid> {
    let (track_id, album_id) = owner.ids();
    // Lock the owner, so two covers added at once don't get the same position.
    let found = match owner {
        CoverOwner::Track(id) => {
            sqlx::query_scalar!("SELECT id FROM tracks WHERE id = $1 FOR UPDATE", id)
                .fetch_optional(&mut *conn)
                .await?
        }
        CoverOwner::Album(id) => {
            sqlx::query_scalar!("SELECT id FROM albums WHERE id = $1 FOR UPDATE", id)
                .fetch_optional(&mut *conn)
                .await?
        }
    };
    if found.is_none() {
        return Err(EditRejected::NotFound(match owner {
            CoverOwner::Track(_) => "track",
            CoverOwner::Album(_) => "album",
        })
        .into());
    }

    Ok(sqlx::query_scalar!(
        r#"
        INSERT INTO covers (track_id, album_id, blob_sha256, mime, width, height, source, position)
        VALUES ($1, $2, $3, $4, $5, $6, $7,
                (SELECT COALESCE(max(position) + 1, 0) FROM covers
                 WHERE (track_id = $8 OR album_id = $8) AND trashed_at IS NULL))
        RETURNING id
        "#,
        track_id,
        album_id,
        cover.blob_sha256,
        cover.mime,
        cover.width,
        cover.height,
        cover.source,
        owner.id(),
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Adds a copy of a cover (same image, same blob) to `owner`, e.g. a track's cover to its
/// album. The cover must be visible and not in the trash.
pub async fn copy(conn: &mut PgConnection, owner: CoverOwner, cover_id: Uuid) -> anyhow::Result<Uuid> {
    let Some(from) = sqlx::query!(
        "SELECT blob_sha256, mime, width, height FROM covers WHERE id = $1 AND trashed_at IS NULL",
        cover_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("cover").into());
    };
    let cover = NewCover {
        blob_sha256: &from.blob_sha256,
        mime: from.mime.as_deref(),
        width: from.width,
        height: from.height,
        source: "copied",
    };
    add(conn, owner, &cover).await
}

/// Puts the owner's covers in this order (the first is the main cover). `cover_ids` must
/// be exactly its covers that aren't in the trash.
pub async fn set_order(conn: &mut PgConnection, owner: CoverOwner, cover_ids: &[Uuid]) -> anyhow::Result<()> {
    let mut current = sqlx::query_scalar!(
        r#"
        SELECT id FROM covers
        WHERE (track_id = $1 OR album_id = $1) AND trashed_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
        owner.id()
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut given = cover_ids.to_vec();
    current.sort();
    given.sort();
    if current != given {
        return Err(EditRejected::Invalid(
            "list each of its covers exactly once (reload: they may have changed)".to_string(),
        )
        .into());
    }
    // `WITH ORDINALITY` numbers the array's elements: 1 for the first, 2 for the next...
    sqlx::query!(
        r#"
        UPDATE covers c SET position = (o.n - 1)::smallint
        FROM unnest($1::uuid[]) WITH ORDINALITY AS o(id, n)
        WHERE c.id = o.id
        "#,
        cover_ids
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Moves a cover to the trash. False if there's no such cover (or it's there already).
pub async fn trash(conn: &mut PgConnection, cover_id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        r#"
        UPDATE covers SET trashed_at = now(), trashed_by = current_app_user()
        WHERE id = $1 AND trashed_at IS NULL
        "#,
        cover_id
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// "Use this cover on all its tracks": adds a copy of the cover (same blob, so nothing is
/// stored twice) to every track of the album the user can see, except tracks that already
/// have that exact image. A track without covers gets it as its main cover (the first one
/// added is main). Returns (added, skipped).
pub async fn apply_to_album_tracks(
    conn: &mut PgConnection,
    album_id: Uuid,
    cover_id: Uuid,
) -> anyhow::Result<(usize, usize)> {
    let Some(blob) = sqlx::query_scalar!(
        "SELECT blob_sha256 FROM covers WHERE id = $1 AND trashed_at IS NULL",
        cover_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("cover").into());
    };
    let tracks = sqlx::query!(
        r#"
        SELECT at.track_id,
               EXISTS (SELECT 1 FROM covers c WHERE c.track_id = at.track_id
                       AND c.blob_sha256 = $2 AND c.trashed_at IS NULL) AS "has_it!"
        FROM album_tracks at
        WHERE at.album_id = $1
        ORDER BY at.track_id
        "#,
        album_id,
        blob
    )
    .fetch_all(&mut *conn)
    .await?;
    let (mut added, mut skipped) = (0, 0);
    for track in tracks {
        if track.has_it {
            skipped += 1;
        } else {
            copy(conn, CoverOwner::Track(track.track_id), cover_id).await?;
            added += 1;
        }
    }
    Ok((added, skipped))
}
