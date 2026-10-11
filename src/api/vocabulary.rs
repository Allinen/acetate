//! The words Acetate knows, for an admin to edit (see `db::vocabulary`). Forms read them
//! from `GET /api/vocab`; this is the whole table, with how many rows use each.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::vocabulary::{Change, NewWord, Word};

/// `GET /api/vocabulary`: every word of every list, in the order to show them.
pub async fn list(State(state): State<AppState>, user: CurrentUser) -> Result<Json<Vec<Word>>, AppError> {
    let mut tx = state.db(&user).await?;
    Ok(Json(db::vocabulary::all(&mut tx).await?))
}

/// `POST /api/vocabulary`, JSON `{"list", "word", "label", "description", "position"}`, and
/// for a relation kind `"inverse"`, `"template"` (and `"inverse_template"`). Admin.
pub async fn add(State(state): State<AppState>, user: CurrentUser, Json(new): Json<NewWord>) -> Result<Json<Value>, AppError> {
    let mut tx = state.db(&user).await?;
    db::vocabulary::add(&mut tx, new).await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

/// `PUT /api/vocabulary/{list}/{word}`, JSON `{"rename_to", "label", "description",
/// "position", "template"}`, any of them. Admin.
pub async fn change(State(state): State<AppState>, user: CurrentUser, Path((list, word)): Path<(String, String)>, Json(change): Json<Change>) -> Result<Json<Value>, AppError> {
    let mut tx = state.db(&user).await?;
    db::vocabulary::change(&mut tx, &list, &word, change).await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

/// `DELETE /api/vocabulary/{list}/{word}`: refused for a locked word and one in use. Admin.
pub async fn remove(State(state): State<AppState>, user: CurrentUser, Path((list, word)): Path<(String, String)>) -> Result<Json<Value>, AppError> {
    let mut tx = state.db(&user).await?;
    db::vocabulary::remove(&mut tx, &list, &word).await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::StatusCode;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

    use crate::api::uploads::tests::{app, send, user};

    #[sqlx::test]
    async fn an_admin_adds_renames_and_removes_words_but_not_the_locked_ones(_: PgPoolOptions, connect: PgConnectOptions) -> anyhow::Result<()> {
        let (owner, app, _dir) = app(connect).await?;
        let alice = user(&owner, "alice").await?;
        let json = Some("application/json");
        let body = |v: serde_json::Value| Body::from(v.to_string());

        let (status, got) = send(&app, "POST", "/api/vocabulary", &alice, body(serde_json::json!({ "list": "extra_kind", "word": "reference" })), json).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        let (_, all) = send(&app, "GET", "/api/vocabulary", &alice, Body::empty(), None).await;
        let find = |all: &serde_json::Value, word: &str| all.as_array().unwrap().iter().find(|w| w["word"] == word).cloned();
        assert_eq!(find(&all, "reference").unwrap()["uses"], 0);
        let (_, vocab) = send(&app, "GET", "/api/vocab", &alice, Body::empty(), None).await;
        assert!(vocab["extra_kinds"].as_array().unwrap().iter().any(|k| k == "reference"), "forms get it");

        let (status, got) = send(&app, "PUT", "/api/vocabulary/extra_kind/reference", &alice, body(serde_json::json!({ "rename_to": "guide", "description": "a guide track" })), json).await;
        assert_eq!(status, StatusCode::OK, "{got}");
        let (_, all) = send(&app, "GET", "/api/vocabulary", &alice, Body::empty(), None).await;
        assert_eq!((find(&all, "guide").unwrap()["description"].clone(), find(&all, "reference")), ("a guide track".into(), None));

        let (status, got) = send(&app, "DELETE", "/api/vocabulary/credit_role/primary", &alice, Body::empty(), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(got["message"].as_str().unwrap().contains("built around"), "{got}");
        let (status, _) = send(&app, "DELETE", "/api/vocabulary/extra_kind/guide", &alice, Body::empty(), None).await;
        assert_eq!(status, StatusCode::OK);
        Ok(())
    }
}
