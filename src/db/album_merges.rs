//! Merging albums, and undoing a merge.
//!
//! A merge records exactly what it moved (`AlbumMergeRecord`, in `album_merges.moved`).
//! The undo puts back what's still where the merge left it, and says beforehand, item by
//! item, what goes back and what stays and why (`undo_album_merge` with `apply: false`).
//! It never guesses: a track whose place on the album was changed since the merge stays
//! where it is, and the preview says so.
//!
//! Tracks move through `revisions::write_external`, so every move is a revision.

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::annotations::{self, CombinedAnnotation, Item};
use super::covers::{CoverOwner, NewCover};
use super::revisions::{self, EditRejected, Source};
use super::sources::MovedSources;
use crate::model::AlbumMembership;
use crate::model::sources::SourceOwner;

fn invalid(msg: String) -> anyhow::Error {
    EditRejected::Invalid(msg).into()
}

/// What an album merge changed (counting only what the user can see).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct MergedAlbum {
    /// The merge's id (to undo it: `undo_album_merge`).
    pub merge_id: Uuid,
    /// Tracks that moved over (each got an external revision).
    pub tracks: usize,
    /// Covers that moved over.
    pub covers: usize,
}

/// What a merge moved, stored with it for the undo.
#[derive(Debug, Serialize, Deserialize)]
struct AlbumMergeRecord {
    tracks: Vec<MovedTrack>,
    covers: Vec<MovedCover>,
    sources: MovedSources,
    /// Users' plays, stars and ratings of the merged album, combined into the other's.
    annotations: Vec<CombinedAnnotation>,
    shares: Vec<Uuid>,
    /// Albums merged into the merged album earlier, which then led to the other one too.
    redirected: Vec<Uuid>,
    /// Covers the merge added to the other album as copies: the cover the merged album
    /// showed from its tracklist, when it had none of its own. (Missing in older records.)
    #[serde(default)]
    copied_covers: Vec<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MovedTrack {
    track_id: Uuid,
    /// Its place on the merged album.
    on_from: Place,
    /// Its place on the album merged into, if it was on both before the merge (it kept
    /// that place).
    on_into_before: Option<Place>,
}

/// A track's disc and track number on an album.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Place {
    disc_number: Option<i16>,
    track_number: Option<i16>,
}

impl Place {
    fn describe(self) -> String {
        match (self.disc_number, self.track_number) {
            (Some(disc), Some(track)) => format!("disc {disc}, track {track}"),
            (None, Some(track)) => format!("track {track}"),
            (Some(disc), None) => format!("disc {disc}, no track number"),
            (None, None) => "no track number".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MovedCover {
    id: Uuid,
    /// Its position on the merged album.
    position: i16,
}

/// Merges album `from` into `into`: they're the same album (e.g. split by ingest because
/// the album artists or the date were written differently).
///
/// * Every track on `from` gets an external revision putting it on `into` instead, with
///   its disc/track number. A track already on `into` keeps its place there.
/// * `into` keeps its own title, kind, date and credits (`from`'s stay in its history).
/// * `from`'s covers move after `into`'s (all of them, also images `into` already has).
///   Sources, users' plays/stars/ratings and share links move too.
/// * `from` stays as a row redirecting to `into` (`merged_into`), so old revisions, links
///   and ids still resolve (see `resolve_albums` in `revisions`).
/// * What moved is recorded in `album_merges`, for the undo.
pub async fn merge_album(conn: &mut PgConnection, from: Uuid, into: Uuid, reason: &str) -> anyhow::Result<MergedAlbum> {
    if from == into {
        return Err(invalid("can't merge an album into itself".to_string()));
    }
    // Lock both, in id order. Through row-level security: both must be visible.
    let rows = sqlx::query!(
        "SELECT id, title, merged_into FROM albums WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &[from, into][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    let (Some(from_row), Some(into_row)) = (
        rows.iter().find(|r| r.id == from),
        rows.iter().find(|r| r.id == into),
    ) else {
        return Err(EditRejected::NotFound("album").into());
    };
    if from_row.merged_into.is_some() || into_row.merged_into.is_some() {
        return Err(invalid("that album was already merged into another".to_string()));
    }
    let reason = if reason.trim().is_empty() {
        format!("merged album \"{}\" into \"{}\"", from_row.title, into_row.title)
    } else {
        reason.to_string()
    };

    // Tracks the user can't see are on the album too, and must move with it: the rest
    // runs as the system (like an artist merge). Only visible counts are returned.
    let system = super::as_system(conn).await?;

    // 1. Where everything is before it moves.
    let places = sqlx::query!(
        r#"
        SELECT f.track_id, f.disc_number, f.track_number,
               i.track_id IS NOT NULL AS "on_into!",
               i.disc_number AS "into_disc?", i.track_number AS "into_track?"
        FROM album_tracks f
        LEFT JOIN album_tracks i ON i.track_id = f.track_id AND i.album_id = $2
        WHERE f.album_id = $1
        ORDER BY f.track_id
        "#,
        from,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    let tracks: Vec<MovedTrack> = places
        .iter()
        .map(|p| MovedTrack {
            track_id: p.track_id,
            on_from: Place { disc_number: p.disc_number, track_number: p.track_number },
            on_into_before: p.on_into.then_some(Place { disc_number: p.into_disc, track_number: p.into_track }),
        })
        .collect();
    let redirected = sqlx::query_scalar!("SELECT id FROM albums WHERE merged_into = $1 ORDER BY id", from)
        .fetch_all(&mut *conn)
        .await?;
    // The cover the album shows from its tracklist (for an album without covers of its
    // own, e.g. a single whose cover is on its track), before its tracks move.
    let tracklist_cover = sqlx::query_scalar!(
        r#"
        SELECT c.id FROM album_tracks at
        JOIN tracks t ON t.id = at.track_id
        JOIN covers c ON c.track_id = t.id AND c.trashed_at IS NULL
        WHERE at.album_id = $1
        ORDER BY at.disc_number NULLS FIRST, at.track_number NULLS LAST, t.title, c.position, c.id
        LIMIT 1
        "#,
        from
    )
    .fetch_optional(&mut *conn)
    .await?;

    // 2. The redirect (and anything merged into `from` before now leads to `into`).
    sqlx::query!("UPDATE albums SET merged_into = $2 WHERE id = $1 OR merged_into = $1", from, into)
        .execute(&mut *conn)
        .await?;

    // 3. The tracks. Rewriting a track's current facts is enough: `write_external`
    //    resolves the now merged album to `into`.
    for track in &tracks {
        let facts = revisions::current_facts(conn, track.track_id).await?;
        revisions::write_external(conn, track.track_id, None, &facts, Source::Edit, &reason).await?;
    }

    // 4. Covers, after `into`'s own (the first stays the main cover). An album without
    //    covers of its own showed one from its tracklist: `into` gets a copy of that one.
    let covers = move_covers(conn, from, into).await?;
    let mut cover_ids: Vec<Uuid> = covers.iter().map(|c| c.id).collect();
    let mut copied_covers = Vec::new();
    if covers.is_empty()
        && let Some(cover_id) = tracklist_cover
        && let Some(copy) = copy_if_new(conn, cover_id, into).await?
    {
        copied_covers.push(copy);
        cover_ids.push(copy);
    }

    // 5. Sources (one with the same platform id joins `into`'s), annotations, shares.
    let sources = super::sources::move_all(conn, SourceOwner::Album(from), SourceOwner::Album(into)).await?;
    let annotations = annotations::combine_into(conn, Item::Album(from), Item::Album(into)).await?;
    let shares = sqlx::query_scalar!("UPDATE shares SET album_id = $2 WHERE album_id = $1 RETURNING id", from, into)
        .fetch_all(&mut *conn)
        .await?;

    // 6. The record, for the undo.
    let record = AlbumMergeRecord { tracks, covers, sources, annotations, shares, redirected, copied_covers };
    let merge_id = sqlx::query_scalar!(
        r#"
        INSERT INTO album_merges (into_album_id, from_album_id, reason, moved)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
        into,
        from,
        reason,
        serde_json::to_value(&record)?
    )
    .fetch_one(&mut *conn)
    .await?;

    system.end(conn).await?;

    let track_ids: Vec<Uuid> = record.tracks.iter().map(|t| t.track_id).collect();
    let visible = sqlx::query!(
        r#"
        SELECT (SELECT count(*) FROM tracks WHERE id = ANY($1)) AS "tracks!",
               (SELECT count(*) FROM covers WHERE id = ANY($2)) AS "covers!"
        "#,
        &track_ids,
        &cover_ids
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(MergedAlbum {
        merge_id,
        tracks: visible.tracks as usize,
        covers: visible.covers as usize,
    })
}

/// Moves all of `from`'s covers (trashed ones too) after `into`'s, keeping their order.
/// Returns them with their positions on `from`, for the undo.
async fn move_covers(conn: &mut PgConnection, from: Uuid, into: Uuid) -> sqlx::Result<Vec<MovedCover>> {
    let covers: Vec<MovedCover> = sqlx::query!(
        "SELECT id, position FROM covers WHERE album_id = $1 ORDER BY position, id",
        from
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| MovedCover { id: r.id, position: r.position })
    .collect();
    let cover_ids: Vec<Uuid> = covers.iter().map(|c| c.id).collect();
    sqlx::query!(
        r#"
        UPDATE covers c
        SET album_id = $2,
            position = c.position + (SELECT COALESCE(max(k.position) + 1, 0) FROM covers k
                                     WHERE k.album_id = $2 AND k.trashed_at IS NULL)
        WHERE c.id = ANY($1)
        "#,
        &cover_ids,
        into
    )
    .execute(&mut *conn)
    .await?;
    Ok(covers)
}

/// Adds a copy of a cover (its image) to the album's covers, after the others, unless the
/// album already has that image. The cover may be in the trash (the image is still there).
pub(crate) async fn copy_if_new(conn: &mut PgConnection, cover_id: Uuid, album_id: Uuid) -> anyhow::Result<Option<Uuid>> {
    let Some(cover) = sqlx::query!(
        r#"
        SELECT blob_sha256, mime, width, height FROM covers c
        WHERE id = $1
          AND NOT EXISTS (SELECT 1 FROM covers k WHERE k.album_id = $2 AND k.blob_sha256 = c.blob_sha256
                                                   AND k.trashed_at IS NULL)
        "#,
        cover_id,
        album_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let new = NewCover {
        blob_sha256: &cover.blob_sha256,
        mime: cover.mime.as_deref(),
        width: cover.width,
        height: cover.height,
        source: "copied",
    };
    Ok(Some(super::covers::add(conn, CoverOwner::Album(album_id), &new).await?))
}

/// For merges made before an album without covers of its own passed on the cover it
/// showed from its tracklist (before 2026-09-26): finds that cover and adds a copy to the
/// album merged into (unless it has that image), recorded in the merge so an undo puts
/// the copy in the trash. The cover is looked for track by track, in the merged album's
/// order: a track since merged into another had its covers go with it (to the trash, if
/// the other track's were kept), so that merge's record says which were its own.
pub async fn copy_tracklist_cover(conn: &mut PgConnection, merge_id: Uuid) -> anyhow::Result<Option<Uuid>> {
    let merge = sqlx::query!(
        "SELECT into_album_id, moved FROM album_merges WHERE id = $1 AND undone_at IS NULL",
        merge_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let mut record: AlbumMergeRecord = serde_json::from_value(merge.moved)?;
    if !record.covers.is_empty() || !record.copied_covers.is_empty() {
        return Ok(None); // it had covers of its own, or it's done already
    }
    // The album's order: disc (none first), track number (none last), title. Tuples sort
    // field by field, and `false` sorts before `true`.
    let mut tracks = Vec::new();
    for moved in &record.tracks {
        let title = sqlx::query_scalar!("SELECT title FROM tracks WHERE id = $1", moved.track_id)
            .fetch_one(&mut *conn)
            .await?;
        tracks.push((moved.on_from.disc_number.is_some(), moved.on_from.disc_number, moved.on_from.track_number.is_none(), moved.on_from.track_number, title, moved.track_id));
    }
    tracks.sort();
    for (.., track_id) in tracks {
        let own = sqlx::query_scalar!(
            r#"
            SELECT c.id FROM covers c JOIN tracks t ON t.id = c.track_id
            WHERE c.track_id = $1 AND c.trashed_at IS NULL AND t.merged_into IS NULL
            ORDER BY c.position, c.id LIMIT 1
            "#,
            track_id
        )
        .fetch_optional(&mut *conn)
        .await?;
        let from_track_merge = sqlx::query_scalar!(
            r#"
            SELECT c.id FROM track_merges m
            CROSS JOIN jsonb_array_elements(m.moved -> 'dupe_covers') AS d(cover)
            JOIN covers c ON c.id = (d.cover ->> 'id')::uuid
            WHERE m.from_track_id = $1 AND m.undone_at IS NULL
            ORDER BY m.id DESC, (d.cover ->> 'position')::int, c.id LIMIT 1
            "#,
            track_id
        )
        .fetch_optional(&mut *conn)
        .await?;
        let Some(cover_id) = own.or(from_track_merge) else {
            continue;
        };
        let copy = copy_if_new(conn, cover_id, merge.into_album_id).await?;
        if let Some(copy) = copy {
            record.copied_covers.push(copy);
            sqlx::query!(
                "UPDATE album_merges SET moved = $2 WHERE id = $1",
                merge_id,
                serde_json::to_value(&record)?
            )
            .execute(&mut *conn)
            .await?;
        }
        return Ok(copy);
    }
    Ok(None)
}

/// For merges made while covers with an image the other album already had were left
/// behind (before 2026-09-26): moves the merged album's remaining covers after the other
/// album's, and adds them to the merge's record, so an undo takes them back too. Returns
/// how many moved. Merges are expected oldest first: a later merge of the album merged
/// into then carries these covers on, like the rest of its covers.
pub async fn move_leftover_covers(conn: &mut PgConnection, merge_id: Uuid) -> anyhow::Result<usize> {
    let merge = sqlx::query!(
        "SELECT from_album_id, into_album_id, moved FROM album_merges WHERE id = $1 AND undone_at IS NULL",
        merge_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let mut record: AlbumMergeRecord = serde_json::from_value(merge.moved)?;
    let moved = move_covers(conn, merge.from_album_id, merge.into_album_id).await?;
    let count = moved.len();
    if count > 0 {
        record.covers.extend(moved);
        sqlx::query!(
            "UPDATE album_merges SET moved = $2 WHERE id = $1",
            merge_id,
            serde_json::to_value(&record)?
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(count)
}

/// A merge into an album, for its page.
#[derive(Debug, Serialize)]
pub struct AlbumMergeInfo {
    pub id: Uuid,
    pub from_album_id: Uuid,
    /// None if the merged album isn't visible to the user (after an undo it's an album of
    /// its own again, with its own tracks deciding).
    pub from_title: Option<String>,
    pub merged_at: chrono::DateTime<chrono::Utc>,
    pub merged_by: Option<String>,
    pub reason: String,
    pub undone_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The merges into an album, newest first.
pub async fn merges_into(conn: &mut PgConnection, album_id: Uuid) -> sqlx::Result<Vec<AlbumMergeInfo>> {
    sqlx::query_as!(
        AlbumMergeInfo,
        r#"
        SELECT m.id, m.from_album_id, al.title AS "from_title?", m.merged_at,
               u.username AS "merged_by?", m.reason, m.undone_at
        FROM album_merges m
        LEFT JOIN albums al ON al.id = m.from_album_id
        LEFT JOIN users u ON u.id = m.merged_by
        WHERE m.into_album_id = $1
        ORDER BY m.id DESC
        "#,
        album_id
    )
    .fetch_all(conn)
    .await
}

/// What undoing a merge does (or did), item by item.
#[derive(Debug, Serialize)]
pub struct UndoReport {
    pub merge_id: Uuid,
    pub from_album_id: Uuid,
    pub from_title: String,
    pub into_album_id: Uuid,
    pub into_title: String,
    /// Why it can't be undone at all (empty: it can).
    pub blockers: Vec<String>,
    /// The tracks the user can see, and what happens to each.
    pub tracks: Vec<TrackUndo>,
    /// Tracks the user can't see: how many go back, how many stay.
    pub hidden_tracks_back: usize,
    pub hidden_tracks_staying: usize,
    /// Everything else that goes back, in plain words.
    pub goes_back: Vec<String>,
    /// What stays as it is, and why, in plain words.
    pub stays: Vec<String>,
    /// Whether this was done (false: a preview, nothing changed).
    pub done: bool,
}

#[derive(Debug, Serialize)]
pub struct TrackUndo {
    pub id: Uuid,
    pub title: String,
    /// Whether it goes back to the merged album.
    pub goes_back: bool,
    /// What happens to it (or why nothing does), in plain words.
    pub detail: String,
}

/// A track's decision, before titles are filtered by what the user can see.
struct TrackPlan {
    track_id: Uuid,
    title: String,
    goes_back: bool,
    detail: String,
    /// Take it off the album merged into (it wasn't on it before the merge).
    leave_into: bool,
    on_from: Place,
}

/// Undoes a merge, or (with `apply: false`) only says what undoing it would do. Only what
/// is still where the merge left it goes back; everything else stays, and the report says
/// what and why. The album merged into must be visible to the user.
///
/// Refused only if it was undone already, or the album merged into was merged into
/// another one since (undo that one first).
pub async fn undo_album_merge(conn: &mut PgConnection, merge_id: Uuid, reason: &str, apply: bool) -> anyhow::Result<UndoReport> {
    // Through row-level security: visible with the album merged into.
    let Some(merge) = sqlx::query!(
        "SELECT into_album_id, from_album_id, moved, undone_at FROM album_merges WHERE id = $1",
        merge_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Err(EditRejected::NotFound("merge").into());
    };
    let (from, into) = (merge.from_album_id, merge.into_album_id);
    let record: AlbumMergeRecord = serde_json::from_value(merge.moved)?;

    // Everything below is read (and changed) as the system: hidden tracks go back too.
    // What's returned about them is only counts.
    let system = super::as_system(conn).await?;

    let albums = sqlx::query!(
        "SELECT id, title, merged_into FROM albums WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &[from, into][..]
    )
    .fetch_all(&mut *conn)
    .await?;
    let (Some(from_row), Some(into_row)) = (albums.iter().find(|a| a.id == from), albums.iter().find(|a| a.id == into))
    else {
        return Err(EditRejected::NotFound("album").into());
    };
    let (from_title, into_title) = (from_row.title.clone(), into_row.title.clone());
    let (b, a) = (format!("\"{from_title}\""), format!("\"{into_title}\""));

    let mut blockers = Vec::new();
    if merge.undone_at.is_some() {
        blockers.push("it was undone already".to_string());
    } else if let Some(later) = into_row.merged_into {
        let later_title = sqlx::query_scalar!("SELECT title FROM albums WHERE id = $1", later)
            .fetch_one(&mut *conn)
            .await?;
        blockers.push(format!("{a} was merged into \"{later_title}\" afterwards: undo that merge first"));
    }

    // Each track: back only if it's still where the merge left it.
    let mut plans = Vec::new();
    for moved in &record.tracks {
        let track = sqlx::query!("SELECT title, merged_into FROM tracks WHERE id = $1", moved.track_id)
            .fetch_one(&mut *conn)
            .await?;
        let now = sqlx::query!(
            "SELECT disc_number, track_number FROM album_tracks WHERE album_id = $1 AND track_id = $2",
            into,
            moved.track_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| Place { disc_number: r.disc_number, track_number: r.track_number });
        let left_by_merge = moved.on_into_before.unwrap_or(moved.on_from);
        let (goes_back, detail) = if track.merged_into.is_some() {
            (false, "was merged into another track since; its albums went with that track".to_string())
        } else {
            match now {
                None => (false, format!("was taken off {a} since, so it stays where it is now")),
                Some(place) if place != left_by_merge => (
                    false,
                    format!(
                        "its place on {a} was changed since (now {}; the merge left {}), so it stays",
                        place.describe(),
                        left_by_merge.describe()
                    ),
                ),
                Some(place) => match moved.on_into_before {
                    Some(_) => (
                        true,
                        format!(
                            "back on {b} ({}); stays on {a} ({}), as it was on both before",
                            moved.on_from.describe(),
                            place.describe()
                        ),
                    ),
                    None => (true, format!("back on {b} ({}), off {a}", moved.on_from.describe())),
                },
            }
        };
        plans.push(TrackPlan {
            track_id: moved.track_id,
            title: track.title,
            goes_back,
            detail,
            leave_into: moved.on_into_before.is_none(),
            on_from: moved.on_from,
        });
    }

    // Covers: back if they're still on the album merged into (trashed ones too).
    let cover_ids: Vec<Uuid> = record.covers.iter().map(|c| c.id).collect();
    let covers_now = sqlx::query!(
        "SELECT id, trashed_at IS NOT NULL AS \"trashed!\" FROM covers WHERE id = ANY($1) AND album_id = $2",
        &cover_ids,
        into
    )
    .fetch_all(&mut *conn)
    .await?;
    let covers_back: Vec<&MovedCover> = record.covers.iter().filter(|c| covers_now.iter().any(|n| n.id == c.id)).collect();
    let covers_trashed = covers_now.iter().filter(|c| c.trashed).count();
    // Copies the merge made: to the trash (those not there already).
    let copies_to_trash = sqlx::query_scalar!(
        "SELECT id FROM covers WHERE id = ANY($1) AND trashed_at IS NULL",
        &record.copied_covers
    )
    .fetch_all(&mut *conn)
    .await?;

    // Sources: the moved ones still on the album merged into; joined ones come back as
    // they were.
    let sources_back = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM sources WHERE id = ANY($1) AND album_id = $2"#,
        &record.sources.moved,
        into
    )
    .fetch_one(&mut *conn)
    .await? as usize;
    let shares_back = sqlx::query_scalar!(
        "SELECT id FROM shares WHERE id = ANY($1) AND album_id = $2",
        &record.shares,
        into
    )
    .fetch_all(&mut *conn)
    .await?;

    // In plain words.
    let mut goes_back = vec![format!(
        "{b} is an album of its own again: it stops leading to {a}, and search, ingest and its old links find it again"
    )];
    let mut stays = vec![
        format!("{a}'s title, kind, date and credits: the merge didn't change them"),
        format!("{b}'s title, kind, date and credits: as they were (the merge didn't change them either)"),
        "anything else changed on the tracks since (metadata, status, relations): only their album changes"
            .to_string(),
        format!("tracks put on {a} after the merge"),
    ];
    if !covers_back.is_empty() {
        let trashed = if covers_trashed > 0 { format!(" ({covers_trashed} of them in the trash)") } else { String::new() };
        goes_back.push(format!("{} covers back to {b}, in their old order{trashed}", covers_back.len()));
    }
    if covers_back.len() < record.covers.len() {
        stays.push(format!(
            "{} covers that moved but aren't on {a} anymore (purged since)",
            record.covers.len() - covers_back.len()
        ));
    }
    if !copies_to_trash.is_empty() {
        goes_back.push(format!(
            "the copy of {b}'s cover (from its tracklist) that the merge added to {a}: to the trash, where it can be taken back out"
        ));
    }
    if sources_back > 0 {
        goes_back.push(format!("{sources_back} sources back to {b}"));
    }
    if sources_back < record.sources.moved.len() {
        stays.push(format!(
            "{} sources that moved but aren't {a}'s anymore",
            record.sources.moved.len() - sources_back
        ));
    }
    if !record.sources.joined.is_empty() {
        goes_back.push(format!(
            "{} sources that had joined one of {a}'s with the same platform id: {b}'s own again, with the snapshots they had",
            record.sources.joined.len()
        ));
        stays.push(format!("snapshots added since to a source both albums shared stay on {a}"));
    }
    if !record.annotations.is_empty() {
        goes_back.push(format!(
            "plays, stars and ratings of {} users: {b} still has its own; {a}'s lose {b}'s plays, and get their star or rating from before the merge back unless it was changed since",
            record.annotations.len()
        ));
        stays.push(format!("plays counted on {a} after the merge"));
    }
    if !shares_back.is_empty() {
        goes_back.push(format!("{} share links back to {b}", shares_back.len()));
    }
    if !record.redirected.is_empty() {
        goes_back.push(format!(
            "{} albums merged into {b} before lead to {b} again",
            record.redirected.len()
        ));
    }

    let done = apply && blockers.is_empty();
    if apply && !blockers.is_empty() {
        return Err(invalid(format!("this merge can't be undone: {}", blockers.join("; "))));
    }
    if done {
        let reason = if reason.trim().is_empty() {
            format!("undid merging album {b} into {a}")
        } else {
            reason.to_string()
        };
        // 1. The redirects first: `write_external` would otherwise send the tracks right
        //    back to `into`.
        sqlx::query!("UPDATE albums SET merged_into = NULL WHERE id = $1", from)
            .execute(&mut *conn)
            .await?;
        sqlx::query!(
            "UPDATE albums SET merged_into = $1 WHERE id = ANY($2) AND merged_into = $3",
            from,
            &record.redirected,
            into
        )
        .execute(&mut *conn)
        .await?;

        // 2. The tracks, each with a revision.
        for plan in plans.iter().filter(|p| p.goes_back) {
            let mut facts = revisions::current_facts(conn, plan.track_id).await?;
            if plan.leave_into {
                facts.albums.retain(|m| m.album_id != into);
            }
            facts.albums.push(AlbumMembership {
                album_id: from,
                disc_number: plan.on_from.disc_number,
                track_number: plan.on_from.track_number,
            });
            revisions::write_external(conn, plan.track_id, None, &facts, Source::Restore, &reason).await?;
        }

        // 3. Covers (their old positions), sources, annotations, shares.
        for cover in &covers_back {
            sqlx::query!(
                "UPDATE covers SET album_id = $2, position = $3 WHERE id = $1",
                cover.id,
                from,
                cover.position
            )
            .execute(&mut *conn)
            .await?;
        }
        sqlx::query!(
            "UPDATE covers SET trashed_at = now(), trashed_by = current_app_user() WHERE id = ANY($1)",
            &copies_to_trash
        )
        .execute(&mut *conn)
        .await?;
        super::sources::unmove_all(conn, &record.sources, SourceOwner::Album(from), SourceOwner::Album(into)).await?;
        annotations::split(conn, Item::Album(into), &record.annotations).await?;
        sqlx::query!("UPDATE shares SET album_id = $2 WHERE id = ANY($1)", &shares_back, from)
            .execute(&mut *conn)
            .await?;

        sqlx::query!(
            "UPDATE album_merges SET undone_at = now(), undone_by = current_app_user() WHERE id = $1",
            merge_id
        )
        .execute(&mut *conn)
        .await?;
    }

    system.end(conn).await?;

    // Only tracks the user can see are listed by title.
    let ids: Vec<Uuid> = plans.iter().map(|p| p.track_id).collect();
    let visible = sqlx::query_scalar!("SELECT id FROM tracks WHERE id = ANY($1)", &ids)
        .fetch_all(&mut *conn)
        .await?;
    let mut report = UndoReport {
        merge_id,
        from_album_id: from,
        from_title,
        into_album_id: into,
        into_title,
        blockers,
        tracks: Vec::new(),
        hidden_tracks_back: 0,
        hidden_tracks_staying: 0,
        goes_back,
        stays,
        done,
    };
    for plan in plans {
        if visible.contains(&plan.track_id) {
            report.tracks.push(TrackUndo {
                id: plan.track_id,
                title: plan.title,
                goes_back: plan.goes_back,
                detail: plan.detail,
            });
        } else if plan.goes_back {
            report.hidden_tracks_back += 1;
        } else {
            report.hidden_tracks_staying += 1;
        }
    }
    Ok(report)
}
