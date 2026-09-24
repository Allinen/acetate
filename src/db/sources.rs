//! External sources of tracks, albums and artists, and their snapshots.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Serialize;
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
              AND COALESCE(track_id, album_id, artist_id) = $2
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
            WHERE COALESCE(track_id, album_id, artist_id) = $1
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
            WHERE COALESCE(s.track_id, s.album_id, s.artist_id) = $1
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
            return Ok(AddedSnapshot {
                source_id: row.source_id,
                snapshot_id: row.snapshot_id,
                created: false,
            });
        }
        None
    };

    let source_id = match existing {
        Some(id) => id,
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

/// All sources of a track, album or artist, each with its snapshots.
pub async fn list_for(conn: &mut PgConnection, owner_id: Uuid) -> sqlx::Result<Vec<Source>> {
    let sources = sqlx::query!(
        r#"
        SELECT id, platform, external_id FROM sources
        WHERE COALESCE(track_id, album_id, artist_id) = $1
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
