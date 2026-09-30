//! Restricting tracks: which groups may see a track. The hiding itself is done by
//! row-level security (migration 0008); this only changes `tracks.is_restricted` and
//! `track_access`, and logs every change in `track_access_log`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

#[derive(Debug, Serialize)]
pub struct Access {
    pub restricted: bool,
    /// The groups that may see it (only meaningful when restricted; admins always can).
    pub groups: Vec<GroupRef>,
    /// Changes, newest first.
    pub log: Vec<AccessChange>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct AccessChange {
    pub changed_at: DateTime<Utc>,
    pub changed_by: Option<String>,
    pub restricted: bool,
    /// `[{id, name}]`, as the groups were named at the time.
    pub groups: serde_json::Value,
    pub reason: String,
}

/// Who can see a track. None if the track isn't visible to the transaction's user.
pub async fn get(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<Option<Access>> {
    let Some(restricted) =
        sqlx::query_scalar!("SELECT is_restricted FROM tracks WHERE id = $1", track_id)
            .fetch_optional(&mut *conn)
            .await?
    else {
        return Ok(None);
    };
    let groups = sqlx::query_as!(
        GroupRef,
        r#"
        SELECT g.id, g.name FROM track_access ta
        JOIN groups g ON g.id = ta.group_id
        WHERE ta.track_id = $1
        ORDER BY g.name
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let log = sqlx::query_as!(
        AccessChange,
        r#"
        SELECT l.changed_at, u.username AS "changed_by?", l.restricted, l.groups, l.reason
        FROM track_access_log l
        LEFT JOIN users u ON u.id = l.changed_by
        WHERE l.track_id = $1
        ORDER BY l.id DESC
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(Access {
        restricted,
        groups,
        log,
    }))
}

/// Sets who can see a track and logs the change. `groups` only count when `restricted`.
/// Returns false if nothing changed.
///
/// The caller must have checked that the user can see the track (e.g. `get` returned
/// Some) and may manage access. The write itself runs as the system: restricting a track
/// to groups you're not in hides it from you, and that's allowed (the UI warns first).
/// Refuses group ids that don't exist (what `set` would refuse), for checking before
/// anything else is done.
pub async fn check_groups(conn: &mut PgConnection, groups: &[Uuid]) -> anyhow::Result<()> {
    let known = sqlx::query_scalar!("SELECT id FROM groups WHERE id = ANY($1)", groups).fetch_all(&mut *conn).await?;
    match groups.iter().find(|id| !known.contains(id)) {
        Some(unknown) => Err(EditRejected::Invalid(format!("unknown group {unknown}")).into()),
        None => Ok(()),
    }
}

pub async fn set(
    conn: &mut PgConnection,
    track_id: Uuid,
    restricted: bool,
    groups: &[Uuid],
    reason: &str,
) -> anyhow::Result<bool> {
    let groups: Vec<Uuid> = if restricted { groups.to_vec() } else { Vec::new() };
    let named = sqlx::query_as!(
        GroupRef,
        "SELECT id, name FROM groups WHERE id = ANY($1) ORDER BY name",
        &groups
    )
    .fetch_all(&mut *conn)
    .await?;
    if let Some(unknown) = groups.iter().find(|id| !named.iter().any(|g| g.id == **id)) {
        return Err(EditRejected::Invalid(format!("unknown group {unknown}")).into());
    }

    let system = super::as_system(conn).await?;
    let result = set_as_system(conn, track_id, restricted, &named, reason).await;
    system.end(conn).await?;
    result
}

async fn set_as_system(
    conn: &mut PgConnection,
    track_id: Uuid,
    restricted: bool,
    groups: &[GroupRef],
    reason: &str,
) -> anyhow::Result<bool> {
    let Some(was_restricted) = sqlx::query_scalar!(
        "SELECT is_restricted FROM tracks WHERE id = $1 FOR UPDATE",
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("track").into());
    };
    let mut old_groups = sqlx::query_scalar!(
        "SELECT group_id FROM track_access WHERE track_id = $1",
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut new_groups: Vec<Uuid> = groups.iter().map(|g| g.id).collect();
    old_groups.sort();
    new_groups.sort();
    if was_restricted == restricted && old_groups == new_groups {
        return Ok(false);
    }

    sqlx::query!(
        "UPDATE tracks SET is_restricted = $2 WHERE id = $1",
        track_id,
        restricted
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM track_access WHERE track_id = $1", track_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "INSERT INTO track_access (track_id, group_id) SELECT $1, unnest($2::uuid[])",
        track_id,
        &new_groups
    )
    .execute(&mut *conn)
    .await?;
    let logged: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| json!({ "id": g.id, "name": g.name }))
        .collect();
    sqlx::query!(
        r#"
        INSERT INTO track_access_log (track_id, restricted, groups, reason)
        VALUES ($1, $2, $3, $4)
        "#,
        track_id,
        restricted,
        serde_json::Value::Array(logged),
        reason
    )
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// The tracks of an album that the transaction's user can see.
pub async fn visible_album_tracks(conn: &mut PgConnection, album_id: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!(
        "SELECT track_id FROM album_tracks WHERE album_id = $1 ORDER BY track_id",
        album_id
    )
    .fetch_all(conn)
    .await
}
