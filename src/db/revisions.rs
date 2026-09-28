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

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::model::sources::SourceOwner;
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
    let resolved = TrackFacts {
        albums: resolve_albums(conn, &facts.albums).await?,
        ..facts.clone()
    };
    let facts = &resolved;
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
pub(super) async fn current_facts(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<TrackFacts> {
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
/// Whether `candidate` is metadata the track has, or has had: any revision of any of its
/// variants. Credits are compared by the artist they lead to now (following merges) and
/// role, not by alias: a name that now resolves to the artist a credit's artist was merged
/// into is the same credit. (For `ingest::merge_duplicate`: tags that match the track's
/// history aren't new.)
pub async fn metadata_is_known(
    conn: &mut PgConnection,
    track_id: Uuid,
    candidate: &TrackMetadata,
) -> anyhow::Result<bool> {
    let rows = sqlx::query_scalar!(
        "SELECT r.data FROM track_revisions r JOIN track_variants v ON v.id = r.variant_id WHERE v.track_id = $1",
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let known: Vec<TrackMetadata> = rows
        .into_iter()
        .filter(|data| data.get("title").is_some())
        .filter_map(|data| serde_json::from_value(data).ok())
        .collect();
    let normalized = |m: &TrackMetadata, now: &HashMap<Uuid, Uuid>| {
        let credits: Vec<(Uuid, String)> = m
            .credits
            .iter()
            .map(|c| (*now.get(&c.artist_id).unwrap_or(&c.artist_id), c.role.clone()))
            .collect();
        (m.title.clone(), m.release_date.clone(), m.genres.clone(), m.comment.clone(), credits)
    };
    let ids: Vec<Uuid> = known.iter().chain(std::iter::once(candidate)).flat_map(|m| m.credits.iter().map(|c| c.artist_id)).collect();
    let now: HashMap<Uuid, Uuid> = sqlx::query!(
        r#"SELECT id AS "id!", COALESCE(merged_into, id) AS "now!" FROM artists WHERE id = ANY($1)"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.id, r.now))
    .collect();
    let wanted = normalized(candidate, &now);
    Ok(known.iter().any(|m| normalized(m, &now) == wanted))
}

/// A track's current metadata: its primary variant's latest revision.
pub async fn primary_metadata(conn: &mut PgConnection, track_id: Uuid) -> anyhow::Result<Option<TrackMetadata>> {
    let variant = sqlx::query_scalar!("SELECT primary_variant_id FROM tracks WHERE id = $1", track_id)
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
    let Some(variant) = variant else {
        return Ok(None);
    };
    Ok(latest_internal(conn, track_id, variant).await?.map(|(_, data)| data))
}

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
// Merging tracks
// ============================================================================

/// Which of the two tracks wins a field in a merge: left = the track kept (merged into),
/// right = the duplicate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Left,
    Right,
}

impl Side {
    /// `left` or `right`, whichever this side is.
    fn pick<T>(self, left: T, right: T) -> T {
        match self {
            Side::Left => left,
            Side::Right => right,
        }
    }
}

/// What to take from which side. Everything not listed is left's. (Sources, albums,
/// relations, lyrics and variants aren't picked: they're combined.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergePicks {
    pub title: Side,
    pub release_date: Side,
    pub genres: Side,
    pub comment: Side,
    pub credits: Side,
    pub status: Side,
    /// Which file stays the track's current one; the other becomes a previous file.
    pub file: Side,
    /// Whose covers come first (the first is the main cover); the other side's follow.
    pub covers: Side,
    /// What the duplicate has that the kept track has too (a variant whose metadata the
    /// kept track has or had, see `metadata_is_known`; a cover with the same image) stays
    /// with the duplicate instead of being doubled. For a file imported again (see `ingest`); off for merges by hand.
    pub leave_identical: bool,
    /// None of the duplicate's variants move: its metadata isn't news (e.g. the same file
    /// imported again, whose tags are the same bytes). For `ingest`; off for merges by hand.
    pub leave_variants: bool,
}

#[derive(Debug, Serialize)]
pub struct MergedTracks {
    /// The track that was kept.
    pub track_id: Uuid,
    /// The merge (`track_merges`), e.g. to undo it.
    pub merge_id: Uuid,
    pub variants_moved: usize,
    pub lyrics_moved: usize,
}

/// Merges track `from` (the duplicate, "right") into `into` (kept, "left"): the same song
/// stored twice.
///
/// * The kept track's primary variant gets a revision with each field from the side
///   `picks` says; `from`'s variants (with their history) become extra variants of it.
/// * The file `picks.file` says stays current; the other becomes a previous file (so it
///   can be trashed and purged). `from`'s previous files move along.
/// * All covers are kept: the picked side's first, then the other side's.
/// * Albums, relations, lyrics and sources are combined; a relation between the two
///   tracks is dropped.
/// * `from` then redirects to `into` (`merged_into`), and its shares point at `into`.
///
/// `into_base` / `from_base`: the primary variants' revisions the choice was made from
/// (409 if either changed since). Both tracks must be visible to the user. What's moved
/// includes things only others can see (e.g. a relation with a restricted track); that
/// part runs as the system, like other cascades.
pub async fn merge_tracks(
    conn: &mut PgConnection,
    into: Uuid,
    from: Uuid,
    picks: &MergePicks,
    into_base: Option<Uuid>,
    from_base: Option<Uuid>,
    reason: &str,
) -> anyhow::Result<MergedTracks> {
    if into == from {
        return Err(invalid("can't merge a track into itself".to_string()));
    }
    // Lock both, in id order (through row-level security: both must be visible).
    let rows = sqlx::query!(
        r#"
        SELECT id, title, file_id, primary_variant_id, merged_into
        FROM tracks WHERE id = ANY($1) ORDER BY id FOR UPDATE
        "#,
        &[into, from][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    let (Some(kept), Some(dupe)) = (
        rows.iter().find(|r| r.id == into),
        rows.iter().find(|r| r.id == from),
    ) else {
        return Err(EditRejected::NotFound("track").into());
    };
    if kept.merged_into.is_some() || dupe.merged_into.is_some() {
        return Err(invalid("one of these tracks was already merged into another".to_string()));
    }
    let (Some(kept_variant), Some(dupe_variant), Some(kept_file), Some(dupe_file)) = (
        kept.primary_variant_id,
        dupe.primary_variant_id,
        kept.file_id,
        dupe.file_id,
    ) else {
        return Err(invalid("both tracks need a primary variant and a file".to_string()));
    };
    let (Some((kept_rev, left)), Some((dupe_rev, right))) = (
        latest_internal(conn, into, kept_variant).await?,
        latest_internal(conn, from, dupe_variant).await?,
    ) else {
        return Err(invalid("both tracks need metadata".to_string()));
    };
    check_base(into_base, Some(kept_rev))?;
    check_base(from_base, Some(dupe_rev))?;
    let dupe_title = dupe.title.clone();
    let reason = if reason.trim().is_empty() {
        format!("merged \"{dupe_title}\" ({from})")
    } else {
        reason.to_string()
    };

    // Everything below may touch rows the user can't see (see above).
    let system = super::as_system(conn).await?;

    // What happens is written down as it goes, so it can be undone (`undo_track_merge`).
    let kept_external_before = latest_external(conn, into).await?.map(|(id, _)| id);
    let dupe_external_before = latest_external(conn, from).await?.map(|(id, _)| id);

    // 1. Metadata, field by field.
    let metadata = TrackMetadata {
        title: picks.title.pick(left.title, right.title),
        release_date: picks.release_date.pick(left.release_date, right.release_date),
        genres: picks.genres.pick(left.genres, right.genres),
        comment: picks.comment.pick(left.comment, right.comment),
        credits: picks.credits.pick(left.credits, right.credits),
    };
    let kept_internal = write_internal(conn, into, kept_variant, None, &metadata, Source::Edit, &reason, false).await?;

    // 2. Albums and relations, combined. The duplicate lets go of its own first, which
    //    also drops a relation between the two (with revisions on the other sides).
    let dupe_facts = current_facts(conn, from).await?;
    let emptied = TrackFacts {
        status: dupe_facts.status.clone(),
        albums: Vec::new(),
        relations: Vec::new(),
    };
    let dupe_external = write_external(conn, from, None, &emptied, Source::Edit, &reason).await?;
    let mut facts = current_facts(conn, into).await?;
    facts.status = picks.status.pick(facts.status, dupe_facts.status);
    for album in dupe_facts.albums {
        // On an album both are on, left's disc/track number wins.
        if !facts.albums.iter().any(|a| a.album_id == album.album_id) {
            facts.albums.push(album);
        }
    }
    for relation in dupe_facts.relations {
        if relation.track_id != into && !facts.relations.contains(&relation) {
            facts.relations.push(relation);
        }
    }
    let kept_external = write_external(conn, into, None, &facts, Source::Edit, &reason).await?;

    // 3. Variants: the duplicate's become the kept track's, with their revisions (the
    //    foreign key moves those along). The duplicate keeps no current metadata. With
    //    `leave_identical`, one whose metadata the kept track has or had stays behind.
    let mut variants = sqlx::query_as!(
        MovedVariant,
        "SELECT id, label FROM track_variants WHERE track_id = $1 ORDER BY id",
        from
    )
    .fetch_all(&mut *conn)
    .await?;
    if picks.leave_variants {
        variants.clear();
    } else if picks.leave_identical {
        let mut differing = Vec::new();
        for variant in variants {
            let known = match latest_internal(conn, from, variant.id).await? {
                Some((_, data)) => metadata_is_known(conn, into, &data).await?,
                None => false,
            };
            if !known {
                differing.push(variant);
            }
        }
        variants = differing;
    }
    let moved_variant_ids: Vec<Uuid> = variants.iter().map(|v| v.id).collect();
    sqlx::query!("UPDATE tracks SET primary_variant_id = NULL WHERE id = $1", from)
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM track_credits WHERE track_id = $1", from)
        .execute(&mut *conn)
        .await?;
    let from_label = format!(" (from \"{dupe_title}\")");
    sqlx::query!(
        "UPDATE track_variants SET track_id = $1, label = label || $3 WHERE id = ANY($2)",
        into,
        &moved_variant_ids,
        from_label
    )
    .execute(&mut *conn)
    .await?;

    // 4. Files. The duplicate gives up its file, becoming a redirect.
    sqlx::query!(
        "UPDATE tracks SET file_id = NULL, merged_into = $2, updated_at = now() WHERE id = $1",
        from,
        into
    )
    .execute(&mut *conn)
    .await?;
    let history_moved = sqlx::query_scalar!(
        "UPDATE track_file_history SET track_id = $2 WHERE track_id = $1 RETURNING id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    let (current, previous) = picks.file.pick((kept_file, dupe_file), (dupe_file, kept_file));
    if current != kept_file {
        sqlx::query!("UPDATE tracks SET file_id = $2 WHERE id = $1", into, current)
            .execute(&mut *conn)
            .await?;
    }
    // Two tracks sharing one file (`track_files::use_file_of`) have no other file to keep.
    let history_added = if previous == current {
        None
    } else {
        let id = sqlx::query_scalar!(
            r#"
            INSERT INTO track_file_history (track_id, old_file_id, new_file_id, reason)
            VALUES ($1, $2, $3, $4)
            RETURNING id
            "#,
            into,
            previous,
            current,
            reason
        )
        .fetch_one(&mut *conn)
        .await?;
        Some(id)
    };

    // 5. Covers: all of them move over; the picked side's come first. With
    //    `leave_identical`, an image the kept track has already stays behind.
    let kept_covers = cover_positions(conn, into).await?;
    let leave: Vec<Uuid> = if picks.leave_identical {
        sqlx::query_scalar!(
            r#"
            SELECT d.id FROM covers d
            WHERE d.track_id = $1
              AND EXISTS (SELECT 1 FROM covers k WHERE k.track_id = $2 AND k.trashed_at IS NULL
                                                     AND k.blob_sha256 = d.blob_sha256)
            "#,
            from,
            into
        )
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    let dupe_covers: Vec<CoverPosition> =
        cover_positions(conn, from).await?.into_iter().filter(|c| !leave.contains(&c.id)).collect();
    let dupe_all_covers = sqlx::query_scalar!(
        "UPDATE covers SET track_id = $2 WHERE track_id = $1 AND NOT (id = ANY($3)) RETURNING id",
        from,
        into,
        &leave
    )
    .fetch_all(&mut *conn)
    .await?;
    let (first, then) = match picks.covers {
        Side::Left => (&kept_covers, &dupe_covers),
        Side::Right => (&dupe_covers, &kept_covers),
    };
    let order: Vec<Uuid> = first.iter().chain(then.iter()).map(|c| c.id).collect();
    set_cover_positions(conn, &order).await?;

    // 6. Lyrics move with their history, except ones the kept track has word for word.
    let lyrics = sqlx::query_scalar!(
        r#"
        UPDATE lyrics l SET track_id = $2
        WHERE l.track_id = $1
          AND NOT EXISTS (SELECT 1 FROM lyrics k WHERE k.track_id = $2 AND k.body = l.body)
        RETURNING l.id
        "#,
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;

    // 7. Sources (same platform id: joined).
    let sources = super::sources::move_all(conn, SourceOwner::Track(from), SourceOwner::Track(into)).await?;

    // Playlists: entries of the duplicate play the kept track now (anyone's playlists,
    // private ones too: this runs as the system).
    let playlist_entries = sqlx::query_scalar!(
        "UPDATE playlist_entries SET track_id = $2 WHERE track_id = $1 RETURNING id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;

    // 8. Links to the duplicate lead to the kept track.
    let shares = sqlx::query_scalar!(
        "UPDATE shares SET track_id = $2 WHERE track_id = $1 RETURNING id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    let redirected = sqlx::query_scalar!(
        "UPDATE tracks SET merged_into = $2 WHERE merged_into = $1 RETURNING id",
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    // Users' plays, stars and ratings: the kept track gets both tracks' combined.
    let annotations = super::annotations::combine_into(
        conn,
        super::annotations::Item::Track(from),
        super::annotations::Item::Track(into),
    )
    .await?;
    // Review flags: "are these two duplicates?" is answered, for these two only (a flag on
    // the kept track that also names other tracks stays open for those). The duplicate's
    // own flags are all closed: it's only a redirect now.
    let resolution = format!("merged into {into}");
    let mut flags = super::review::settle_duplicate_pair(conn, into, from, &resolution).await?;
    flags.extend(
        sqlx::query_scalar!(
            r#"
            UPDATE review_flags
            SET resolved_at = now(), resolved_by = current_app_user(), resolution = $2
            WHERE resolved_at IS NULL AND track_id = $1
            RETURNING id
            "#,
            from,
            resolution
        )
        .fetch_all(&mut *conn)
        .await?,
    );

    let record = MergeRecord {
        kept_variant,
        kept_internal_before: kept_rev,
        kept_internal_after: kept_internal.revision_id,
        kept_external_before,
        kept_external_after: kept_external.revision_id,
        dupe_external_before,
        dupe_external_after: dupe_external.revision_id,
        dupe_primary_variant: dupe_variant,
        variants,
        kept_file,
        dupe_file,
        file_pick: picks.file,
        history_moved,
        history_added,
        kept_covers,
        dupe_covers,
        dupe_all_covers,
        covers_trashed: Vec::new(),
        lyrics: lyrics.clone(),
        sources,
        shares,
        redirected,
        flags,
        annotations,
        playlist_entries,
    };
    let merge_id = sqlx::query_scalar!(
        r#"
        INSERT INTO track_merges (into_track_id, from_track_id, picks, reason, moved)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        into,
        from,
        serde_json::to_value(picks)?,
        reason,
        serde_json::to_value(&record)?
    )
    .fetch_one(&mut *conn)
    .await?;

    system.end(conn).await?;
    Ok(MergedTracks {
        track_id: into,
        merge_id,
        variants_moved: record.variants.len(),
        lyrics_moved: lyrics.len(),
    })
}

/// Everything a merge did, by id: stored with the merge (`track_merges.moved`) so it can
/// be undone exactly.
#[derive(Debug, Serialize, Deserialize)]
struct MergeRecord {
    kept_variant: Uuid,
    kept_internal_before: Uuid,
    kept_internal_after: Uuid,
    kept_external_before: Option<Uuid>,
    kept_external_after: Uuid,
    dupe_external_before: Option<Uuid>,
    dupe_external_after: Uuid,
    dupe_primary_variant: Uuid,
    /// The duplicate's variants, with their labels from before.
    variants: Vec<MovedVariant>,
    kept_file: Uuid,
    dupe_file: Uuid,
    file_pick: Side,
    /// The duplicate's file history rows (now the kept track's).
    history_moved: Vec<Uuid>,
    /// The row saying the file not picked became a previous file. None if both tracks
    /// played the same file.
    history_added: Option<Uuid>,
    /// Both sides' covers (not in the trash) and their order before.
    kept_covers: Vec<CoverPosition>,
    dupe_covers: Vec<CoverPosition>,
    /// All of the duplicate's covers that moved, trashed ones included.
    dupe_all_covers: Vec<Uuid>,
    /// Covers the merge put in the trash: the side not picked, in merges made before every
    /// cover was kept (2026-09-26). Empty now.
    covers_trashed: Vec<Uuid>,
    lyrics: Vec<Uuid>,
    sources: super::sources::MovedSources,
    shares: Vec<Uuid>,
    redirected: Vec<Uuid>,
    /// The review flags the merge settled. Kept for the record only: an undo doesn't open
    /// them again (whoever undid the merge knows about the two tracks).
    flags: Vec<Uuid>,
    /// Users' plays, stars and ratings of the duplicate, combined into the kept track's.
    /// (Missing in merges recorded before annotations existed.)
    #[serde(default)]
    annotations: Vec<super::annotations::CombinedAnnotation>,
    /// Playlist entries of the duplicate, now pointing at the kept track. (Missing in
    /// merges recorded before playlists existed.)
    #[serde(default)]
    playlist_entries: Vec<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MovedVariant {
    id: Uuid,
    label: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CoverPosition {
    id: Uuid,
    position: i16,
}

/// A merge into a track, for its page.
#[derive(Debug, Serialize)]
pub struct MergeInfo {
    pub id: Uuid,
    pub from_track_id: Uuid,
    pub from_title: String,
    pub merged_at: chrono::DateTime<chrono::Utc>,
    pub merged_by: Option<String>,
    pub undone_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Why it can't be undone (empty: it can).
    pub undo_blockers: Vec<String>,
}

/// The merges into a track, newest first.
pub async fn merges_into(conn: &mut PgConnection, track_id: Uuid) -> anyhow::Result<Vec<MergeInfo>> {
    let rows = sqlx::query!(
        r#"
        SELECT m.id, m.from_track_id, t.title AS from_title, m.merged_at, u.username AS "merged_by?",
               m.undone_at
        FROM track_merges m
        JOIN tracks t ON t.id = m.from_track_id
        LEFT JOIN users u ON u.id = m.merged_by
        WHERE m.into_track_id = $1
        ORDER BY m.id DESC
        "#,
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut merges = Vec::new();
    for row in rows {
        let undo_blockers = if row.undone_at.is_some() {
            vec!["it was undone already".to_string()]
        } else {
            merge_undo_blockers(conn, row.id).await?
        };
        merges.push(MergeInfo {
            id: row.id,
            from_track_id: row.from_track_id,
            from_title: row.from_title,
            merged_at: row.merged_at,
            merged_by: row.merged_by,
            undone_at: row.undone_at,
            undo_blockers,
        });
    }
    Ok(merges)
}

/// Why a merge can't be undone, in plain words (empty: it can). A merge can be undone
/// only while nothing depends on it: nothing happened to the kept track since, and
/// nothing it moved or trashed has been purged.
pub async fn merge_undo_blockers(conn: &mut PgConnection, merge_id: Uuid) -> anyhow::Result<Vec<String>> {
    let Some(merge) = sqlx::query!(
        "SELECT into_track_id, from_track_id, moved, undone_at FROM track_merges WHERE id = $1",
        merge_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("merge").into());
    };
    if merge.undone_at.is_some() {
        return Ok(vec!["it was undone already".to_string()]);
    }
    let Some(moved) = merge.moved else {
        return Ok(vec!["it was made before merges could be undone".to_string()]);
    };
    let record: MergeRecord = serde_json::from_value(moved)?;
    let (kept, dupe) = (merge.into_track_id, merge.from_track_id);

    // Checked as the system: whether something changed, not what, and nothing is returned
    // but these sentences.
    let system = super::as_system(conn).await?;
    let mut blockers = Vec::new();
    let later_merge = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM track_merges WHERE into_track_id = $1 AND id > $2 AND undone_at IS NULL) AS "e!""#,
        kept,
        merge_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if later_merge {
        blockers.push("another track was merged into it later (undo that first)".to_string());
    }
    let tracks = sqlx::query!(
        "SELECT id, file_id, primary_variant_id, merged_into FROM tracks WHERE id = ANY($1)",
        &[kept, dupe][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    let kept_row = tracks.iter().find(|t| t.id == kept);
    let dupe_row = tracks.iter().find(|t| t.id == dupe);
    if dupe_row.and_then(|t| t.merged_into) != Some(kept) {
        blockers.push("the merged track doesn't redirect here anymore".to_string());
    }
    // "Changed" means the data differs from what the merge left, not just that there's a
    // newer revision: undoing a later merge writes revisions with the same data again.
    let latest_internal_data = sqlx::query_scalar!(
        "SELECT data FROM track_revisions WHERE variant_id = $1 ORDER BY id DESC LIMIT 1",
        record.kept_variant
    )
    .fetch_optional(&mut *conn)
    .await?;
    if latest_internal_data != Some(revision_data(conn, record.kept_internal_after).await?)
        || kept_row.and_then(|t| t.primary_variant_id) != Some(record.kept_variant)
    {
        blockers.push("its metadata (or primary variant) was changed after the merge".to_string());
    }
    if latest_external(conn, kept).await?.map(|(_, data)| data)
        != Some(revision_data(conn, record.kept_external_after).await?)
    {
        blockers.push("its status, albums or relations changed after the merge".to_string());
    }
    if latest_external(conn, dupe).await?.map(|(_, data)| data)
        != Some(revision_data(conn, record.dupe_external_after).await?)
    {
        blockers.push("the merged track's relations changed after the merge".to_string());
    }
    let expected_file = record.file_pick.pick(record.kept_file, record.dupe_file);
    let later_file = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM track_file_history WHERE track_id = $1 AND id > $2) AS "e!""#,
        kept,
        // Without a row of its own, the merge's id (also a UUIDv7) marks the moment.
        record.history_added.unwrap_or(merge_id)
    )
    .fetch_one(&mut *conn)
    .await?;
    if kept_row.and_then(|t| t.file_id) != Some(expected_file) || later_file {
        blockers.push("its file was replaced after the merge".to_string());
    }
    let not_picked = record.file_pick.pick(record.dupe_file, record.kept_file);
    let purged = sqlx::query_scalar!("SELECT purged_at IS NOT NULL AS \"p!\" FROM files WHERE id = $1", not_picked)
        .fetch_one(&mut *conn)
        .await?;
    if purged {
        blockers.push("the file that wasn't picked has been purged".to_string());
    }
    let variant_ids: Vec<Uuid> = record.variants.iter().map(|v| v.id).collect();
    let variants_here = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM track_variants WHERE id = ANY($1) AND track_id = $2"#,
        &variant_ids,
        kept
    )
    .fetch_one(&mut *conn)
    .await?;
    if variants_here as usize != variant_ids.len() {
        blockers.push("the variants that came with the merge changed".to_string());
    }
    let covers_left = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM covers WHERE id = ANY($1)"#,
        &record.covers_trashed
    )
    .fetch_one(&mut *conn)
    .await?;
    if covers_left as usize != record.covers_trashed.len() {
        blockers.push("covers it put in the trash have been purged".to_string());
    }
    let mut source_ids = record.sources.moved.clone();
    source_ids.extend(record.sources.joined.iter().map(|j| j.into_source));
    let sources_left = sqlx::query_scalar!(
        r#"SELECT count(DISTINCT id) AS "n!" FROM sources WHERE id = ANY($1)"#,
        &source_ids
    )
    .fetch_one(&mut *conn)
    .await?;
    let distinct: std::collections::BTreeSet<Uuid> = source_ids.iter().copied().collect();
    if sources_left as usize != distinct.len() {
        blockers.push("sources that came with the merge have been purged".to_string());
    }
    system.end(conn).await?;
    Ok(blockers)
}

/// Undoes a merge: gives back both tracks as they were before it (see `merge_tracks`).
/// Refused, listing why, unless nothing depends on it (`merge_undo_blockers`). Both tracks
/// must be visible to the user.
pub async fn undo_track_merge(conn: &mut PgConnection, merge_id: Uuid, reason: &str) -> anyhow::Result<Uuid> {
    let Some(merge) = sqlx::query!(
        "SELECT into_track_id, from_track_id, moved FROM track_merges WHERE id = $1",
        merge_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("merge").into());
    };
    let (kept, dupe) = (merge.into_track_id, merge.from_track_id);
    // Lock both (through row-level security: both must be visible).
    let locked = sqlx::query_scalar!(
        "SELECT id FROM tracks WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &[kept, dupe][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    if locked.len() != 2 {
        return Err(EditRejected::NotFound("track").into());
    }
    let blockers = merge_undo_blockers(conn, merge_id).await?;
    if !blockers.is_empty() {
        return Err(invalid(format!("this merge can't be undone: {}", blockers.join("; "))));
    }
    let record: MergeRecord = serde_json::from_value(merge.moved.unwrap_or_default())?;
    let reason = if reason.trim().is_empty() {
        format!("undid merge {merge_id}")
    } else {
        reason.to_string()
    };

    let system = super::as_system(conn).await?;

    // 1. The kept track's metadata as it was.
    let before = sqlx::query_scalar!("SELECT data FROM track_revisions WHERE id = $1", record.kept_internal_before)
        .fetch_one(&mut *conn)
        .await?;
    let before: TrackMetadata = serde_json::from_value(before)?;
    write_internal(conn, kept, record.kept_variant, None, &before, Source::Restore, &reason, false).await?;

    // 2. Variants back (with their labels), so the duplicate has metadata again.
    for variant in &record.variants {
        sqlx::query!(
            "UPDATE track_variants SET track_id = $2, label = $3 WHERE id = $1",
            variant.id,
            dupe,
            variant.label
        )
        .execute(&mut *conn)
        .await?;
    }

    // 3. Files: each track its own again; the duplicate stops redirecting.
    if let Some(history_added) = record.history_added {
        sqlx::query!("DELETE FROM track_file_history WHERE id = $1", history_added)
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query!(
        "UPDATE track_file_history SET track_id = $2 WHERE id = ANY($1)",
        &record.history_moved,
        dupe
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("UPDATE tracks SET file_id = $2 WHERE id = $1", kept, record.kept_file)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        r#"
        UPDATE tracks SET file_id = $2, merged_into = NULL, primary_variant_id = $3, updated_at = now()
        WHERE id = $1
        "#,
        dupe,
        record.dupe_file,
        record.dupe_primary_variant
    )
    .execute(&mut *conn)
    .await?;
    // The file that wasn't picked may have been put in the trash since.
    sqlx::query!(
        "UPDATE files SET trashed_at = NULL, trashed_by = NULL WHERE id = ANY($1) AND purged_at IS NULL",
        &[record.kept_file, record.dupe_file][..]
    )
    .execute(&mut *conn)
    .await?;
    if let Some((_, data)) = latest_internal(conn, dupe, record.dupe_primary_variant).await? {
        apply_internal(conn, dupe, &data).await?;
    }

    // 4. Albums and relations as they were (the relation between the two comes back too).
    for (track, revision) in [(kept, record.kept_external_before), (dupe, record.dupe_external_before)] {
        let facts: TrackFacts = match revision {
            Some(id) => {
                let data = sqlx::query_scalar!("SELECT data FROM track_revisions WHERE id = $1", id)
                    .fetch_one(&mut *conn)
                    .await?;
                serde_json::from_value(data)?
            }
            None => TrackFacts::default(),
        };
        write_external(conn, track, None, &facts, Source::Restore, &reason).await?;
    }

    // 5. Covers back, the ones the merge trashed out of the trash, the old order.
    sqlx::query!(
        "UPDATE covers SET track_id = $2 WHERE id = ANY($1)",
        &record.dupe_all_covers,
        dupe
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE covers SET trashed_at = NULL, trashed_by = NULL WHERE id = ANY($1)",
        &record.covers_trashed
    )
    .execute(&mut *conn)
    .await?;
    for side in [&record.kept_covers, &record.dupe_covers] {
        let ids: Vec<Uuid> = side.iter().map(|c| c.id).collect();
        let positions: Vec<i16> = side.iter().map(|c| c.position).collect();
        sqlx::query!(
            r#"
            UPDATE covers c SET position = p.position
            FROM unnest($1::uuid[], $2::smallint[]) AS p(id, position)
            WHERE c.id = p.id
            "#,
            &ids,
            &positions
        )
        .execute(&mut *conn)
        .await?;
    }

    // 6. Lyrics, sources, playlist entries (those still on the kept track).
    sqlx::query!("UPDATE lyrics SET track_id = $2 WHERE id = ANY($1)", &record.lyrics, dupe)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "UPDATE playlist_entries SET track_id = $2 WHERE id = ANY($1) AND track_id = $3",
        &record.playlist_entries,
        dupe,
        kept
    )
    .execute(&mut *conn)
    .await?;
    super::sources::unmove_all(conn, &record.sources, SourceOwner::Track(dupe), SourceOwner::Track(kept)).await?;

    // 7. Links and redirects as they were. (Review flags stay settled, see `MergeRecord::flags`.)
    sqlx::query!("UPDATE shares SET track_id = $2 WHERE id = ANY($1)", &record.shares, dupe)
        .execute(&mut *conn)
        .await?;
    // Users' plays, stars and ratings: the kept track's lose the duplicate's again.
    super::annotations::split(conn, super::annotations::Item::Track(kept), &record.annotations).await?;
    sqlx::query!("UPDATE tracks SET merged_into = $2 WHERE id = ANY($1)", &record.redirected, dupe)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "UPDATE track_merges SET undone_at = now(), undone_by = current_app_user() WHERE id = $1",
        merge_id
    )
    .execute(&mut *conn)
    .await?;

    system.end(conn).await?;
    Ok(dupe)
}

async fn revision_data(conn: &mut PgConnection, revision_id: Uuid) -> sqlx::Result<serde_json::Value> {
    sqlx::query_scalar!("SELECT data FROM track_revisions WHERE id = $1", revision_id)
        .fetch_one(conn)
        .await
}

/// For merges made by importing a stored file again (`ingest::merge_duplicate`) before an
/// album an import creates got the track's covers (before 2026-09-27): an album the
/// re-import brought (e.g. the single), which the kept track wasn't on before and which
/// still has no covers of its own, gets copies of the covers that came with that import
/// (the merged-away track's), in their order. So it shows its own cover instead of its
/// first track's main one, the other release's. Returns (album title, covers copied) for
/// each album that got some. The merge's undo is unaffected (the copies stay with the
/// album, which goes back to the merged-away track with it).
pub async fn copy_reimport_covers_to_albums(
    conn: &mut PgConnection,
    merge_id: Uuid,
) -> anyhow::Result<Vec<(String, usize)>> {
    let merge = sqlx::query!(
        "SELECT from_track_id, picks, moved FROM track_merges WHERE id = $1 AND undone_at IS NULL",
        merge_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let picks: MergePicks = serde_json::from_value(merge.picks)?;
    if !picks.leave_identical {
        return Ok(Vec::new()); // a merge by hand, not a re-import
    }
    let Some(moved) = merge.moved else {
        return Ok(Vec::new());
    };
    let record: MergeRecord = serde_json::from_value(moved)?;

    // The albums the import brought: the merged-away track's before the merge, minus the
    // kept track's before the merge.
    let albums_in = async |conn: &mut PgConnection, revision: Option<Uuid>| -> anyhow::Result<Vec<Uuid>> {
        let Some(revision) = revision else {
            return Ok(Vec::new());
        };
        let data = sqlx::query_scalar!("SELECT data FROM track_revisions WHERE id = $1", revision)
            .fetch_one(&mut *conn)
            .await?;
        let facts: TrackFacts = serde_json::from_value(data)?;
        Ok(facts.albums.iter().map(|a| a.album_id).collect())
    };
    let kept_before = albums_in(conn, record.kept_external_before).await?;
    let brought: Vec<Uuid> = albums_in(conn, record.dupe_external_before)
        .await?
        .into_iter()
        .filter(|album| !kept_before.contains(album))
        .collect();

    // The import's covers, in its order: the ones that moved (with their positions from
    // before) and the ones left behind as identical.
    let mut covers: Vec<(i16, Uuid)> = record.dupe_covers.iter().map(|c| (c.position, c.id)).collect();
    for left in cover_positions(conn, merge.from_track_id).await? {
        covers.push((left.position, left.id));
    }
    covers.sort();

    let mut copied = Vec::new();
    for album in brought {
        let row = sqlx::query!(
            r#"
            SELECT title, merged_into IS NOT NULL AS "merged!",
                   EXISTS (SELECT 1 FROM covers c WHERE c.album_id = a.id AND c.trashed_at IS NULL) AS "has_covers!"
            FROM albums a WHERE id = $1
            "#,
            album
        )
        .fetch_one(&mut *conn)
        .await?;
        // Merged since: its covers went with it (see `album_merges`).
        if row.merged || row.has_covers {
            continue;
        }
        let mut count = 0;
        for (_, cover) in &covers {
            if super::album_merges::copy_if_new(conn, *cover, album).await?.is_some() {
                count += 1;
            }
        }
        if count > 0 {
            copied.push((row.title, count));
        }
    }
    Ok(copied)
}

/// For track merges made before every cover was kept (before 2026-09-26): the side not
/// picked had its covers put in the trash. Takes those out again, in their old order,
/// after the covers of the track that holds them now (a later merge may have moved them
/// on). Only covers still in the trash from that merge itself (trashed at the merge's own
/// moment), not ones someone trashed later. Returns them. Undoing the merge still works
/// as before.
pub async fn restore_trashed_covers(conn: &mut PgConnection, merge_id: Uuid) -> anyhow::Result<Vec<Uuid>> {
    let merge = sqlx::query!(
        "SELECT moved, merged_at FROM track_merges WHERE id = $1 AND undone_at IS NULL",
        merge_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let Some(moved) = merge.moved else {
        return Ok(Vec::new()); // made before merges were recorded
    };
    let record: MergeRecord = serde_json::from_value(moved)?;
    if record.covers_trashed.is_empty() {
        return Ok(Vec::new());
    }
    // In their old order (they were all one side's).
    let mut in_order: Vec<&CoverPosition> = record
        .kept_covers
        .iter()
        .chain(record.dupe_covers.iter())
        .filter(|c| record.covers_trashed.contains(&c.id))
        .collect();
    in_order.sort_by_key(|c| c.position);
    let ids: Vec<Uuid> = in_order.iter().map(|c| c.id).collect();
    // `now()` is the same for a whole transaction, so the merge's trashing has exactly
    // the merge's time.
    let trashed = sqlx::query!(
        r#"
        SELECT id, track_id AS "track_id!" FROM covers
        WHERE id = ANY($1) AND trashed_at = $2 AND track_id IS NOT NULL
        "#,
        &ids,
        merge.merged_at
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut restored = Vec::new();
    let mut holders: Vec<Uuid> = trashed.iter().map(|c| c.track_id).collect();
    holders.dedup();
    for holder in holders {
        let back: Vec<Uuid> = ids
            .iter()
            .copied()
            .filter(|id| trashed.iter().any(|c| c.id == *id && c.track_id == holder))
            .collect();
        let mut order: Vec<Uuid> = cover_positions(conn, holder).await?.into_iter().map(|c| c.id).collect();
        order.extend(&back);
        sqlx::query!(
            "UPDATE covers SET trashed_at = NULL, trashed_by = NULL WHERE id = ANY($1)",
            &back
        )
        .execute(&mut *conn)
        .await?;
        set_cover_positions(conn, &order).await?;
        restored.extend(back);
    }
    Ok(restored)
}

/// A track's covers that aren't in the trash, with their positions.
async fn cover_positions(conn: &mut PgConnection, track_id: Uuid) -> sqlx::Result<Vec<CoverPosition>> {
    sqlx::query_as!(
        CoverPosition,
        "SELECT id, position FROM covers WHERE track_id = $1 AND trashed_at IS NULL ORDER BY position, id",
        track_id
    )
    .fetch_all(conn)
    .await
}

/// Numbers covers 0, 1, 2... in the given order.
async fn set_cover_positions(conn: &mut PgConnection, ids: &[Uuid]) -> sqlx::Result<()> {
    sqlx::query!(
        r#"
        UPDATE covers c SET position = (o.n - 1)::smallint
        FROM unnest($1::uuid[]) WITH ORDINALITY AS o(id, n)
        WHERE c.id = o.id
        "#,
        ids
    )
    .execute(conn)
    .await?;
    Ok(())
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
            let seen = alias_uses_seen(conn, *alias_id).await?;
            return Err(invalid(format!(
                "alias `{name}` is still credited on {}; change those credits first",
                seen.describe()
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
        SELECT (SELECT count(DISTINCT album_id) FROM album_credits WHERE alias_id = $1)
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

/// Where an alias is credited (as `alias_uses` counts), as far as the user can see: so an
/// editor knows which credits to change before removing it.
#[derive(Debug, Serialize)]
pub struct AliasUses {
    pub tracks: Vec<AliasTrackUse>,
    pub albums: Vec<AliasAlbumUse>,
    /// Also credited on tracks or albums the user can't see (which ones isn't said).
    pub not_visible: bool,
}

#[derive(Debug, Serialize)]
pub struct AliasTrackUse {
    pub track_id: Uuid,
    pub title: String,
    /// The variant's label, if the credit is on one that isn't the track's primary.
    pub variant: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AliasAlbumUse {
    pub album_id: Uuid,
    pub title: String,
}

impl AliasUses {
    /// `track "A", track "B" (variant "Live"), album "C"`, and the hidden ones if any.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = self
            .tracks
            .iter()
            .map(|t| match &t.variant {
                Some(label) => format!("track \"{}\" (variant \"{label}\")", t.title),
                None => format!("track \"{}\"", t.title),
            })
            .chain(self.albums.iter().map(|a| format!("album \"{}\"", a.title)))
            .collect();
        if self.not_visible {
            parts.push("tracks or albums you can't see".to_string());
        }
        parts.join(", ")
    }
}

/// See `AliasUses`. Reads with the connection's own access; only whether there are more
/// is checked as the system.
pub async fn alias_uses_seen(conn: &mut PgConnection, alias_id: Uuid) -> anyhow::Result<AliasUses> {
    let pattern = serde_json::json!({ "credits": [{ "alias_id": alias_id }] });
    let tracks = sqlx::query_as!(
        AliasTrackUse,
        r#"
        SELECT t.id AS "track_id!", t.title AS "title!",
               CASE WHEN t.primary_variant_id = v.id THEN NULL ELSE v.label END AS variant
        FROM track_revisions r
        JOIN track_variants v ON v.id = r.variant_id
        JOIN tracks t ON t.id = v.track_id
        WHERE r.data @> $1
          AND r.id = (SELECT r2.id FROM track_revisions r2
                      WHERE r2.variant_id = r.variant_id ORDER BY r2.id DESC LIMIT 1)
        ORDER BY lower(t.title), t.id, v.id
        "#,
        pattern
    )
    .fetch_all(&mut *conn)
    .await?;
    let albums = sqlx::query_as!(
        AliasAlbumUse,
        r#"
        SELECT DISTINCT a.id AS "album_id!", a.title AS "title!"
        FROM album_credits c JOIN albums a ON a.id = c.album_id
        WHERE c.alias_id = $1
        ORDER BY 2, 1
        "#,
        alias_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let system = super::as_system(conn).await?;
    let all = alias_uses(conn, alias_id).await?;
    system.end(conn).await?;
    let seen = i64::try_from(tracks.len() + albums.len()).unwrap_or(i64::MAX);
    Ok(AliasUses { tracks, albums, not_visible: all > seen })
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
    //    stays behind and redirects to that one instead. One spelled like `into`'s own name
    //    moves over hidden: its credits become credits under `into`'s name (the same
    //    text), see `resolve_credits`.
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
            _ if alias.alias == into_row.name => {
                sqlx::query!(
                    "UPDATE artist_aliases SET artist_id = $2, listed = false WHERE id = $1",
                    alias.id,
                    into
                )
                .execute(&mut *conn)
                .await?;
            }
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

    // 5. Sources. One with the same platform id as one of `into`'s joins it.
    super::sources::move_all(conn, SourceOwner::Artist(from), SourceOwner::Artist(into)).await?;
    // 6. Users' plays, stars and ratings of the artist: combined into `into`'s.
    super::annotations::combine_into(
        conn,
        super::annotations::Item::Artist(from),
        super::annotations::Item::Artist(into),
    )
    .await?;

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

/// Album memberships with merged albums replaced by the album they were merged into (an
/// old revision being restored, or an ingest with an old album id). If the track is on
/// that album already, that membership (and its disc/track number) wins over the merged
/// album's.
async fn resolve_albums(conn: &mut PgConnection, albums: &[AlbumMembership]) -> sqlx::Result<Vec<AlbumMembership>> {
    let mut kept: Vec<AlbumMembership> = Vec::with_capacity(albums.len());
    let mut redirected: Vec<AlbumMembership> = Vec::new();
    for album in albums {
        let mut target = album.album_id;
        // Merges can chain; the loop limit is just a safety net.
        for _ in 0..16 {
            let into = sqlx::query_scalar!("SELECT merged_into FROM albums WHERE id = $1", target)
                .fetch_optional(&mut *conn)
                .await?
                .flatten();
            match into {
                Some(into) => target = into,
                None => break, // not merged (or unknown: reported by check_facts)
            }
        }
        if target == album.album_id {
            kept.push(album.clone());
        } else {
            redirected.push(AlbumMembership { album_id: target, ..album.clone() });
        }
    }
    for album in redirected {
        if !kept.iter().any(|a| a.album_id == album.album_id) {
            kept.push(album);
        }
    }
    Ok(kept)
}

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
        // An alias spelled like the credited artist's own name (a merge brought it over,
        // hidden): credited under the name itself.
        if let Some(alias_id) = credit.alias_id {
            let own_name = sqlx::query_scalar!(
                r#"
                SELECT al.alias = a.name AS "same!"
                FROM artist_aliases al JOIN artists a ON a.id = al.artist_id
                WHERE al.id = $1 AND al.artist_id = $2 AND NOT al.listed
                "#,
                alias_id,
                credit.artist_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            if own_name == Some(true) {
                credit.alias_id = None;
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
