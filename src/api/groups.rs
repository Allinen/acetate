//! Managing groups: their permissions, and (through `track_access`) which restricted
//! tracks their members see. Changing groups needs `manage_users`; listing them also
//! works with `manage_access` (to choose who may see a restricted track).
//!
//! As with users (see `users.rs`), you can't hand out more than you have: a non-admin can
//! only give a group permissions they have themselves, and can't change a group that has
//! permissions they lack.

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::groups::{ADMIN_GROUP, GroupInfo};

/// `GET /api/groups`
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<Vec<GroupInfo>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::groups::list(&mut tx).await?))
}

#[derive(Debug, Deserialize)]
pub struct GroupData {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    permissions: Vec<String>,
}

/// `POST /api/groups`, JSON `{"name": "friends", "description": "", "permissions": ["browse", "stream"]}`
pub async fn create(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<GroupData>,
) -> Result<Json<GroupInfo>, AppError> {
    let name = check_name(&body.name)?;
    check_permissions(&user, &body.permissions)?;
    let mut tx = state.db(&user).await?;
    let id = match db::groups::create(&mut tx, name, &body.description, &body.permissions).await {
        Ok(id) => id,
        Err(err) if is_unique_violation(&err) => {
            return Err(AppError::Conflict(format!("a group named `{name}` exists already")));
        }
        Err(err) => return Err(err.into()),
    };
    let created = db::groups::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(created))
}

/// `PUT /api/groups/{id}`, same JSON as creating: replaces name, description and
/// permissions.
pub async fn update(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(body): Json<GroupData>,
) -> Result<Json<GroupInfo>, AppError> {
    let name = check_name(&body.name)?;
    let mut tx = state.db(&user).await?;
    let group = db::groups::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    if group.name == ADMIN_GROUP {
        return Err(AppError::Forbidden(format!("the `{ADMIN_GROUP}` group can't be changed")));
    }
    // Both what it has now and what it would get must be within your own permissions.
    check_permissions(&user, &group.permissions)?;
    check_permissions(&user, &body.permissions)?;
    match db::groups::update(&mut tx, id, name, &body.description, &body.permissions).await {
        Ok(()) => {}
        Err(err) if is_unique_violation(&err) => {
            return Err(AppError::Conflict(format!("a group named `{name}` exists already")));
        }
        Err(err) => return Err(err.into()),
    }
    let updated = db::groups::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(updated))
}

/// `DELETE /api/groups/{id}`
pub async fn delete(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let group = db::groups::get(&mut tx, id).await?.ok_or(AppError::NotFound)?;
    if group.name == ADMIN_GROUP {
        return Err(AppError::Forbidden(format!("the `{ADMIN_GROUP}` group can't be deleted")));
    }
    check_permissions(&user, &group.permissions)?;
    if db::groups::restricts_tracks(&mut tx, id).await? {
        return Err(AppError::Conflict(
            "tracks are restricted to this group; change their access first, or they'd \
             become visible to admins only"
                .to_string(),
        ));
    }
    db::groups::delete(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "deleted": id })))
}

fn check_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name can't be empty".to_string()));
    }
    // Not logged in = "guest", with no permissions, and that's not configurable. A group
    // by that name would suggest otherwise.
    if name.eq_ignore_ascii_case("guest") {
        return Err(AppError::BadRequest(
            "`guest` is reserved: people who aren't logged in have no permissions, and that \
             can't be changed"
                .to_string(),
        ));
    }
    Ok(name)
}

/// Fails unless every permission exists and `user` has it.
fn check_permissions(user: &CurrentUser, permissions: &[String]) -> Result<(), AppError> {
    let mut lacking = Vec::new();
    for name in permissions {
        let Ok(permission) = name.parse::<Permission>() else {
            return Err(AppError::BadRequest(format!("unknown permission `{name}`")));
        };
        if !user.has(permission) {
            lacking.push(name.as_str());
        }
    }
    if !lacking.is_empty() {
        return Err(AppError::Forbidden(format!(
            "permissions you don't have: {}",
            lacking.join(", ")
        )));
    }
    Ok(())
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .is_some_and(|e| e.is_unique_violation())
}
