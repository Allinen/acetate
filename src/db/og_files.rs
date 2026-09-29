//! Whether a file is an original file (an "og file", `files.original_filename`): ingest
//! knows it only from a tag, and some original files came without one. Corrected here, one
//! file at a time (any name, or not an original file after all), or for many tracks at once
//! (each marked as one under the name it was uploaded with). Every change is kept
//! (`file_original_filename_changes`, migration 0038). Needs `edit_files`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// A change of a file's original filename. Names None: not an original file.
#[derive(Debug, Serialize)]
pub struct Change {
    pub id: Uuid,
    pub changed_at: DateTime<Utc>,
    pub changed_by: Option<String>,
    pub reason: String,
    pub old_name: Option<String>,
    pub new_name: Option<String>,
}

/// Sets a file's original filename (None: not an original file). Returns false when it's
/// that already (nothing kept).
pub async fn set(conn: &mut PgConnection, file_id: Uuid, name: Option<&str>, reason: &str) -> anyhow::Result<bool> {
    let name = name.map(str::trim);
    if name.is_some_and(str::is_empty) {
        return Err(EditRejected::Invalid("an original filename can't be empty (to say it isn't an original file, send null)".into()).into());
    }
    let Some(now) = sqlx::query_scalar!("SELECT original_filename FROM files WHERE id = $1 FOR UPDATE", file_id)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Err(EditRejected::NotFound("file").into());
    };
    if now.as_deref() == name {
        return Ok(false);
    }
    sqlx::query!("UPDATE files SET original_filename = $2 WHERE id = $1", file_id, name)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "INSERT INTO file_original_filename_changes (file_id, old_name, new_name, reason) VALUES ($1, $2, $3, $4)",
        file_id,
        now,
        name,
        reason.trim()
    )
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// A track whose file was marked (or would be).
#[derive(Debug, Serialize, PartialEq)]
pub struct MarkedTrack {
    pub track_id: Uuid,
    pub title: String,
    /// The name it's marked with: the one it was uploaded with.
    pub name: String,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Marked {
    pub tracks: Vec<MarkedTrack>,
    /// Their files were original files already.
    pub already: usize,
    /// Not there (merged away since, or not seen).
    pub not_found: usize,
}

/// Each track's file (the one it plays now) marked as an original file, under the name it
/// was uploaded with.
pub async fn mark_original(conn: &mut PgConnection, tracks: &[Uuid], reason: &str) -> anyhow::Result<Marked> {
    let reason = if reason.trim().is_empty() { "marked as an original file" } else { reason.trim() };
    let mut out = Marked::default();
    for &track in tracks {
        let Some(row) = sqlx::query!(
            "SELECT t.title, f.id AS file_id, f.original_filename, f.uploaded_filename
             FROM tracks t JOIN files f ON f.id = t.file_id WHERE t.id = $1 AND t.merged_into IS NULL",
            track
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            out.not_found += 1;
            continue;
        };
        if row.original_filename.is_some() {
            out.already += 1;
            continue;
        }
        set(conn, row.file_id, Some(&row.uploaded_filename), reason).await?;
        out.tracks.push(MarkedTrack { track_id: track, title: row.title, name: row.uploaded_filename });
    }
    Ok(out)
}

/// A file's original-filename changes, newest first.
pub async fn changes(conn: &mut PgConnection, file_id: Uuid) -> sqlx::Result<Vec<Change>> {
    sqlx::query_as!(
        Change,
        r#"
        SELECT c.id, c.changed_at, u.username AS "changed_by?", c.reason, c.old_name, c.new_name
        FROM file_original_filename_changes c
        LEFT JOIN users u ON u.id = c.changed_by
        WHERE c.file_id = $1
        ORDER BY c.id DESC
        "#,
        file_id
    )
    .fetch_all(conn)
    .await
}
