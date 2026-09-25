//! Groups: bundles of permissions, and what restricted tracks are opened to.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

/// The group every admin is in. It can't be deleted or have its permissions changed, so
/// there's always a way back in.
pub const ADMIN_GROUP: &str = "admin";

#[derive(Debug, Serialize)]
pub struct GroupInfo {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub members: Vec<String>,
}

pub async fn list(conn: &mut PgConnection) -> sqlx::Result<Vec<GroupInfo>> {
    sqlx::query_as!(
        GroupInfo,
        r#"
        SELECT g.id, g.name, g.description,
               ARRAY(SELECT permission FROM group_permissions
                     WHERE group_id = g.id ORDER BY permission) AS "permissions!",
               ARRAY(SELECT u.username FROM user_groups ug JOIN users u ON u.id = ug.user_id
                     WHERE ug.group_id = g.id ORDER BY lower(u.username)) AS "members!"
        FROM groups g
        ORDER BY g.name
        "#
    )
    .fetch_all(conn)
    .await
}

pub async fn get(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<GroupInfo>> {
    let groups = list(conn).await?;
    Ok(groups.into_iter().find(|g| g.id == id))
}

/// Group ids by name, in the same order. Err: the names that don't exist.
pub async fn ids_by_name(
    conn: &mut PgConnection,
    names: &[String],
) -> sqlx::Result<Result<Vec<Uuid>, Vec<String>>> {
    let rows = sqlx::query!("SELECT id, name FROM groups WHERE name = ANY($1)", names)
        .fetch_all(conn)
        .await?;
    let mut ids = Vec::new();
    let mut unknown = Vec::new();
    for name in names {
        match rows.iter().find(|r| &r.name == name) {
            Some(row) => ids.push(row.id),
            None => unknown.push(name.clone()),
        }
    }
    Ok(if unknown.is_empty() { Ok(ids) } else { Err(unknown) })
}

/// Creates a group. The caller has checked the name and permissions.
pub async fn create(
    conn: &mut PgConnection,
    name: &str,
    description: &str,
    permissions: &[String],
) -> sqlx::Result<Uuid> {
    let id = sqlx::query_scalar!(
        "INSERT INTO groups (name, description) VALUES ($1, $2) RETURNING id",
        name,
        description
    )
    .fetch_one(&mut *conn)
    .await?;
    set_permissions(conn, id, permissions).await?;
    Ok(id)
}

pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    name: &str,
    description: &str,
    permissions: &[String],
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE groups SET name = $2, description = $3 WHERE id = $1",
        id,
        name,
        description
    )
    .execute(&mut *conn)
    .await?;
    set_permissions(conn, id, permissions).await
}

async fn set_permissions(conn: &mut PgConnection, id: Uuid, permissions: &[String]) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM group_permissions WHERE group_id = $1", id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "INSERT INTO group_permissions (group_id, permission) SELECT $1, unnest($2::text[])",
        id,
        permissions
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Whether any track is restricted to this group. Deleting such a group would quietly
/// make those tracks admin-only, so that's refused.
///
/// Counts tracks the user can't see too (as the system), since those are affected all the
/// same; the answer is only yes/no.
pub async fn restricts_tracks(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<bool> {
    let system = super::as_system(conn).await?;
    let used = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM track_access WHERE group_id = $1) AS "used!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    system.end(conn).await?;
    Ok(used)
}

pub async fn delete(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!("DELETE FROM groups WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(result.rows_affected() > 0)
}
