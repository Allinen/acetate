//! Users, groups and login sessions.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// How long a session lasts without being used. Using it pushes the expiry forward.
const SESSION_DAYS: i32 = 30;

pub struct Session {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    /// Permission names, from all the user's groups.
    pub permissions: Vec<String>,
    pub last_used_at: DateTime<Utc>,
}

/// Looks up a live (not expired) session by token hash, with its user's permissions.
///
/// Sessions are looked up before we know who the user is, so these queries take the pool
/// rather than a transaction from `db::begin`.
pub async fn find_session(pool: &PgPool, token_sha256: &str) -> sqlx::Result<Option<Session>> {
    sqlx::query_as!(
        Session,
        r#"
        SELECT s.id AS session_id, u.id AS user_id, u.username, s.last_used_at,
               ARRAY(
                   SELECT DISTINCT gp.permission
                   FROM user_groups ug
                   JOIN group_permissions gp ON gp.group_id = ug.group_id
                   WHERE ug.user_id = u.id
               ) AS "permissions!"
        FROM sessions s
        JOIN users u ON u.id = s.user_id
        WHERE s.token_sha256 = $1 AND s.expires_at > now()
        "#,
        token_sha256
    )
    .fetch_optional(pool)
    .await
}

/// Marks a session as used and pushes its expiry forward. Skipped if it was already done
/// in the last few minutes, so a stream of requests (e.g. seeking) doesn't mean a stream of
/// writes.
pub async fn touch_session(pool: &PgPool, session: &Session) -> sqlx::Result<()> {
    if Utc::now() - session.last_used_at < chrono::Duration::minutes(5) {
        return Ok(());
    }
    sqlx::query!(
        r#"
        UPDATE sessions
        SET last_used_at = now(), expires_at = now() + make_interval(days => $2)
        WHERE id = $1
        "#,
        session.session_id,
        SESSION_DAYS
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    token_sha256: &str,
    user_agent: Option<&str>,
    ip: &str,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        r#"
        INSERT INTO sessions (user_id, token_sha256, user_agent, ip, expires_at)
        VALUES ($1, $2, $3, $4, now() + make_interval(days => $5))
        RETURNING id
        "#,
        user_id,
        token_sha256,
        user_agent,
        ip,
        SESSION_DAYS
    )
    .fetch_one(pool)
    .await
}

/// Deletes one of `user_id`'s sessions. Returns false if they have no such session.
pub async fn delete_session(pool: &PgPool, user_id: Uuid, session_id: Uuid) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM sessions WHERE id = $1 AND user_id = $2",
        session_id,
        user_id
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub user_agent: Option<String>,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Updated at most every few minutes (see `touch_session`).
    pub last_used_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// The session this request was made with.
    pub current: bool,
}

/// A user's live sessions, newest first. `current` marks the one making the request.
pub async fn list_sessions(
    pool: &PgPool,
    user_id: Uuid,
    current: Uuid,
) -> sqlx::Result<Vec<SessionInfo>> {
    sqlx::query_as!(
        SessionInfo,
        r#"
        SELECT id, user_agent, ip, created_at, last_used_at, expires_at,
               id = $2 AS "current!"
        FROM sessions
        WHERE user_id = $1 AND expires_at > now()
        ORDER BY id DESC
        "#,
        user_id,
        current
    )
    .fetch_all(pool)
    .await
}

/// Logs a user out everywhere, except (optionally) the session `keep`.
pub async fn delete_sessions_of(pool: &PgPool, user_id: Uuid, keep: Option<Uuid>) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM sessions WHERE user_id = $1 AND id IS DISTINCT FROM $2",
        user_id,
        keep
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub struct Login {
    pub user_id: Uuid,
    pub password_hash: Option<String>,
}

/// For logging in: a user's id and password hash, by username (case-insensitive).
pub async fn find_login(pool: &PgPool, username: &str) -> sqlx::Result<Option<Login>> {
    sqlx::query_as!(
        Login,
        "SELECT id AS user_id, password_hash FROM users WHERE lower(username) = lower($1)",
        username
    )
    .fetch_optional(pool)
    .await
}

pub async fn find_user_id(conn: &mut PgConnection, username: &str) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar!(
        "SELECT id FROM users WHERE lower(username) = lower($1)",
        username
    )
    .fetch_optional(conn)
    .await
}

pub async fn password_hash(conn: &mut PgConnection, user_id: Uuid) -> sqlx::Result<Option<String>> {
    let hash = sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = $1", user_id)
        .fetch_optional(conn)
        .await?;
    Ok(hash.flatten())
}

/// Creates a user. Fails with a unique violation (`users_username_key`) if the name is
/// taken, ignoring case.
pub async fn create_user(
    conn: &mut PgConnection,
    username: &str,
    password_hash: &str,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar!(
        "INSERT INTO users (username, password_hash) VALUES ($1, $2) RETURNING id",
        username,
        password_hash
    )
    .fetch_one(conn)
    .await
}

pub async fn set_password(conn: &mut PgConnection, user_id: Uuid, password_hash: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users SET password_hash = $2 WHERE id = $1",
        user_id,
        password_hash
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Replaces a user's groups with `groups` (names). Changes nothing and returns an error
/// message if some of them don't exist.
pub async fn set_groups(
    conn: &mut PgConnection,
    user_id: Uuid,
    groups: &[String],
) -> sqlx::Result<Result<(), String>> {
    let unknown = unknown_groups(&mut *conn, groups).await?;
    if !unknown.is_empty() {
        return Ok(Err(format!("unknown groups: {}", unknown.join(", "))));
    }
    sqlx::query!("DELETE FROM user_groups WHERE user_id = $1", user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        r#"
        INSERT INTO user_groups (user_id, group_id)
        SELECT $1, id FROM groups WHERE name = ANY($2)
        "#,
        user_id,
        groups
    )
    .execute(&mut *conn)
    .await?;
    Ok(Ok(()))
}

async fn unknown_groups(conn: &mut PgConnection, groups: &[String]) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        r#"
        SELECT name AS "name!" FROM unnest($1::text[]) AS name
        WHERE name NOT IN (SELECT name FROM groups)
        "#,
        groups
    )
    .fetch_all(conn)
    .await
}

/// Every permission that membership of `groups` (names) would give.
pub async fn permissions_of_groups(
    conn: &mut PgConnection,
    groups: &[String],
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar!(
        r#"
        SELECT DISTINCT gp.permission
        FROM groups g
        JOIN group_permissions gp ON gp.group_id = g.id
        WHERE g.name = ANY($1)
        "#,
        groups
    )
    .fetch_all(conn)
    .await
}

#[derive(Debug, Serialize)]
pub struct UserInfo {
    pub id: Uuid,
    pub username: String,
    pub groups: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub has_password: bool,
}

pub async fn list_users(conn: &mut PgConnection) -> sqlx::Result<Vec<UserInfo>> {
    sqlx::query_as!(
        UserInfo,
        r#"
        SELECT u.id, u.username, u.created_at,
               u.password_hash IS NOT NULL AS "has_password!",
               ARRAY(
                   SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id
                   WHERE ug.user_id = u.id ORDER BY g.name
               ) AS "groups!"
        FROM users u
        ORDER BY lower(u.username)
        "#
    )
    .fetch_all(conn)
    .await
}

pub async fn get_user(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<UserInfo>> {
    sqlx::query_as!(
        UserInfo,
        r#"
        SELECT u.id, u.username, u.created_at,
               u.password_hash IS NOT NULL AS "has_password!",
               ARRAY(
                   SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id
                   WHERE ug.user_id = u.id ORDER BY g.name
               ) AS "groups!"
        FROM users u
        WHERE u.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}
