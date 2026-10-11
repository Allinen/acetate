//! The trash (see `db::trash`): listing, taking things back out, and purging (admins,
//! after the grace period). Also the old audio files: listing replaced ones and trashing
//! them (admins).

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::{CurrentUser, Permission};
use crate::db;
use crate::db::trash::{Kind, ReplacedFile, TrashItem};

/// `GET /api/trash`: what's in the trash (that you can see), oldest first.
pub async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<Vec<TrashItem>>, AppError> {
    let mut tx = state.db(&user).await?;
    let mut items = db::trash::list(&mut tx).await?;
    drop(tx);
    // Covers' sizes aren't in the database; the blob store knows.
    for item in &mut items {
        if item.kind == "cover"
            && let Some(sha) = &item.sha256
        {
            item.size_bytes = state.store.size(sha).await.map(|s| s as i64);
        }
    }
    Ok(Json(items))
}

#[derive(Debug, Deserialize)]
pub struct Item {
    kind: Kind,
    id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct Restore {
    items: Vec<Item>,
}

/// `POST /api/trash/restore`, JSON `{"items": [{"kind": "cover", "id": "..."}]}`: takes
/// them back out of the trash. Needs what trashing them needs: `edit_metadata` for
/// covers and extras, `edit_sources` for sources, `admin` for files.
pub async fn restore(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<Restore>,
) -> Result<Json<serde_json::Value>, AppError> {
    for item in &body.items {
        check_may_trash(&user, item.kind)?;
    }
    let mut tx = state.db(&user).await?;
    for item in &body.items {
        if !db::trash::restore(&mut tx, item.kind, item.id).await? {
            return Err(AppError::BadRequest(format!(
                "{} {} isn't in the trash; nothing was restored",
                item.kind.as_str(),
                item.id
            )));
        }
    }
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "restored": body.items.len() })))
}

/// Who may put things of a kind in the trash (and take them back out).
fn check_may_trash(user: &CurrentUser, kind: Kind) -> Result<(), AppError> {
    let needed = match kind {
        Kind::Cover => Permission::EditMetadata,
        Kind::Source => Permission::EditSources,
        Kind::File => Permission::Admin,
        // Merging them.
        Kind::Extra => Permission::EditMetadata,
        // Whoever may resolve files that didn't import.
        Kind::Problem => Permission::Upload,
    };
    if user.has(needed) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "{}s in the trash need the `{}` permission",
            kind.as_str(),
            needed.name()
        )))
    }
}

#[derive(Debug, Deserialize)]
pub struct Purge {
    items: Vec<Item>,
    /// One item: the first 6+ characters of its sha256 (a source: its id). Several: how
    /// many there are.
    confirm: String,
}

#[derive(Debug, Serialize)]
pub struct PurgeResult {
    purged: usize,
    blobs_deleted: usize,
    bytes_freed: i64,
}

/// `POST /api/trash/purge` (admins), JSON `{"items": [...], "confirm": "3"}`: deletes
/// them for good. Only items at least `TRASH_GRACE_DAYS` in the trash; all or nothing.
pub async fn purge(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<Purge>,
) -> Result<Json<PurgeResult>, AppError> {
    let items: Vec<(Kind, Uuid)> = body.items.iter().map(|i| (i.kind, i.id)).collect();
    let mut tx = state.db(&user).await?;
    let purged = db::trash::purge(&mut tx, &state.store, &items, &body.confirm).await?;
    tx.commit().await?;

    // Only now that the database no longer refers to them.
    let mut blobs_deleted = 0;
    for sha in &purged.delete_blobs {
        match state.store.delete(sha).await {
            Ok(true) => blobs_deleted += 1,
            Ok(false) => {}
            // The purge itself happened; a leftover blob only takes space.
            Err(err) => tracing::error!("purged, but deleting blob {sha} failed: {err:#}"),
        }
    }
    tracing::info!(
        "purged {} items, deleted {blobs_deleted} blobs ({} bytes)",
        purged.items,
        purged.bytes_freed
    );
    Ok(Json(PurgeResult {
        purged: purged.items,
        blobs_deleted,
        bytes_freed: purged.bytes_freed,
    }))
}

// ---- Old audio files (admins) ----

#[derive(Debug, Deserialize)]
pub struct Page {
    before: Option<Uuid>,
    limit: Option<i64>,
}

/// `GET /api/files/replaced?before=&limit=`: previous files of tracks, not in the trash
/// yet, newest first.
pub async fn replaced_files(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(page): Query<Page>,
) -> Result<Json<Vec<ReplacedFile>>, AppError> {
    let limit = page.limit.unwrap_or(100).clamp(1, 1000);
    let mut tx = state.db(&user).await?;
    Ok(Json(db::trash::replaced_files(&mut tx, page.before, limit).await?))
}

#[derive(Debug, Deserialize)]
pub struct TrashFiles {
    file_ids: Vec<Uuid>,
}

/// `POST /api/files/trash`, JSON `{"file_ids": [...]}`: moves old audio files to the
/// trash. Never a file a track currently plays.
pub async fn trash_files(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(body): Json<TrashFiles>,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.db(&user).await?;
    let trashed = db::trash::trash_files(&mut tx, &body.file_ids).await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({ "trashed": trashed })))
}
