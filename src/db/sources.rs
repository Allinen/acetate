//! External sources of tracks, albums and artists, and their snapshots.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::model::sources::{InvalidSource, SourceOwner, ValidSource};

/// (track_id, album_id, artist_id): exactly one is set.
fn owner_columns(owner: SourceOwner) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match owner {
        SourceOwner::Track(id) => (Some(id), None, None),
        SourceOwner::Album(id) => (None, Some(id), None),
        SourceOwner::Artist(id) => (None, None, Some(id)),
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AddedSnapshot {
    pub source_id: Uuid,
    /// The snapshot holding this data: the new one, or an existing identical one.
    pub snapshot_id: Uuid,
    /// False if identical data was already stored, so nothing new was added.
    pub created: bool,
}

/// Adds a snapshot of platform data to an owner, creating its source if needed.
///
/// Which source it belongs to, in order: the explicit `source_id`; the source with the
/// same platform id (e.g. the same `song_id`); for platforms without an id (custom...),
/// a source that already has exactly this data. Otherwise a new source is created.
pub async fn add_snapshot(
    conn: &mut PgConnection,
    owner: SourceOwner,
    source: &ValidSource,
) -> anyhow::Result<AddedSnapshot> {
    let owner_id = owner.id();

    let existing: Option<Uuid> = if let Some(source_id) = source.source_id {
        let found = sqlx::query_scalar!(
            r#"
            SELECT id FROM sources
            WHERE id = $1
              AND (track_id = $2 OR album_id = $2 OR artist_id = $2)
              AND platform = $3
              AND external_id IS NOT DISTINCT FROM $4
            "#,
            source_id,
            owner_id,
            source.platform,
            source.external_id,
        )
        .fetch_optional(&mut *conn)
        .await?;
        if found.is_none() {
            return Err(InvalidSource(format!(
                "source {source_id} doesn't exist for this owner, platform and id"
            ))
            .into());
        }
        found
    } else if let Some(external_id) = &source.external_id {
        sqlx::query_scalar!(
            r#"
            SELECT id FROM sources
            WHERE (track_id = $1 OR album_id = $1 OR artist_id = $1)
              AND platform = $2 AND external_id = $3
            "#,
            owner_id,
            source.platform,
            external_id,
        )
        .fetch_optional(&mut *conn)
        .await?
    } else {
        // No id: reuse a source that already has exactly this data, e.g. the same
        // custom "discord" source given again for a duplicate upload.
        let same = sqlx::query!(
            r#"
            SELECT s.id AS source_id, ss.id AS snapshot_id
            FROM sources s
            JOIN source_snapshots ss ON ss.source_id = s.id
            WHERE (s.track_id = $1 OR s.album_id = $1 OR s.artist_id = $1)
              AND s.platform = $2 AND s.external_id IS NULL
              AND ss.data = $3
            ORDER BY ss.id
            LIMIT 1
            "#,
            owner_id,
            source.platform,
            source.data,
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(row) = same {
            untrash(conn, row.source_id).await?;
            return Ok(AddedSnapshot {
                source_id: row.source_id,
                snapshot_id: row.snapshot_id,
                created: false,
            });
        }
        None
    };

    let source_id = match existing {
        Some(id) => {
            untrash(conn, id).await?;
            id
        }
        None => {
            let (track_id, album_id, artist_id) = owner_columns(owner);
            sqlx::query_scalar!(
                r#"
                INSERT INTO sources (track_id, album_id, artist_id, platform, external_id)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id
                "#,
                track_id,
                album_id,
                artist_id,
                source.platform,
                source.external_id,
            )
            .fetch_one(&mut *conn)
            .await?
        }
    };

    let captured_at = source.captured_at.unwrap_or_else(Utc::now);

    // Reuse the snapshot just before this point in time if it has exactly the same data.
    // (Data going A -> B -> A is still stored as three snapshots.)
    let previous = sqlx::query!(
        r#"
        SELECT id, data FROM source_snapshots
        WHERE source_id = $1 AND captured_at <= $2
        ORDER BY captured_at DESC, id DESC
        LIMIT 1
        "#,
        source_id,
        captured_at,
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(previous) = previous.filter(|p| p.data == source.data) {
        return Ok(AddedSnapshot {
            source_id,
            snapshot_id: previous.id,
            created: false,
        });
    }

    let snapshot_id = sqlx::query_scalar!(
        r#"
        INSERT INTO source_snapshots (source_id, captured_at, data)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
        source_id,
        captured_at,
        source.data,
    )
    .fetch_one(&mut *conn)
    .await?;

    Ok(AddedSnapshot {
        source_id,
        snapshot_id,
        created: true,
    })
}

/// Moves every source of `from` to `into` (the same kind of owner: two artists, two
/// tracks), when merging them. A source with the same platform id as one of `into`'s
/// (e.g. both have NetEase song 123) joins that one: its snapshots move over, so files'
/// origins keep pointing at the same snapshots, and the emptied source is deleted.
pub async fn move_all(conn: &mut PgConnection, from: SourceOwner, into: SourceOwner) -> sqlx::Result<MovedSources> {
    let sources = sqlx::query!(
        r#"
        SELECT id, platform, external_id, created_at, created_by, trashed_at, trashed_by
        FROM sources WHERE (track_id = $1 OR album_id = $1 OR artist_id = $1)
        "#,
        from.id()
    )
    .fetch_all(&mut *conn)
    .await?;
    let (track_id, album_id, artist_id) = owner_columns(into);
    let mut result = MovedSources::default();
    for src in sources {
        let same = match &src.external_id {
            Some(external_id) => {
                sqlx::query_scalar!(
                    r#"
                    SELECT id FROM sources
                    WHERE (track_id = $1 OR album_id = $1 OR artist_id = $1) AND platform = $2 AND external_id = $3
                    "#,
                    into.id(),
                    src.platform,
                    external_id
                )
                .fetch_optional(&mut *conn)
                .await?
            }
            None => None,
        };
        match same {
            Some(target) => {
                let snapshot_ids = sqlx::query_scalar!(
                    "UPDATE source_snapshots SET source_id = $2 WHERE source_id = $1 RETURNING id",
                    src.id,
                    target
                )
                .fetch_all(&mut *conn)
                .await?;
                sqlx::query!("DELETE FROM sources WHERE id = $1", src.id)
                    .execute(&mut *conn)
                    .await?;
                result.joined.push(JoinedSource {
                    id: src.id,
                    platform: src.platform,
                    external_id: src.external_id,
                    created_at: src.created_at,
                    created_by: src.created_by,
                    trashed_at: src.trashed_at,
                    trashed_by: src.trashed_by,
                    snapshot_ids,
                    into_source: target,
                });
            }
            None => {
                sqlx::query!(
                    "UPDATE sources SET track_id = $2, album_id = $3, artist_id = $4 WHERE id = $1",
                    src.id,
                    track_id,
                    album_id,
                    artist_id
                )
                .execute(&mut *conn)
                .await?;
                result.moved.push(src.id);
            }
        }
    }
    Ok(result)
}

/// What `move_all` did, so it can be put back (undoing a track merge).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct MovedSources {
    /// Sources that just changed owner.
    pub moved: Vec<Uuid>,
    /// Sources that joined one of the new owner's (and were deleted), as they were.
    pub joined: Vec<JoinedSource>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinedSource {
    pub id: Uuid,
    pub platform: String,
    pub external_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub trashed_at: Option<DateTime<Utc>>,
    pub trashed_by: Option<Uuid>,
    /// Its snapshots, now in `into_source`.
    pub snapshot_ids: Vec<Uuid>,
    pub into_source: Uuid,
}

/// Puts back what `move_all` moved from `from` to `into`: moved sources return, joined
/// ones are recreated (same id) and get their snapshots back.
pub async fn unmove_all(
    conn: &mut PgConnection,
    moved: &MovedSources,
    from: SourceOwner,
    into: SourceOwner,
) -> sqlx::Result<()> {
    let (track_id, album_id, artist_id) = owner_columns(from);
    sqlx::query!(
        r#"
        UPDATE sources SET track_id = $2, album_id = $3, artist_id = $4
        WHERE id = ANY($1) AND (track_id = $5 OR album_id = $5 OR artist_id = $5)
        "#,
        &moved.moved,
        track_id,
        album_id,
        artist_id,
        into.id()
    )
    .execute(&mut *conn)
    .await?;
    for source in &moved.joined {
        sqlx::query!(
            r#"
            INSERT INTO sources (id, track_id, album_id, artist_id, platform, external_id,
                                 created_at, created_by, trashed_at, trashed_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            source.id,
            track_id,
            album_id,
            artist_id,
            source.platform,
            source.external_id,
            source.created_at,
            source.created_by,
            source.trashed_at,
            source.trashed_by
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "UPDATE source_snapshots SET source_id = $1 WHERE id = ANY($2)",
            source.id,
            &source.snapshot_ids
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Takes a source back out of the trash, if it's there: adding it again means it's wanted.
async fn untrash(conn: &mut PgConnection, source_id: Uuid) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE sources SET trashed_at = NULL, trashed_by = NULL WHERE id = $1 AND trashed_at IS NOT NULL",
        source_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Moves a source (with its snapshots) to the trash. False if there's no such source (or
/// it's in the trash already).
///
/// Refused while one of its snapshots is a file's origin: every file keeps a known origin.
/// (A file's origin is always a source of the file's own track, so the filename in the
/// message is something the user can see anyway.)
pub async fn trash(conn: &mut PgConnection, source_id: Uuid) -> anyhow::Result<bool> {
    let system = super::as_system(conn).await?;
    let origin_of = sqlx::query_scalar!(
        r#"
        SELECT COALESCE(f.original_filename, f.uploaded_filename) AS "filename!" FROM files f
        JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id
        WHERE ss.source_id = $1
        LIMIT 1
        "#,
        source_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    system.end(conn).await?;
    if let Some(filename) = origin_of {
        return Err(super::revisions::EditRejected::Invalid(format!(
            "it's where the file \"{filename}\" came from, and a file's origin can't be removed"
        ))
        .into());
    }
    let result = sqlx::query!(
        r#"
        UPDATE sources SET trashed_at = now(), trashed_by = current_app_user()
        WHERE id = $1 AND trashed_at IS NULL
        "#,
        source_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug, Serialize)]
pub struct Source {
    pub id: Uuid,
    pub platform: String,
    pub external_id: Option<String>,
    /// Newest first.
    pub snapshots: Vec<Snapshot>,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub data: serde_json::Value,
}

/// All sources of a track, album or artist (not those in the trash), each with its snapshots.
pub async fn list_for(conn: &mut PgConnection, owner_id: Uuid) -> sqlx::Result<Vec<Source>> {
    let sources = sqlx::query!(
        r#"
        SELECT id, platform, external_id FROM sources
        WHERE (track_id = $1 OR album_id = $1 OR artist_id = $1) AND trashed_at IS NULL
        ORDER BY platform, id
        "#,
        owner_id
    )
    .fetch_all(&mut *conn)
    .await?;

    let source_ids: Vec<Uuid> = sources.iter().map(|s| s.id).collect();
    let snapshots = sqlx::query!(
        r#"
        SELECT id, source_id, captured_at, data FROM source_snapshots
        WHERE source_id = ANY($1)
        ORDER BY captured_at DESC, id DESC
        "#,
        &source_ids
    )
    .fetch_all(&mut *conn)
    .await?;

    // Group snapshots by source.
    let mut by_source: HashMap<Uuid, Vec<Snapshot>> = HashMap::new();
    for snap in snapshots {
        by_source.entry(snap.source_id).or_default().push(Snapshot {
            id: snap.id,
            captured_at: snap.captured_at,
            data: snap.data,
        });
    }

    Ok(sources
        .into_iter()
        .map(|s| Source {
            snapshots: by_source.remove(&s.id).unwrap_or_default(),
            id: s.id,
            platform: s.platform,
            external_id: s.external_id,
        })
        .collect())
}

/// A file's origin: the snapshot it points at, with its source's platform and id.
#[derive(Debug, Serialize)]
pub struct Origin {
    pub snapshot_id: Uuid,
    pub source_id: Uuid,
    pub platform: String,
    pub external_id: Option<String>,
    pub captured_at: DateTime<Utc>,
    pub data: serde_json::Value,
}

/// Origins of the given files, keyed by file id. Files without an origin are left out.
pub async fn origins_of_files(
    conn: &mut PgConnection,
    file_ids: &[Uuid],
) -> sqlx::Result<HashMap<Uuid, Origin>> {
    let rows = sqlx::query!(
        r#"
        SELECT f.id AS file_id, ss.id AS snapshot_id, s.id AS source_id, s.platform,
               s.external_id, ss.captured_at, ss.data
        FROM files f
        JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id
        JOIN sources s ON s.id = ss.source_id
        WHERE f.id = ANY($1)
        "#,
        file_ids
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.file_id,
                Origin {
                    snapshot_id: r.snapshot_id,
                    source_id: r.source_id,
                    platform: r.platform,
                    external_id: r.external_id,
                    captured_at: r.captured_at,
                    data: r.data,
                },
            )
        })
        .collect())
}

/// Whether the track / album / artist exists.
pub async fn owner_exists(conn: &mut PgConnection, owner: SourceOwner) -> sqlx::Result<bool> {
    let exists = match owner {
        SourceOwner::Track(id) => {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1) AS "e!""#, id)
                .fetch_one(&mut *conn)
                .await?
        }
        SourceOwner::Album(id) => {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM albums WHERE id = $1) AS "e!""#, id)
                .fetch_one(&mut *conn)
                .await?
        }
        SourceOwner::Artist(id) => {
            sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM artists WHERE id = $1) AS "e!""#, id)
                .fetch_one(&mut *conn)
                .await?
        }
    };
    Ok(exists)
}
