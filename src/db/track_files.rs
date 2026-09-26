//! Which file a track plays: sharing another track's file, and going back to a previous
//! file. (Uploading a new file is `ingest::add_file_to_track`; merges are in `revisions`.)
//!
//! Every switch is recorded in `track_file_history`, and the file given up stays a
//! previous file of the track: nothing is lost, and a file no track plays anymore is
//! listed for cleanup (`/api/files/replaced`), like after an upgrade or a merge.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// A track switched files.
#[derive(Debug, Serialize, PartialEq)]
pub struct FileSwitch {
    pub track_id: Uuid,
    /// What it played before (now one of its previous files).
    pub old_file_id: Uuid,
    pub new_file_id: Uuid,
}

/// Another track playing the same file.
#[derive(Debug, Serialize)]
pub struct SharedWith {
    pub id: Uuid,
    pub title: String,
    pub artist_display: String,
}

fn invalid(msg: &str) -> anyhow::Error {
    EditRejected::Invalid(msg.to_string()).into()
}

/// The current file of a track, locked. Through row-level security: the track must be
/// visible to the user. A merged track has no file of its own anymore, so it's refused.
async fn lock_current_file(conn: &mut PgConnection, track_id: Uuid) -> anyhow::Result<Uuid> {
    let row = sqlx::query!(
        "SELECT file_id, merged_into FROM tracks WHERE id = $1 FOR UPDATE",
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        None => Err(EditRejected::NotFound("track").into()),
        Some(row) => match (row.merged_into, row.file_id) {
            (Some(_), _) | (None, None) => Err(invalid("this track was merged into another one")),
            (None, Some(file_id)) => Ok(file_id),
        },
    }
}

/// Points the track at another file, recording the switch.
async fn switch(conn: &mut PgConnection, track_id: Uuid, old_file_id: Uuid, new_file_id: Uuid, reason: &str) -> anyhow::Result<FileSwitch> {
    super::ingest::replace_track_file(conn, track_id, old_file_id, new_file_id, reason).await?;
    Ok(FileSwitch { track_id, old_file_id, new_file_id })
}

/// Makes `track_id` play the file `other_id` plays. Both keep everything else (ids,
/// metadata, albums, covers): they just share the bytes from now on. The track's own file
/// becomes one of its previous files (`use_previous_file` switches back).
pub async fn use_file_of(conn: &mut PgConnection, track_id: Uuid, other_id: Uuid, reason: &str) -> anyhow::Result<FileSwitch> {
    if track_id == other_id {
        return Err(invalid("a track can't take the file of itself"));
    }
    // Locked in id order, so two opposite requests at once can't deadlock.
    let (first, second) = if track_id < other_id { (track_id, other_id) } else { (other_id, track_id) };
    let first_file = lock_current_file(conn, first).await?;
    let second_file = lock_current_file(conn, second).await?;
    let (current, wanted) = if first == track_id { (first_file, second_file) } else { (second_file, first_file) };
    if current == wanted {
        return Err(invalid("these tracks already play the same file"));
    }
    let title = sqlx::query_scalar!("SELECT title FROM tracks WHERE id = $1", other_id)
        .fetch_one(&mut *conn)
        .await?;
    let reason = match reason.trim() {
        "" => format!("uses the file of \"{title}\" ({other_id})"),
        given => given.to_string(),
    };
    let switched = switch(conn, track_id, current, wanted, &reason).await?;
    // Deciding to keep both as separate tracks answers "are these duplicates?" for these
    // two (and only these two: other possible duplicates of either stay flagged).
    super::review::settle_duplicate_pair(conn, track_id, other_id, &format!("shared file: {reason}")).await?;
    Ok(switched)
}

/// Makes a track play one of its previous files again (e.g. after `use_file_of`, or to
/// undo an upgrade). The current file becomes a previous file in turn. A previous file in
/// the trash is taken back out; a purged one is gone and can't be used.
pub async fn use_previous_file(conn: &mut PgConnection, track_id: Uuid, file_id: Uuid, reason: &str) -> anyhow::Result<FileSwitch> {
    let current = lock_current_file(conn, track_id).await?;
    if current == file_id {
        return Err(invalid("this is already the track's current file"));
    }
    let file = sqlx::query!(
        r#"
        SELECT f.purged_at IS NOT NULL AS "purged!"
        FROM track_file_history h JOIN files f ON f.id = h.old_file_id
        WHERE h.track_id = $1 AND h.old_file_id = $2
        LIMIT 1
        "#,
        track_id,
        file_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match file {
        None => return Err(invalid("that's not one of this track's previous files")),
        Some(f) if f.purged => return Err(invalid("that file was purged: its audio is gone")),
        Some(_) => {}
    }
    // Taken out of the trash (a current file can't be in it).
    sqlx::query!(
        "UPDATE files SET trashed_at = NULL, trashed_by = NULL WHERE id = $1 AND trashed_at IS NOT NULL",
        file_id
    )
    .execute(&mut *conn)
    .await?;
    let reason = match reason.trim() {
        "" => "back to a previous file".to_string(),
        given => given.to_string(),
    };
    switch(conn, track_id, current, file_id, &reason).await
}

/// The other tracks (visible to the user) playing this file.
pub async fn shared_with(conn: &mut PgConnection, file_id: Uuid, except: Uuid) -> sqlx::Result<Vec<SharedWith>> {
    sqlx::query_as!(
        SharedWith,
        "SELECT id, title, artist_display FROM tracks WHERE file_id = $1 AND id <> $2 ORDER BY id",
        file_id,
        except
    )
    .fetch_all(conn)
    .await
}
