//! Revisions: the only way metadata changes.
//!
//! This module is the only code that writes revisions, and the only code that writes the
//! tables holding the current state: the metadata columns of `tracks`, `track_credits`,
//! `album_tracks`, `track_relations`, `artists`, `artist_aliases`, `albums`,
//! `album_credits`. Everything else (ingest, the API, restores) goes through here, so
//! current state and history can't disagree.
//!
//! Every write is a full snapshot:
//! * internal track revision: a variant's `TrackMetadata`. Applied to `tracks` and
//!   `track_credits` if the variant is the primary one.
//! * external track revision: the track's `TrackFacts` (status, albums, relations).
//! * artist / album revision: `ArtistData` / `AlbumData`.
//!
//! Edits pass the id of the revision they were based on (`base`). If that's no longer the
//! latest one, someone else changed it in the meantime and the edit is rejected
//! (`EditRejected::Stale`) instead of silently undoing their change. Writing the same data
//! as the latest revision writes nothing.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::model::{
    AlbumData, AlbumMembership, Alias, ArtistData, Credit, LyricsData, Relation, TrackFacts,
    TrackMetadata, diff_relations,
};

/// Where a revision comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Import,
    Edit,
    Restore,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Import => "import",
            Source::Edit => "edit",
            Source::Restore => "restore",
        }
    }
}

/// An edit that was refused. The client's problem, not a server error: the API turns
/// these into 409 / 400 / 404.
#[derive(Debug)]
pub enum EditRejected {
    /// Someone else changed it after the client loaded it. `latest` is the current revision.
    Stale { latest: Option<Uuid> },
    Invalid(String),
    NotFound(&'static str),
}

impl fmt::Display for EditRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditRejected::Stale { latest } => {
                write!(f, "changed by someone else in the meantime; reload and apply your edit again")?;
                match latest {
                    Some(id) => write!(f, " (latest revision: {id})"),
                    None => Ok(()),
                }
            }
            EditRejected::Invalid(msg) => write!(f, "{msg}"),
            EditRejected::NotFound(what) => write!(f, "{what} not found"),
        }
    }
}

impl std::error::Error for EditRejected {}

fn invalid(msg: String) -> anyhow::Error {
    EditRejected::Invalid(msg).into()
}

/// The result of a write.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Written {
    /// The new revision, or the latest one if nothing changed.
    pub revision_id: Uuid,
    /// False if the data was the same as the latest revision (then nothing was written).
    pub changed: bool,
}

fn check_base(base: Option<Uuid>, latest: Option<Uuid>) -> Result<(), EditRejected> {
    match base {
        Some(base) if Some(base) != latest => Err(EditRejected::Stale { latest }),
        _ => Ok(()),
    }
}

/// True if `latest` (a revision's JSON) holds exactly `data`.
fn same_data<T>(latest: &serde_json::Value, data: &T) -> bool
where
    T: serde::de::DeserializeOwned + PartialEq,
{
    serde_json::from_value::<T>(latest.clone()).is_ok_and(|old| old == *data)
}

// ============================================================================
// Tracks
// ============================================================================

/// Creates a track with its first variant and revision 0 of both kinds. The only way
/// tracks are created (used by ingest).
pub async fn create_track(
    conn: &mut PgConnection,
    file_id: Uuid,
    meta: &TrackMetadata,
    facts: &TrackFacts,
    variant_label: &str,
    reason: &str,
) -> anyhow::Result<Uuid> {
    // Generate the ids up front: the track points at its variant and the variant points
    // at the track. The FK from tracks to track_variants is DEFERRABLE, so it's only
    // checked at COMMIT, by which point both rows exist. (Same for `file_id`: the file
    // row is inserted later in the transaction.)
    let track_id = Uuid::now_v7();
    let variant_id = Uuid::now_v7();

    // Checked up front: the row below already needs a valid status.
    facts.validate(track_id).map_err(invalid)?;
    check_facts(conn, facts).await?;

    // Placeholder values for the NOT NULL columns; the revisions below set the real ones.
    sqlx::query!(
        r#"
        INSERT INTO tracks (id, file_id, status, primary_variant_id, title)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        track_id,
        file_id,
        facts.status,
        variant_id,
        meta.title,
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "INSERT INTO track_variants (id, track_id, label) VALUES ($1, $2, $3)",
        variant_id,
        track_id,
        variant_label,
    )
    .execute(&mut *conn)
    .await?;

    write_internal(conn, track_id, variant_id, None, meta, Source::Import, reason, false).await?;
    write_external(conn, track_id, None, facts, Source::Import, reason).await?;
    Ok(track_id)
}

/// Writes an internal revision: new metadata for one variant of a track. Applied to the
/// track's current state if it's the primary variant.
#[allow(clippy::too_many_arguments)]
pub async fn write_internal(
    conn: &mut PgConnection,
    track_id: Uuid,
    variant_id: Uuid,
    base: Option<Uuid>,
    data: &TrackMetadata,
    source: Source,
    reason: &str,
    is_canon: bool,
) -> anyhow::Result<Written> {
    data.validate().map_err(invalid)?;
    // Credits to merged artists/aliases point at where they were merged into.
    let resolved = TrackMetadata {
        credits: resolve_credits(conn, &data.credits).await?,
        ..data.clone()
    };
    let data = &resolved;

    // Locking the variant row makes concurrent edits of it wait for each other, so the
    // base check below can't be raced.
    let Some(track) = sqlx::query!(
        r#"
        SELECT t.primary_variant_id
        FROM track_variants v
        JOIN tracks t ON t.id = v.track_id
        WHERE v.id = $1 AND v.track_id = $2
        FOR UPDATE OF v
        "#,
        variant_id,
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("variant").into());
    };

    let latest = sqlx::query!(
        "SELECT id, data FROM track_revisions WHERE variant_id = $1 ORDER BY id DESC LIMIT 1",
        variant_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    check_base(base, latest.as_ref().map(|r| r.id))?;

    // Credits of artists the user can't see are carried over unchanged, like relations
    // with hidden tracks in `write_external`. (Only possible on a variant that isn't
    // primary: a credit on the primary variant makes the artist visible.)
    let hidden = match &latest {
        Some(latest) => hidden_credits(conn, &latest.data).await?,
        None => Vec::new(),
    };
    let mut stored = data.clone();
    stored.credits.extend(hidden);

    if let Some(latest) = &latest
        && same_data(&latest.data, &stored)
    {
        return Ok(Written {
            revision_id: latest.id,
            changed: false,
        });
    }
    check_credits(conn, &data.credits).await?;

    let revision_id = sqlx::query_scalar!(
        r#"
        INSERT INTO track_revisions (track_id, variant_id, kind, source, reason, is_canon, data)
        VALUES ($1, $2, 'internal', $3, $4, $5, $6)
        RETURNING id
        "#,
        track_id,
        variant_id,
        source.as_str(),
        reason,
        is_canon,
        serde_json::to_value(&stored)?,
    )
    .fetch_one(&mut *conn)
    .await?;

    if track.primary_variant_id == Some(variant_id) {
        apply_internal(conn, track_id, &stored).await?;
    }
    Ok(Written {
        revision_id,
        changed: true,
    })
}

/// The credits in a variant's revision data whose artist the transaction's user can't see.
async fn hidden_credits(conn: &mut PgConnection, data: &serde_json::Value) -> anyhow::Result<Vec<Credit>> {
    let credits = serde_json::from_value::<TrackMetadata>(data.clone())?.credits;
    let ids: Vec<Uuid> = credits.iter().map(|c| c.artist_id).collect();
    let visible = sqlx::query_scalar!("SELECT id FROM artists WHERE id = ANY($1)", &ids)
        .fetch_all(&mut *conn)
        .await?;
    Ok(credits
        .into_iter()
        .filter(|c| !visible.contains(&c.artist_id))
        .collect())
}

/// Copies a variant's metadata onto the track's current state.
async fn apply_internal(
    conn: &mut PgConnection,
    track_id: Uuid,
    data: &TrackMetadata,
) -> anyhow::Result<()> {
    sqlx::query!(
        r#"
        UPDATE tracks SET title = $2, release_date = $3, genres = $4, updated_at = now()
        WHERE id = $1
        "#,
        track_id,
        data.title,
        data.release_date,
        &data.genres,
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!("DELETE FROM track_credits WHERE track_id = $1", track_id)
        .execute(&mut *conn)
        .await?;
    for (position, credit) in data.credits.iter().enumerate() {
        sqlx::query!(
            r#"
            INSERT INTO track_credits (track_id, artist_id, role, alias_id, position)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            track_id,
            credit.artist_id,
            credit.role,
            credit.alias_id,
            position as i16,
        )
        .execute(&mut *conn)
        .await?;
    }

    // The artist text is derived from the credits just written.
    sqlx::query!(
        "UPDATE tracks SET artist_display = artist_display_of(id) WHERE id = $1",
        track_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Writes an external revision: new status, albums and relations for a track.
///
/// Relations are stored from both sides, so changing one also changes the other track.
/// That track then gets an external revision too, recording its new relations.
pub async fn write_external(
    conn: &mut PgConnection,
    track_id: Uuid,
    base: Option<Uuid>,
    facts: &TrackFacts,
    source: Source,
    reason: &str,
) -> anyhow::Result<Written> {
    facts.validate(track_id).map_err(invalid)?;

    // Lock this track and every track whose relations might change (currently related,
    // or about to be), in id order so two edits can't wait on each other forever.
    let mut involved: BTreeSet<Uuid> = current_relations(conn, track_id)
        .await?
        .into_iter()
        .map(|r| r.track_id)
        .collect();
    involved.insert(track_id);
    involved.extend(facts.relations.iter().map(|r| r.track_id));
    let mut locked = lock_tracks(conn, &involved).await?;
    if !locked.contains(&track_id) {
        return Err(EditRejected::NotFound("track").into());
    }
    let unknown: Vec<String> = facts
        .relations
        .iter()
        .filter(|r| !locked.contains(&r.track_id))
        .map(|r| r.track_id.to_string())
        .collect();
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown tracks: {}", unknown.join(", "))));
    }

    // Relations with tracks the user can't see are neither in `facts` (they'd be "unknown
    // tracks" above) nor in what row-level security lets us read or delete below. They're
    // carried over unchanged, so the revision still records the whole picture.
    let hidden = hidden_relations(conn, track_id).await?;
    let mut stored = facts.clone();
    stored.relations.extend(hidden);

    let latest = latest_external(conn, track_id).await?;
    check_base(base, latest.as_ref().map(|(id, _)| *id))?;
    if let Some((latest_id, latest_data)) = &latest
        && same_data(latest_data, &stored)
    {
        return Ok(Written {
            revision_id: *latest_id,
            changed: false,
        });
    }
    check_facts(conn, facts).await?;

    // Status and albums.
    sqlx::query!(
        "UPDATE tracks SET status = $2, updated_at = now() WHERE id = $1",
        track_id,
        facts.status
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM album_tracks WHERE track_id = $1", track_id)
        .execute(&mut *conn)
        .await?;
    for album in &facts.albums {
        sqlx::query!(
            r#"
            INSERT INTO album_tracks (album_id, track_id, disc_number, track_number)
            VALUES ($1, $2, $3, $4)
            "#,
            album.album_id,
            track_id,
            album.disc_number,
            album.track_number,
        )
        .execute(&mut *conn)
        .await?;
    }

    // Relations: re-read now that everything is locked, and change only the difference.
    let current = current_relations(conn, track_id).await?;
    let (added, removed) = diff_relations(&current, &facts.relations);
    // A relation removed from a track that wasn't locked above (added by someone else just
    // before we locked): lock it too before touching it.
    let late: BTreeSet<Uuid> = removed
        .iter()
        .map(|r| r.track_id)
        .filter(|id| !locked.contains(id))
        .collect();
    if !late.is_empty() {
        locked.extend(lock_tracks(conn, &late).await?);
    }
    let inverse = inverse_kinds(conn).await?;
    for relation in &removed {
        sqlx::query!(
            r#"
            DELETE FROM track_relations
            WHERE (from_track_id = $1 AND kind = $2 AND to_track_id = $3)
               OR (from_track_id = $3 AND kind = $4 AND to_track_id = $1)
            "#,
            track_id,
            relation.kind,
            relation.track_id,
            inverse[&relation.kind],
        )
        .execute(&mut *conn)
        .await?;
    }
    for relation in &added {
        // Both halves; the deferred trigger checks at COMMIT that they're both there.
        sqlx::query!(
            r#"
            INSERT INTO track_relations (from_track_id, kind, to_track_id)
            VALUES ($1, $2, $3), ($3, $4, $1)
            ON CONFLICT DO NOTHING
            "#,
            track_id,
            relation.kind,
            relation.track_id,
            inverse[&relation.kind],
        )
        .execute(&mut *conn)
        .await?;
    }

    let revision_id = insert_external(conn, track_id, &stored, source, reason).await?;

    // The other side of every changed relation gets a revision with its new relations.
    // Its *whole* current facts, including relations to tracks this user can't see (read
    // as the system; nothing of it is returned).
    let others: BTreeSet<Uuid> = added.iter().chain(&removed).map(|r| r.track_id).collect();
    if !others.is_empty() {
        let title = sqlx::query_scalar!("SELECT title FROM tracks WHERE id = $1", track_id)
            .fetch_one(&mut *conn)
            .await?;
        let other_reason = format!("relation changed on \"{title}\"");
        for other in others {
            let system = super::as_system(conn).await?;
            let facts = current_facts(conn, other).await?;
            system.end(conn).await?;
            insert_external(conn, other, &facts, source, &other_reason).await?;
        }
    }

    Ok(Written {
        revision_id,
        changed: true,
    })
}

async fn insert_external(
    conn: &mut PgConnection,
    track_id: Uuid,
    facts: &TrackFacts,
    source: Source,
    reason: &str,
) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar!(
        r#"
        INSERT INTO track_revisions (track_id, kind, source, reason, data)
        VALUES ($1, 'external', $2, $3, $4)
        RETURNING id
        "#,
        track_id,
        source.as_str(),
        reason,
        serde_json::to_value(facts)?,
    )
    .fetch_one(&mut *conn)
    .await?)
}

async fn latest_external(
    conn: &mut PgConnection,
    track_id: Uuid,
) -> sqlx::Result<Option<(Uuid, serde_json::Value)>> {
    let row = sqlx::query!(
        r#"
        SELECT id, data FROM track_revisions
        WHERE track_id = $1 AND kind = 'external'
        ORDER BY id DESC LIMIT 1
        "#,
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| (r.id, r.data)))
}

/// Locks the given tracks (those that exist) and returns their ids.
async fn lock_tracks(
    conn: &mut PgConnection,
    ids: &BTreeSet<Uuid>,
) -> sqlx::Result<BTreeSet<Uuid>> {
    let ids: Vec<Uuid> = ids.iter().copied().collect();
    let locked = sqlx::query_scalar!(
        "SELECT id FROM tracks WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(locked.into_iter().collect())
}

/// A track's current relations, seen from it.
pub async fn current_relations(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<Vec<Relation>> {
    sqlx::query_as!(
        Relation,
        r#"
        SELECT kind, to_track_id AS track_id FROM track_relations
        WHERE from_track_id = $1
        ORDER BY kind, to_track_id
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await
}

/// The track's relations with tracks the transaction's user can't see.
async fn hidden_relations(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<Vec<Relation>> {
    let visible = current_relations(conn, track_id).await?;
    let system = super::as_system(conn).await?;
    let all = current_relations(conn, track_id).await?;
    system.end(conn).await?;
    Ok(all.into_iter().filter(|r| !visible.contains(r)).collect())
}

/// `facts` without what the transaction's user can't see: relations with hidden tracks,
/// and hidden albums. For restoring an old revision: only its visible part is restored
/// (`write_external` keeps the current hidden part).
async fn visible_facts(conn: &mut PgConnection, facts: TrackFacts) -> sqlx::Result<TrackFacts> {
    let track_ids: Vec<Uuid> = facts.relations.iter().map(|r| r.track_id).collect();
    let visible_tracks = sqlx::query_scalar!("SELECT id FROM tracks WHERE id = ANY($1)", &track_ids)
        .fetch_all(&mut *conn)
        .await?;
    let album_ids: Vec<Uuid> = facts.albums.iter().map(|a| a.album_id).collect();
    let visible_albums = sqlx::query_scalar!("SELECT id FROM albums WHERE id = ANY($1)", &album_ids)
        .fetch_all(&mut *conn)
        .await?;
    Ok(TrackFacts {
        status: facts.status,
        albums: facts
            .albums
            .into_iter()
            .filter(|a| visible_albums.contains(&a.album_id))
            .collect(),
        relations: facts
            .relations
            .into_iter()
            .filter(|r| visible_tracks.contains(&r.track_id))
            .collect(),
    })
}

/// A track's facts as they currently are in the tables.
async fn current_facts(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<TrackFacts> {
    let status = sqlx::query_scalar!("SELECT status FROM tracks WHERE id = $1", track_id)
        .fetch_one(&mut *conn)
        .await?;
    let albums = sqlx::query_as!(
        AlbumMembership,
        r#"
        SELECT album_id, disc_number, track_number FROM album_tracks
        WHERE track_id = $1 ORDER BY album_id
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let relations = current_relations(conn, track_id).await?;
    Ok(TrackFacts {
        status,
        albums,
        relations,
    })
}

/// Relation kind -> its inverse (remix_of -> remix_to, alternate -> alternate).
async fn inverse_kinds(conn: &mut PgConnection) -> sqlx::Result<HashMap<String, String>> {
    let rows = sqlx::query!("SELECT kind, inverse_kind FROM relation_kinds")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().map(|r| (r.kind, r.inverse_kind)).collect())
}

// ---- Variants ----

/// Creates a new variant of a track, starting as a copy of `copy_from` (default: the
/// primary variant). Returns the variant id and its first revision.
pub async fn create_variant(
    conn: &mut PgConnection,
    track_id: Uuid,
    label: &str,
    copy_from: Option<Uuid>,
) -> anyhow::Result<(Uuid, Written)> {
    if label.trim().is_empty() {
        return Err(invalid("label can't be empty".to_string()));
    }
    let Some(primary) = sqlx::query_scalar!(
        "SELECT primary_variant_id FROM tracks WHERE id = $1 FOR UPDATE",
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("track").into());
    };
    let from = copy_from.or(primary).ok_or(EditRejected::NotFound("variant"))?;
    let data = latest_internal(conn, track_id, from)
        .await?
        .ok_or(EditRejected::NotFound("variant to copy"))?
        .1;

    let variant_id = sqlx::query_scalar!(
        "INSERT INTO track_variants (track_id, label) VALUES ($1, $2) RETURNING id",
        track_id,
        label
    )
    .fetch_one(&mut *conn)
    .await?;
    let written = write_internal(
        conn,
        track_id,
        variant_id,
        None,
        &data,
        Source::Edit,
        "variant created",
        false,
    )
    .await?;
    Ok((variant_id, written))
}

pub async fn rename_variant(
    conn: &mut PgConnection,
    track_id: Uuid,
    variant_id: Uuid,
    label: &str,
) -> anyhow::Result<()> {
    if label.trim().is_empty() {
        return Err(invalid("label can't be empty".to_string()));
    }
    let result = sqlx::query!(
        "UPDATE track_variants SET label = $3 WHERE id = $1 AND track_id = $2",
        variant_id,
        track_id,
        label
    )
    .execute(&mut *conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(EditRejected::NotFound("variant").into());
    }
    Ok(())
}

/// Makes a variant the one that's shown, applying its latest metadata to the track.
pub async fn set_primary_variant(
    conn: &mut PgConnection,
    track_id: Uuid,
    variant_id: Uuid,
) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar!(
        "SELECT id FROM tracks WHERE id = $1 FOR UPDATE",
        track_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        return Err(EditRejected::NotFound("track").into());
    }
    let (_, data) = latest_internal(conn, track_id, variant_id)
        .await?
        .ok_or(EditRejected::NotFound("variant"))?;
    sqlx::query!(
        "UPDATE tracks SET primary_variant_id = $2 WHERE id = $1",
        track_id,
        variant_id
    )
    .execute(&mut *conn)
    .await?;
    apply_internal(conn, track_id, &data).await
}

/// The latest internal revision of a variant of `track_id`: (revision id, data).
async fn latest_internal(
    conn: &mut PgConnection,
    track_id: Uuid,
    variant_id: Uuid,
) -> anyhow::Result<Option<(Uuid, TrackMetadata)>> {
    let row = sqlx::query!(
        r#"
        SELECT id, data FROM track_revisions
        WHERE track_id = $1 AND variant_id = $2
        ORDER BY id DESC LIMIT 1
        "#,
        track_id,
        variant_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        Some(row) => Ok(Some((row.id, serde_json::from_value(row.data)?))),
        None => Ok(None),
    }
}

/// Restores a track revision (either kind): writes a new revision with its data.
pub async fn restore_track_revision(
    conn: &mut PgConnection,
    revision_id: Uuid,
    reason: &str,
) -> anyhow::Result<Written> {
    let Some(old) = sqlx::query!(
        "SELECT track_id, variant_id, kind, is_canon, data FROM track_revisions WHERE id = $1",
        revision_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("revision").into());
    };
    let reason = restore_reason(reason, revision_id);
    match old.variant_id {
        Some(variant_id) => {
            // Only the visible part is restored; `write_internal` keeps the current
            // credits of hidden artists.
            let hidden = hidden_credits(conn, &old.data).await?;
            let mut data: TrackMetadata = serde_json::from_value(old.data)?;
            data.credits.retain(|c| !hidden.contains(c));
            write_internal(
                conn,
                old.track_id,
                variant_id,
                None,
                &data,
                Source::Restore,
                &reason,
                old.is_canon,
            )
            .await
        }
        None => {
            let facts: TrackFacts = serde_json::from_value(old.data)?;
            let facts = visible_facts(conn, facts).await?;
            write_external(conn, old.track_id, None, &facts, Source::Restore, &reason).await
        }
    }
}

fn restore_reason(reason: &str, revision_id: Uuid) -> String {
    if reason.trim().is_empty() {
        format!("restored revision {revision_id}")
    } else {
        reason.to_string()
    }
}

// ============================================================================
// Lyrics
// ============================================================================
//
// A track can have several lyrics entries (e.g. official and a fan transcription, or two
// languages). Each has its own history, like an artist or album; `lyrics` holds the
// current state.

/// Adds a lyrics entry to a track, with its first revision. `origin` says where the
/// entry came from ("embedded" in the file, "manual"). Returns (lyrics id, revision).
pub async fn create_lyrics(
    conn: &mut PgConnection,
    track_id: Uuid,
    origin: &str,
    data: &LyricsData,
    source: Source,
    reason: &str,
) -> anyhow::Result<(Uuid, Written)> {
    data.validate().map_err(invalid)?;
    let track_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1) AS "exists!""#,
        track_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !track_exists {
        return Err(EditRejected::NotFound("track").into());
    }
    let lyrics_id = sqlx::query_scalar!(
        r#"
        INSERT INTO lyrics (track_id, is_official, is_synced, language, source, body)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
        track_id,
        data.is_official,
        data.is_synced(),
        data.language,
        origin,
        data.body
    )
    .fetch_one(&mut *conn)
    .await?;
    let revision_id = insert_lyrics_revision(conn, lyrics_id, data, source, reason).await?;
    Ok((
        lyrics_id,
        Written {
            revision_id,
            changed: true,
        },
    ))
}

/// Writes a new revision of a lyrics entry.
pub async fn write_lyrics(
    conn: &mut PgConnection,
    lyrics_id: Uuid,
    base: Option<Uuid>,
    data: &LyricsData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Written> {
    data.validate().map_err(invalid)?;
    // Locked, so the base check below can't be raced.
    let found = sqlx::query_scalar!("SELECT id FROM lyrics WHERE id = $1 FOR UPDATE", lyrics_id)
        .fetch_optional(&mut *conn)
        .await?;
    if found.is_none() {
        return Err(EditRejected::NotFound("lyrics").into());
    }
    let latest = sqlx::query!(
        "SELECT id, data FROM lyrics_revisions WHERE lyrics_id = $1 ORDER BY id DESC LIMIT 1",
        lyrics_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    check_base(base, latest.as_ref().map(|r| r.id))?;
    if let Some(latest) = &latest
        && same_data(&latest.data, data)
    {
        return Ok(Written {
            revision_id: latest.id,
            changed: false,
        });
    }

    sqlx::query!(
        r#"
        UPDATE lyrics SET body = $2, is_official = $3, is_synced = $4, language = $5
        WHERE id = $1
        "#,
        lyrics_id,
        data.body,
        data.is_official,
        data.is_synced(),
        data.language
    )
    .execute(&mut *conn)
    .await?;
    let revision_id = insert_lyrics_revision(conn, lyrics_id, data, source, reason).await?;
    Ok(Written {
        revision_id,
        changed: true,
    })
}

async fn insert_lyrics_revision(
    conn: &mut PgConnection,
    lyrics_id: Uuid,
    data: &LyricsData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar!(
        r#"
        INSERT INTO lyrics_revisions (lyrics_id, source, reason, data)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
        lyrics_id,
        source.as_str(),
        reason,
        serde_json::to_value(data)?,
    )
    .fetch_one(&mut *conn)
    .await?)
}

pub async fn restore_lyrics_revision(
    conn: &mut PgConnection,
    revision_id: Uuid,
    reason: &str,
) -> anyhow::Result<Written> {
    let Some(old) = sqlx::query!(
        "SELECT lyrics_id, data FROM lyrics_revisions WHERE id = $1",
        revision_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("revision").into());
    };
    let data: LyricsData = serde_json::from_value(old.data)?;
    let reason = restore_reason(reason, revision_id);
    write_lyrics(conn, old.lyrics_id, None, &data, Source::Restore, &reason).await
}

// ============================================================================
// Artists
// ============================================================================

/// Creates an artist with revision 0.
pub async fn create_artist(
    conn: &mut PgConnection,
    data: &ArtistData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Uuid> {
    data.validate().map_err(invalid)?;
    let artist_id = sqlx::query_scalar!(
        "INSERT INTO artists (name, disambiguation) VALUES ($1, $2) RETURNING id",
        data.name,
        data.disambiguation
    )
    .fetch_one(&mut *conn)
    .await?;
    write_artist(conn, artist_id, None, data, source, reason).await?;
    Ok(artist_id)
}

/// Writes an artist revision: name, disambiguation and the full list of aliases.
///
/// Aliases are matched by id: listed ids keep (and may rename) their alias, entries
/// without an id are new (or re-list a hidden alias with exactly that name). Aliases left
/// out are hidden, never deleted, since revisions refer to them; hiding one that current
/// credits use is refused.
pub async fn write_artist(
    conn: &mut PgConnection,
    artist_id: Uuid,
    base: Option<Uuid>,
    data: &ArtistData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Written> {
    data.validate().map_err(invalid)?;
    let Some(artist) = sqlx::query!(
        "SELECT name, merged_into FROM artists WHERE id = $1 FOR UPDATE",
        artist_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("artist").into());
    };
    if let Some(into) = artist.merged_into {
        return Err(invalid(format!("this artist was merged into {into}; edit that one")));
    }

    let latest = sqlx::query!(
        "SELECT id, data FROM artist_revisions WHERE artist_id = $1 ORDER BY id DESC LIMIT 1",
        artist_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    check_base(base, latest.as_ref().map(|r| r.id))?;

    // The artist's aliases, listed or hidden: id -> (name, listed).
    let existing: HashMap<Uuid, (String, bool)> = sqlx::query!(
        "SELECT id, alias, listed FROM artist_aliases WHERE artist_id = $1 AND merged_into IS NULL",
        artist_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.id, (r.alias, r.listed)))
    .collect();

    // Give every alias an id: known ids must be this artist's; new names reuse a hidden
    // alias of the same name, else get a new row.
    let mut stored = data.clone();
    for alias in &mut stored.aliases {
        match alias.id {
            Some(id) if !existing.contains_key(&id) => {
                return Err(invalid(format!("alias {id} isn't one of this artist's")));
            }
            Some(_) => {}
            None => {
                let hidden = existing
                    .iter()
                    .find(|(_, (name, listed))| !listed && *name == alias.name)
                    .map(|(id, _)| *id);
                alias.id = Some(match hidden {
                    Some(id) => id,
                    None => {
                        sqlx::query_scalar!(
                            "INSERT INTO artist_aliases (artist_id, alias, listed) VALUES ($1, $2, false) RETURNING id",
                            artist_id,
                            alias.name
                        )
                        .fetch_one(&mut *conn)
                        .await?
                    }
                });
            }
        }
    }

    if let Some(latest) = &latest
        && same_data(&latest.data, &stored)
    {
        return Ok(Written {
            revision_id: latest.id,
            changed: false,
        });
    }

    // Aliases being hidden must not be in use by current credits.
    let keep: Vec<Uuid> = stored.aliases.iter().filter_map(|a| a.id).collect();
    let hiding: Vec<Uuid> = existing
        .iter()
        .filter(|(id, (_, listed))| *listed && !keep.contains(id))
        .map(|(id, _)| *id)
        .collect();
    for alias_id in &hiding {
        // Counting uses on hidden tracks too (otherwise hiding the alias would break their
        // credits), so the message mustn't say how many there are.
        let system = super::as_system(conn).await?;
        let uses = alias_uses(conn, *alias_id).await?;
        system.end(conn).await?;
        if uses > 0 {
            let name = &existing[alias_id].0;
            return Err(invalid(format!(
                "alias `{name}` is still credited on tracks or albums; change those credits first"
            )));
        }
    }

    sqlx::query!(
        "UPDATE artists SET name = $2, disambiguation = $3 WHERE id = $1",
        artist_id,
        data.name,
        data.disambiguation
    )
    .execute(&mut *conn)
    .await?;
    // Hide first, then (re)list: a rename to a just-hidden alias's name must not clash.
    sqlx::query!(
        "UPDATE artist_aliases SET listed = false WHERE id = ANY($1)",
        &hiding
    )
    .execute(&mut *conn)
    .await?;
    for alias in &stored.aliases {
        sqlx::query!(
            "UPDATE artist_aliases SET alias = $2, listed = true WHERE id = $1",
            alias.id,
            alias.name
        )
        .execute(&mut *conn)
        .await?;
    }

    // The artist text of tracks crediting this artist is derived from its name/aliases.
    let names_changed = artist.name != data.name
        || stored.aliases.iter().any(|a| {
            a.id.and_then(|id| existing.get(&id))
                .is_some_and(|(old_name, _)| *old_name != a.name)
        });
    if names_changed {
        // Hidden tracks too: their artist text must follow the name like everyone else's.
        let system = super::as_system(conn).await?;
        sqlx::query!(
            r#"
            UPDATE tracks SET artist_display = artist_display_of(id)
            WHERE id IN (SELECT track_id FROM track_credits WHERE artist_id = $1)
            "#,
            artist_id
        )
        .execute(&mut *conn)
        .await?;
        system.end(conn).await?;
    }

    let revision_id = sqlx::query_scalar!(
        r#"
        INSERT INTO artist_revisions (artist_id, source, reason, data)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
        artist_id,
        source.as_str(),
        reason,
        serde_json::to_value(&stored)?,
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Written {
        revision_id,
        changed: true,
    })
}

/// How many tracks/albums currently credit an alias: in current credits, or in the latest
/// revision of any variant (non-primary variants aren't in `track_credits`).
async fn alias_uses(conn: &mut PgConnection, alias_id: Uuid) -> sqlx::Result<i64> {
    let pattern = serde_json::json!({ "credits": [{ "alias_id": alias_id }] });
    let count = sqlx::query_scalar!(
        r#"
        SELECT (SELECT count(*) FROM album_credits WHERE alias_id = $1)
             + (SELECT count(DISTINCT r.variant_id) FROM track_revisions r
                WHERE r.data @> $2
                  AND r.id = (SELECT r2.id FROM track_revisions r2
                              WHERE r2.variant_id = r.variant_id ORDER BY r2.id DESC LIMIT 1))
          AS "count!"
        "#,
        alias_id,
        pattern
    )
    .fetch_one(conn)
    .await?;
    Ok(count)
}

/// The latest data of an artist.
async fn latest_artist_data(conn: &mut PgConnection, artist_id: Uuid) -> anyhow::Result<ArtistData> {
    let data = sqlx::query_scalar!(
        "SELECT data FROM artist_revisions WHERE artist_id = $1 ORDER BY id DESC LIMIT 1",
        artist_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(EditRejected::NotFound("artist"))?;
    Ok(serde_json::from_value(data)?)
}

/// Adds an alias to an artist (an artist revision), or finds the listed one with exactly
/// that name. Returns its id. Used by ingest for new spellings.
pub async fn ensure_alias(
    conn: &mut PgConnection,
    artist_id: Uuid,
    name: &str,
    source: Source,
    reason: &str,
) -> anyhow::Result<Uuid> {
    let mut data = latest_artist_data(conn, artist_id).await?;
    if let Some(id) = data.aliases.iter().find(|a| a.name == name).and_then(|a| a.id) {
        return Ok(id);
    }
    data.aliases.push(Alias {
        id: None,
        name: name.to_string(),
    });
    write_artist(conn, artist_id, None, &data, source, reason).await?;
    let id = sqlx::query_scalar!(
        "SELECT id FROM artist_aliases WHERE artist_id = $1 AND alias = $2 AND listed",
        artist_id,
        name
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// What a merge changed.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Merged {
    /// Variants (of tracks) and albums that got a revision with the credits moved over.
    pub variants: usize,
    pub albums: usize,
    /// Tracks flagged for review (with `flag_for_review`).
    pub flagged: usize,
}

/// Merges artist `from` into `into`: they're the same artist.
///
/// * `from`'s aliases move to `into` (same ids, so old revisions still resolve).
/// * With `keep_name`, `from`'s name becomes an alias of `into`, and credits of `from`
///   under its own name are credited to `into` under that alias: they keep showing the
///   old name. Without it they show `into`'s name.
/// * Every variant and album currently crediting `from` gets a revision crediting `into`.
///   Older revisions are left as they were; `from` stays as a row redirecting to `into`
///   (`merged_into`), so they still resolve (see `resolve_credits`).
/// * `from`'s sources move to `into`.
/// * With `flag_for_review`, every track whose credits changed gets a review flag, so
///   someone can check it (nothing else is changed automatically).
pub async fn merge_artist(
    conn: &mut PgConnection,
    from: Uuid,
    into: Uuid,
    keep_name: bool,
    flag_for_review: bool,
    reason: &str,
) -> anyhow::Result<Merged> {
    if from == into {
        return Err(invalid("can't merge an artist into itself".to_string()));
    }
    // Lock both, in id order.
    let rows = sqlx::query!(
        "SELECT id, name, merged_into FROM artists WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &[from, into][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    let (Some(from_row), Some(into_row)) = (
        rows.iter().find(|r| r.id == from),
        rows.iter().find(|r| r.id == into),
    ) else {
        return Err(EditRejected::NotFound("artist").into());
    };
    if from_row.merged_into.is_some() || into_row.merged_into.is_some() {
        return Err(invalid("that artist was already merged into another".to_string()));
    }
    let from_name = from_row.name.clone();
    let reason = if reason.trim().is_empty() {
        format!("merged \"{from_name}\" into \"{}\"", into_row.name)
    } else {
        reason.to_string()
    };

    // Both artists are visible (the lock above went through row-level security). What
    // follows must also reach tracks and albums the user can't see, or those would keep
    // crediting an artist that no longer exists as such: it runs as the system. The
    // counts returned at the end only include what the user can see.
    let system = super::as_system(conn).await?;

    // 1. Move `from`'s aliases over. A listed alias whose exact name `into` already lists
    //    stays behind and redirects to that one instead.
    let into_listed: HashMap<String, Uuid> = sqlx::query!(
        "SELECT id, alias FROM artist_aliases WHERE artist_id = $1 AND listed",
        into
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.alias, r.id))
    .collect();
    let from_aliases = sqlx::query!(
        "SELECT id, alias, listed FROM artist_aliases WHERE artist_id = $1 AND merged_into IS NULL",
        from
    )
    .fetch_all(&mut *conn)
    .await?;
    for alias in &from_aliases {
        match into_listed.get(&alias.alias) {
            Some(same) if alias.listed => {
                sqlx::query!(
                    "UPDATE artist_aliases SET listed = false, merged_into = $2 WHERE id = $1",
                    alias.id,
                    same
                )
                .execute(&mut *conn)
                .await?;
            }
            _ => {
                sqlx::query!(
                    "UPDATE artist_aliases SET artist_id = $2 WHERE id = $1",
                    alias.id,
                    into
                )
                .execute(&mut *conn)
                .await?;
            }
        }
    }

    // 2. Revisions for both artists' new alias lists. `from` has none left.
    let mut from_data = latest_artist_data(conn, from).await?;
    from_data.aliases.clear();
    write_artist(conn, from, None, &from_data, Source::Edit, &reason).await?;

    let mut into_data = latest_artist_data(conn, into).await?;
    for alias in &from_aliases {
        let moved = alias.listed && !into_listed.contains_key(&alias.alias);
        if moved && alias.alias != into_data.name {
            into_data.aliases.push(Alias {
                id: Some(alias.id),
                name: alias.alias.clone(),
            });
        }
    }
    let keep_as_alias = keep_name && from_name != into_data.name;
    if keep_as_alias && !into_data.aliases.iter().any(|a| a.name == from_name) {
        into_data.aliases.push(Alias {
            id: None,
            name: from_name.clone(),
        });
    }
    write_artist(conn, into, None, &into_data, Source::Edit, &reason).await?;
    let name_alias = if keep_as_alias {
        sqlx::query_scalar!(
            "SELECT id FROM artist_aliases WHERE artist_id = $1 AND alias = $2 AND listed",
            into,
            from_name
        )
        .fetch_optional(&mut *conn)
        .await?
    } else {
        None
    };

    // 3. `from` now redirects to `into` (and so does anything merged into `from` before).
    sqlx::query!(
        "UPDATE artists SET merged_into = $2, merged_alias_id = $3 WHERE id = $1",
        from,
        into,
        name_alias
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE artists SET merged_into = $2 WHERE merged_into = $1",
        from,
        into
    )
    .execute(&mut *conn)
    .await?;

    // 4. Every variant/album currently crediting `from`: rewrite its latest data. The
    //    redirect set above makes `resolve_credits` move the credits to `into`.
    let pattern = serde_json::json!({ "credits": [{ "artist_id": from }] });
    let variants = sqlx::query!(
        r#"
        SELECT r.track_id, r.variant_id AS "variant_id!", r.data
        FROM track_revisions r
        WHERE r.data @> $1
          AND r.id = (SELECT r2.id FROM track_revisions r2
                      WHERE r2.variant_id = r.variant_id ORDER BY r2.id DESC LIMIT 1)
        "#,
        pattern
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut flagged: BTreeSet<Uuid> = BTreeSet::new();
    for variant in &variants {
        let data: TrackMetadata = serde_json::from_value(variant.data.clone())?;
        write_internal(conn, variant.track_id, variant.variant_id, None, &data, Source::Edit, &reason, false)
            .await?;
        // One flag per track, even if several of its variants changed.
        if flag_for_review && flagged.insert(variant.track_id) {
            super::review::flag_merged_artist(conn, variant.track_id, from, &from_name, into).await?;
        }
    }
    let albums = sqlx::query!(
        r#"
        SELECT r.album_id, r.data
        FROM album_revisions r
        WHERE r.data @> $1
          AND r.id = (SELECT r2.id FROM album_revisions r2
                      WHERE r2.album_id = r.album_id ORDER BY r2.id DESC LIMIT 1)
        "#,
        pattern
    )
    .fetch_all(&mut *conn)
    .await?;
    for album in &albums {
        let data: AlbumData = serde_json::from_value(album.data.clone())?;
        write_album(conn, album.album_id, None, &data, Source::Edit, &reason).await?;
    }

    // 5. Sources. One with the same platform id as one of `into`'s joins it (its
    //    snapshots move over); the others just change owner.
    let sources = sqlx::query!(
        "SELECT id, platform, external_id FROM sources WHERE artist_id = $1",
        from
    )
    .fetch_all(&mut *conn)
    .await?;
    for src in sources {
        let same = match &src.external_id {
            Some(external_id) => {
                sqlx::query_scalar!(
                    "SELECT id FROM sources WHERE artist_id = $1 AND platform = $2 AND external_id = $3",
                    into,
                    src.platform,
                    external_id
                )
                .fetch_optional(&mut *conn)
                .await?
            }
            None => None,
        };
        match same {
            Some(target) => {
                sqlx::query!(
                    "UPDATE source_snapshots SET source_id = $2 WHERE source_id = $1",
                    src.id,
                    target
                )
                .execute(&mut *conn)
                .await?;
                sqlx::query!("DELETE FROM sources WHERE id = $1", src.id)
                    .execute(&mut *conn)
                    .await?;
            }
            None => {
                sqlx::query!("UPDATE sources SET artist_id = $2 WHERE id = $1", src.id, into)
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }

    system.end(conn).await?;

    let variant_ids: Vec<Uuid> = variants.iter().map(|v| v.variant_id).collect();
    let album_ids: Vec<Uuid> = albums.iter().map(|a| a.album_id).collect();
    let flagged_ids: Vec<Uuid> = flagged.into_iter().collect();
    let visible = sqlx::query!(
        r#"
        SELECT (SELECT count(*) FROM track_variants WHERE id = ANY($1)) AS "variants!",
               (SELECT count(*) FROM albums WHERE id = ANY($2)) AS "albums!",
               (SELECT count(*) FROM tracks WHERE id = ANY($3)) AS "flagged!"
        "#,
        &variant_ids,
        &album_ids,
        &flagged_ids
    )
    .fetch_one(&mut *conn)
    .await?;
    let flagged = if flag_for_review { visible.flagged as usize } else { 0 };
    Ok(Merged {
        variants: visible.variants as usize,
        albums: visible.albums as usize,
        flagged,
    })
}

pub async fn restore_artist_revision(
    conn: &mut PgConnection,
    revision_id: Uuid,
    reason: &str,
) -> anyhow::Result<Written> {
    let Some(old) = sqlx::query!(
        "SELECT artist_id, data FROM artist_revisions WHERE id = $1",
        revision_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("revision").into());
    };
    let data: ArtistData = serde_json::from_value(old.data)?;
    let reason = restore_reason(reason, revision_id);
    write_artist(conn, old.artist_id, None, &data, Source::Restore, &reason).await
}

// ============================================================================
// Albums
// ============================================================================

/// Creates an album with revision 0.
pub async fn create_album(
    conn: &mut PgConnection,
    data: &AlbumData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Uuid> {
    data.validate().map_err(invalid)?;
    check_album_kind(conn, &data.kind).await?;
    let album_id = sqlx::query_scalar!(
        "INSERT INTO albums (title, kind, release_date) VALUES ($1, $2, $3) RETURNING id",
        data.title,
        data.kind,
        data.release_date
    )
    .fetch_one(&mut *conn)
    .await?;
    write_album(conn, album_id, None, data, source, reason).await?;
    Ok(album_id)
}

pub async fn write_album(
    conn: &mut PgConnection,
    album_id: Uuid,
    base: Option<Uuid>,
    data: &AlbumData,
    source: Source,
    reason: &str,
) -> anyhow::Result<Written> {
    data.validate().map_err(invalid)?;
    let resolved = AlbumData {
        credits: resolve_credits(conn, &data.credits).await?,
        ..data.clone()
    };
    let data = &resolved;
    let exists = sqlx::query_scalar!("SELECT id FROM albums WHERE id = $1 FOR UPDATE", album_id)
        .fetch_optional(&mut *conn)
        .await?;
    if exists.is_none() {
        return Err(EditRejected::NotFound("album").into());
    }

    let latest = sqlx::query!(
        "SELECT id, data FROM album_revisions WHERE album_id = $1 ORDER BY id DESC LIMIT 1",
        album_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    check_base(base, latest.as_ref().map(|r| r.id))?;
    if let Some(latest) = &latest
        && same_data(&latest.data, data)
    {
        return Ok(Written {
            revision_id: latest.id,
            changed: false,
        });
    }
    check_album_kind(conn, &data.kind).await?;
    check_credits(conn, &data.credits).await?;

    sqlx::query!(
        "UPDATE albums SET title = $2, kind = $3, release_date = $4 WHERE id = $1",
        album_id,
        data.title,
        data.kind,
        data.release_date
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM album_credits WHERE album_id = $1", album_id)
        .execute(&mut *conn)
        .await?;
    for (position, credit) in data.credits.iter().enumerate() {
        sqlx::query!(
            r#"
            INSERT INTO album_credits (album_id, artist_id, role, alias_id, position)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            album_id,
            credit.artist_id,
            credit.role,
            credit.alias_id,
            position as i16,
        )
        .execute(&mut *conn)
        .await?;
    }

    let revision_id = sqlx::query_scalar!(
        r#"
        INSERT INTO album_revisions (album_id, source, reason, data)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
        album_id,
        source.as_str(),
        reason,
        serde_json::to_value(data)?,
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Written {
        revision_id,
        changed: true,
    })
}

pub async fn restore_album_revision(
    conn: &mut PgConnection,
    revision_id: Uuid,
    reason: &str,
) -> anyhow::Result<Written> {
    let Some(old) = sqlx::query!(
        "SELECT album_id, data FROM album_revisions WHERE id = $1",
        revision_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("revision").into());
    };
    let data: AlbumData = serde_json::from_value(old.data)?;
    let reason = restore_reason(reason, revision_id);
    write_album(conn, old.album_id, None, &data, Source::Restore, &reason).await
}

// ============================================================================
// Reference checks
// ============================================================================
//
// (The unnested values are named `u.v` on purpose: a bare name like `id` inside the
// subquery would mean the table's own column, making the check always pass.)
//
// Run before writing a revision, so a revision never refers to things that don't exist
// (for a non-primary variant, nothing else would catch it: its data isn't applied).

/// Credits with merged artists and aliases replaced by what they were merged into. A
/// credit of a merged artist under its own name gets the alias that name became (if it
/// was kept). Duplicates this creates (the same artist and role twice) are dropped.
async fn resolve_credits(conn: &mut PgConnection, credits: &[Credit]) -> anyhow::Result<Vec<Credit>> {
    let mut resolved: Vec<Credit> = Vec::with_capacity(credits.len());
    for credit in credits {
        let mut credit = credit.clone();
        // Merges can chain (A into B, B into C); the loop limit is just a safety net.
        for _ in 0..16 {
            let Some(row) = sqlx::query!(
                "SELECT merged_into, merged_alias_id FROM artists WHERE id = $1",
                credit.artist_id
            )
            .fetch_optional(&mut *conn)
            .await?
            else {
                break; // unknown artist: reported by check_credits
            };
            let Some(into) = row.merged_into else { break };
            credit.artist_id = into;
            if credit.alias_id.is_none() {
                credit.alias_id = row.merged_alias_id;
            }
        }
        if let Some(alias_id) = credit.alias_id {
            let merged = sqlx::query_scalar!(
                "SELECT merged_into FROM artist_aliases WHERE id = $1",
                alias_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
            if let Some(into) = merged {
                credit.alias_id = Some(into);
            }
        }
        if !resolved.iter().any(|c| c.artist_id == credit.artist_id && c.role == credit.role) {
            resolved.push(credit);
        }
    }
    Ok(resolved)
}

async fn check_credits(conn: &mut PgConnection, credits: &[Credit]) -> anyhow::Result<()> {
    let artist_ids: Vec<Uuid> = credits.iter().map(|c| c.artist_id).collect();
    let unknown = sqlx::query_scalar!(
        r#"
        SELECT u.v AS "id!" FROM unnest($1::uuid[]) AS u(v)
        WHERE NOT EXISTS (SELECT 1 FROM artists a WHERE a.id = u.v)
        "#,
        &artist_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    if !unknown.is_empty() {
        let list: Vec<String> = unknown.iter().map(Uuid::to_string).collect();
        return Err(invalid(format!("unknown artists: {}", list.join(", "))));
    }

    let roles: Vec<String> = credits.iter().map(|c| c.role.clone()).collect();
    let unknown = sqlx::query_scalar!(
        r#"
        SELECT u.v AS "role!" FROM unnest($1::text[]) AS u(v)
        WHERE NOT EXISTS (SELECT 1 FROM credit_roles r WHERE r.role = u.v)
        "#,
        &roles
    )
    .fetch_all(&mut *conn)
    .await?;
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown credit roles: {}", unknown.join(", "))));
    }

    // An alias must be one of the credited artist's, and listed.
    let (alias_artists, alias_ids): (Vec<Uuid>, Vec<Uuid>) = credits
        .iter()
        .filter_map(|c| c.alias_id.map(|alias| (c.artist_id, alias)))
        .unzip();
    let bad = sqlx::query_scalar!(
        r#"
        SELECT u.alias AS "alias!" FROM unnest($1::uuid[], $2::uuid[]) AS u(artist, alias)
        WHERE NOT EXISTS (SELECT 1 FROM artist_aliases al
                          WHERE al.id = u.alias AND al.artist_id = u.artist AND al.listed)
        "#,
        &alias_artists,
        &alias_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    if !bad.is_empty() {
        let list: Vec<String> = bad.iter().map(Uuid::to_string).collect();
        return Err(invalid(format!(
            "these aliases aren't (listed) aliases of the credited artist: {}",
            list.join(", ")
        )));
    }
    Ok(())
}

async fn check_facts(conn: &mut PgConnection, facts: &TrackFacts) -> anyhow::Result<()> {
    let status_ok = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM track_statuses WHERE status = $1) AS "ok!""#,
        facts.status
    )
    .fetch_one(&mut *conn)
    .await?;
    if !status_ok {
        return Err(invalid(format!("unknown status `{}`", facts.status)));
    }

    let album_ids: Vec<Uuid> = facts.albums.iter().map(|a| a.album_id).collect();
    let unknown = sqlx::query_scalar!(
        r#"
        SELECT u.v AS "id!" FROM unnest($1::uuid[]) AS u(v)
        WHERE NOT EXISTS (SELECT 1 FROM albums a WHERE a.id = u.v)
        "#,
        &album_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    if !unknown.is_empty() {
        let list: Vec<String> = unknown.iter().map(Uuid::to_string).collect();
        return Err(invalid(format!("unknown albums: {}", list.join(", "))));
    }

    let kinds: Vec<String> = facts.relations.iter().map(|r| r.kind.clone()).collect();
    let unknown = sqlx::query_scalar!(
        r#"
        SELECT u.v AS "kind!" FROM unnest($1::text[]) AS u(v)
        WHERE NOT EXISTS (SELECT 1 FROM relation_kinds k WHERE k.kind = u.v)
        "#,
        &kinds
    )
    .fetch_all(&mut *conn)
    .await?;
    if !unknown.is_empty() {
        return Err(invalid(format!("unknown relation kinds: {}", unknown.join(", "))));
    }
    Ok(())
}

async fn check_album_kind(conn: &mut PgConnection, kind: &str) -> anyhow::Result<()> {
    let ok = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM album_kinds WHERE kind = $1) AS "ok!""#,
        kind
    )
    .fetch_one(&mut *conn)
    .await?;
    if !ok {
        return Err(invalid(format!("unknown album kind `{kind}`")));
    }
    Ok(())
}
