//! Managing users and their groups. All routes need the `manage_users` permission.
//!
//! You can't hand out more than you have: a non-admin user manager can only put users in
//! groups whose permissions they have themselves, and can't change users who have
//! permissions they lack (e.g. demote an admin).

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{AppError, AppState, auth::check_password};
use crate::auth::{self, CurrentUser, Permission};
use crate::db;
use crate::db::users::UserInfo;

/// `GET /api/users`
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<Vec<UserInfo>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::users::list_users(&mut tx).await?))
}

#[derive(Debug, Deserialize)]
pub struct NewUser {
    username: String,
    password: String,
    /// Group names. Defaults to `["user"]`.
    groups: Option<Vec<String>>,
}

/// `POST /api/users`, JSON `{"username": "...", "password": "...", "groups": ["user"]}`.
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<NewUser>,
) -> Result<Json<UserInfo>, AppError> {
    let username = body.username.trim();
    if username.is_empty() {
        return Err(AppError::BadRequest("username can't be empty".to_string()));
    }
    check_password(&body.password)?;
    let groups = body.groups.unwrap_or_else(|| vec!["user".to_string()]);

    let mut tx = state.db(&user).await?;
    check_can_grant(&mut tx, &user, &groups).await?;
    let hash = auth::hash_password_async(body.password).await?;
    let id = match db::users::create_user(&mut tx, username, &hash).await {
        Ok(id) => id,
        Err(err) if is_unique_violation(&err) => {
            return Err(AppError::Conflict(format!("username `{username}` is taken")));
        }
        Err(err) => return Err(err.into()),
    };
    set_groups(&mut tx, id, &groups).await?;
    let created = db::users::get_user(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(created))
}

#[derive(Debug, Deserialize)]
pub struct SetGroups {
    groups: Vec<String>,
}

/// `PUT /api/users/{id}/groups`, JSON `{"groups": ["user", "editor"]}`: replaces the
/// user's groups.
pub async fn set_user_groups(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<SetGroups>,
) -> Result<Json<UserInfo>, AppError> {
    let mut tx = state.db(&user).await?;
    let target = db::users::get_user(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    // Both what they have now and what they'd get must be within your own permissions.
    check_can_grant(&mut tx, &user, &target.groups).await?;
    check_can_grant(&mut tx, &user, &body.groups).await?;
    set_groups(&mut tx, id, &body.groups).await?;
    let updated = db::users::get_user(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(updated))
}

async fn set_groups(conn: &mut PgConnection, user_id: Uuid, groups: &[String]) -> Result<(), AppError> {
    // The outer `?` is for database errors; the inner result is "these groups won't do".
    db::users::set_groups(conn, user_id, groups)
        .await?
        .map_err(AppError::BadRequest)
}

/// Fails unless `user` has every permission that membership of `groups` gives.
async fn check_can_grant(
    conn: &mut PgConnection,
    user: &CurrentUser,
    groups: &[String],
) -> Result<(), AppError> {
    if user.has(Permission::Admin) {
        return Ok(());
    }
    let granted = db::users::permissions_of_groups(conn, groups).await?;
    let lacking: Vec<String> = granted
        .into_iter()
        .filter(|name| !name.parse().is_ok_and(|p| user.has(p)))
        .collect();
    if !lacking.is_empty() {
        return Err(AppError::Forbidden(format!(
            "these groups have permissions you don't: {}",
            lacking.join(", ")
        )));
    }
    Ok(())
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}
