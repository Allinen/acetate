//! Editing many tracks in one go, as the desktop editor does: each track's whole new
//! metadata (the version edited) and facts, worked out by the client from the changes typed
//! there, written through `revisions` like any edit. What this adds:
//!
//! * **All of a track, or none of it.** Each track is written in a savepoint: a track that
//!   fails (stale, invalid) leaves nothing behind, and the others still go.
//! * **New artists and albums** the edit credits or puts tracks on are made first, once,
//!   under stand-in ids the client chose; the tracks' data refers to them by those.
//! * **The album along.** An album the track was the only track on (as far as the user can
//!   see) gets the fields this edit changes on the track: title, release date, and the
//!   performers (primary and featured, in order, under the same names) as its credits.
//!   Only fields the edit changes: a status change on 500 tracks doesn't rewrite their
//!   singles. Only albums it was already alone on: a track put on a new album doesn't
//!   rename it. Counting what the user can see means an album whose other tracks are all
//!   hidden from them looks like it has one track; that's the price of not telling them
//!   hidden tracks are there. (Their tracks themselves are never touched.)
//! * **The flags after it**: each track's open review flags once the checks have run
//!   (they're deferred to the commit; see migration 0030), so a preview says exactly what
//!   would still be missing.
//!
//! A preview is the same run, rolled back by the caller.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use super::revisions::{self, EditRejected, Source, Written};
use crate::model::{AlbumData, ArtistData, Credit, TrackFacts, TrackMetadata, diff_relations};

/// At most this many tracks per request.
pub const MAX_TRACKS: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Edit {
    #[serde(default)]
    pub reason: String,
    /// Artists to make first. Their `id`s are stand-ins, used in the tracks' credits.
    #[serde(default)]
    pub new_artists: Vec<NewArtist>,
    /// Albums to make first (after the artists: their credits may use those). Stand-in ids
    /// too, used in the tracks' albums.
    #[serde(default)]
    pub new_albums: Vec<NewAlbum>,
    pub tracks: Vec<TrackEdit>,
}

#[derive(Debug, Deserialize)]
pub struct NewArtist {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct NewAlbum {
    pub id: Uuid,
    #[serde(flatten)]
    pub data: AlbumData,
}

#[derive(Debug, Deserialize)]
pub struct TrackEdit {
    pub track_id: Uuid,
    /// The version's new metadata (None: left as it is).
    #[serde(default)]
    pub meta: Option<MetaEdit>,
    /// The track's new facts (None: left as they are).
    #[serde(default)]
    pub facts: Option<FactsEdit>,
    /// Change the albums it's alone on along with it (see the module's notes).
    #[serde(default = "yes")]
    pub along: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct MetaEdit {
    pub variant_id: Uuid,
    pub base_revision_id: Uuid,
    pub data: TrackMetadata,
}

#[derive(Debug, Deserialize)]
pub struct FactsEdit {
    pub base_revision_id: Uuid,
    #[serde(flatten)]
    pub data: TrackFacts,
}

/// What became of an edit (or would, in a preview).
#[derive(Debug, Serialize)]
pub struct Edited {
    /// Stand-in id → the artist made.
    pub artists: HashMap<Uuid, Uuid>,
    /// Stand-in id → the album made.
    pub albums: HashMap<Uuid, Uuid>,
    pub tracks: Vec<TrackEdited>,
}

#[derive(Debug, Serialize)]
pub struct TrackEdited {
    pub track_id: Uuid,
    /// Why nothing of this track was written (None: it was, or would be).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Written>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facts: Option<Written>,
    /// Albums changed along with it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub along: Vec<Along>,
    /// Its open review flags afterwards (their kinds, in the server's order).
    pub flags: Vec<String>,
}

/// Why a track wasn't written: the error's `code` and `message`, as the API says them.
#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Along {
    pub album_id: Uuid,
    pub before: AlbumData,
    pub after: AlbumData,
}

/// Runs the edit on `conn` (a transaction acting as the user). `describe` turns a track's
/// error into what the client is told. `may_edit_relations`: the user has
/// `edit_relations` (else a track whose relations change fails).
///
/// Errors only for what stops everything (making the new artists or albums); a track's own
/// error is in its result.
pub async fn edit(
    conn: &mut PgConnection,
    edit: &Edit,
    may_edit_relations: bool,
    describe: impl Fn(anyhow::Error) -> Failure,
) -> anyhow::Result<Edited> {
    let reason = edit.reason.trim();
    let mut artists = HashMap::new();
    for new in &edit.new_artists {
        let data = ArtistData { name: new.name.trim().to_string(), ..Default::default() };
        artists.insert(new.id, revisions::create_artist(conn, &data, Source::Edit, reason).await?);
    }
    let mut albums = HashMap::new();
    for new in &edit.new_albums {
        let data = AlbumData { credits: real_credits(&new.data.credits, &artists), ..new.data.clone() };
        albums.insert(new.id, revisions::create_album(conn, &data, Source::Edit, reason).await?);
    }

    let mut tracks = Vec::new();
    for track in &edit.tracks {
        let mut savepoint = conn.begin().await?;
        let result = edit_track(&mut savepoint, track, &artists, &albums, reason, may_edit_relations).await;
        let done = match result {
            Ok(done) => {
                savepoint.commit().await?;
                done
            }
            Err(err) => {
                savepoint.rollback().await?;
                TrackEdited { track_id: track.track_id, error: Some(describe(err)), meta: None, facts: None, along: Vec::new(), flags: Vec::new() }
            }
        };
        tracks.push(done);
    }

    // The checks run now instead of at the commit, so their flags can be read.
    sqlx::query("SET CONSTRAINTS ALL IMMEDIATE").execute(&mut *conn).await?;
    let ids: Vec<Uuid> = tracks.iter().map(|t| t.track_id).collect();
    let open = sqlx::query!(
        r#"SELECT f.track_id, f.kind FROM review_flags f JOIN review_kinds k ON k.kind = f.kind
           WHERE f.track_id = ANY($1) AND f.resolved_at IS NULL
           ORDER BY k.position, f.kind"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    for t in &mut tracks {
        for flag in open.iter().filter(|f| f.track_id == t.track_id) {
            if !t.flags.contains(&flag.kind) {
                t.flags.push(flag.kind.clone());
            }
        }
    }
    Ok(Edited { artists, albums, tracks })
}

async fn edit_track(
    conn: &mut PgConnection,
    edit: &TrackEdit,
    artists: &HashMap<Uuid, Uuid>,
    albums: &HashMap<Uuid, Uuid>,
    reason: &str,
    may_edit_relations: bool,
) -> anyhow::Result<TrackEdited> {
    let track = edit.track_id;
    let shown = sqlx::query_scalar!("SELECT primary_variant_id FROM tracks WHERE id = $1 AND merged_into IS NULL", track)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("track"))?;

    // Before anything changes: the albums it's alone on, and the shown version's metadata.
    let alone_on = if edit.along { alone_on(conn, track).await? } else { Vec::new() };
    let before = match (&edit.meta, shown) {
        (Some(m), Some(shown)) if m.variant_id == shown && !alone_on.is_empty() => latest_meta(conn, m.variant_id).await?,
        _ => None,
    };

    let mut done = TrackEdited { track_id: track, error: None, meta: None, facts: None, along: Vec::new(), flags: Vec::new() };
    if let Some(m) = &edit.meta {
        let data = TrackMetadata { credits: real_credits(&m.data.credits, artists), ..m.data.clone() };
        done.meta = Some(revisions::write_internal(conn, track, m.variant_id, Some(m.base_revision_id), &data, Source::Edit, reason, false).await?);
    }
    if let Some(f) = &edit.facts {
        let mut data = f.data.clone();
        for membership in &mut data.albums {
            if let Some(real) = albums.get(&membership.album_id) {
                membership.album_id = *real;
            }
        }
        if !may_edit_relations {
            let current = revisions::current_relations(conn, track).await?;
            let (added, removed) = diff_relations(&current, &data.relations);
            if !added.is_empty() || !removed.is_empty() {
                return Err(EditRejected::Invalid("changing related tracks needs the `edit_relations` permission".into()).into());
            }
        }
        done.facts = Some(revisions::write_external(conn, track, Some(f.base_revision_id), &data, Source::Edit, reason).await?);
    }

    // The albums along: what changed on the track, if it's still on them.
    if let (Some(before), Some(m)) = (before, &edit.meta) {
        let after = TrackMetadata { credits: real_credits(&m.data.credits, artists), ..m.data.clone() };
        let still_on: Vec<Uuid> = sqlx::query_scalar!("SELECT album_id FROM album_tracks WHERE track_id = $1", track)
            .fetch_all(&mut *conn)
            .await?;
        for album in alone_on.into_iter().filter(|a| still_on.contains(a)) {
            let Some((base, old)) = latest_album(conn, album).await? else { continue };
            let new = along(&old, &before, &after);
            if new != old {
                revisions::write_album(conn, album, Some(base), &new, Source::Edit, &along_reason(reason)).await?;
                done.along.push(Along { album_id: album, before: old, after: new });
            }
        }
    }
    Ok(done)
}

/// The album with the track's changes (from `before` to `after`) along: its title, release
/// date and performers, where the track's changed.
pub fn along(album: &AlbumData, before: &TrackMetadata, after: &TrackMetadata) -> AlbumData {
    let mut new = album.clone();
    if before.title != after.title {
        new.title = after.title.clone();
    }
    if before.release_date != after.release_date {
        new.release_date = after.release_date.clone();
    }
    if performers(&before.credits) != performers(&after.credits) {
        new.credits = performers(&after.credits);
    }
    new
}

/// Primary and featured credits, in order: what an album credits.
fn performers(credits: &[Credit]) -> Vec<Credit> {
    credits.iter().filter(|c| c.role == "primary" || c.role == "featured").cloned().collect()
}

fn along_reason(reason: &str) -> String {
    if reason.is_empty() { "changed along with its only track".to_string() } else { reason.to_string() }
}

/// Credits with the new artists' real ids for their stand-ins.
fn real_credits(credits: &[Credit], artists: &HashMap<Uuid, Uuid>) -> Vec<Credit> {
    credits
        .iter()
        .map(|c| Credit { artist_id: artists.get(&c.artist_id).copied().unwrap_or(c.artist_id), ..c.clone() })
        .collect()
}

/// The albums (not merged away) this track is the only track on, as far as the
/// transaction's user can see.
async fn alone_on(conn: &mut PgConnection, track: Uuid) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar!(
        r#"SELECT at.album_id AS "album_id!" FROM album_tracks at
           JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL
           WHERE at.track_id = $1
             AND NOT EXISTS (SELECT 1 FROM album_tracks o WHERE o.album_id = at.album_id AND o.track_id <> $1)"#,
        track
    )
    .fetch_all(&mut *conn)
    .await
}

async fn latest_meta(conn: &mut PgConnection, variant: Uuid) -> anyhow::Result<Option<TrackMetadata>> {
    let data = sqlx::query_scalar!("SELECT data FROM track_revisions WHERE variant_id = $1 ORDER BY id DESC LIMIT 1", variant)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(data.map(serde_json::from_value).transpose()?)
}

async fn latest_album(conn: &mut PgConnection, album: Uuid) -> anyhow::Result<Option<(Uuid, AlbumData)>> {
    let row = sqlx::query!("SELECT id, data FROM album_revisions WHERE album_id = $1 ORDER BY id DESC LIMIT 1", album)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(match row {
        Some(r) => Some((r.id, serde_json::from_value(r.data)?)),
        None => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(n: u128, role: &str) -> Credit {
        Credit { artist_id: Uuid::from_u128(n), role: role.into(), alias_id: None }
    }

    /// A body as the desktop editor sends it (`ui/edit/editor/engine.rs`, `body`).
    #[test]
    fn reads_what_the_editor_sends() {
        let body = serde_json::json!({
            "tracks": [{
                "track_id": Uuid::from_u128(1), "along": false,
                "meta": { "variant_id": Uuid::from_u128(2), "base_revision_id": Uuid::from_u128(3),
                          "data": { "title": "T", "credits": [{ "artist_id": Uuid::from_u128(9), "role": "featured" }] } },
                "facts": { "status": "demo", "albums": [{ "album_id": Uuid::from_u128(8), "disc_number": 1 }], "base_revision_id": Uuid::from_u128(4) },
            }, { "track_id": Uuid::from_u128(5), "along": true }],
            "new_artists": [{ "id": Uuid::from_u128(9), "name": "New" }],
            "new_albums": [{ "id": Uuid::from_u128(8), "title": "EP", "kind": "ep", "release_date": null,
                             "credits": [{ "artist_id": Uuid::from_u128(9), "role": "primary", "alias_id": null }] }],
            "reason": "why",
        });
        let edit: Edit = serde_json::from_value(body).unwrap();
        let t = &edit.tracks[0];
        assert!(!t.along);
        assert_eq!(t.meta.as_ref().unwrap().data.credits[0].role, "featured");
        let facts = t.facts.as_ref().unwrap();
        assert_eq!((facts.base_revision_id, facts.data.status.as_str()), (Uuid::from_u128(4), "demo"));
        assert_eq!(facts.data.albums[0].disc_number, Some(1));
        assert!(edit.tracks[1].meta.is_none() && edit.tracks[1].facts.is_none());
        assert_eq!((edit.new_albums[0].data.kind.as_str(), edit.new_albums[0].data.credits.len()), ("ep", 1));
        assert_eq!(edit.new_artists[0].name, "New");
    }

    #[test]
    fn only_what_changed_goes_along() {
        let album = AlbumData { title: "Old single".into(), kind: "ep".into(), release_date: Some("2019".into()), credits: vec![credit(1, "primary")] };
        let before = TrackMetadata { title: "Song".into(), release_date: Some("2020".into()), credits: vec![credit(1, "primary")], ..Default::default() };

        // Only the title changed: only the title goes (the album's own date stays).
        let after = TrackMetadata { title: "Song (Remix)".into(), ..before.clone() };
        let new = along(&album, &before, &after);
        assert_eq!((new.title.as_str(), new.release_date.as_deref(), new.kind.as_str()), ("Song (Remix)", Some("2019"), "ep"));

        // A producer added: the performers are the same, nothing goes.
        let after = TrackMetadata { credits: vec![credit(1, "primary"), credit(2, "producer")], ..before.clone() };
        assert_eq!(along(&album, &before, &after), album);

        // A featured artist added: the album's credits become the performers, in order.
        let after = TrackMetadata { credits: vec![credit(1, "primary"), credit(2, "producer"), credit(3, "featured")], ..before.clone() };
        assert_eq!(along(&album, &before, &after).credits, vec![credit(1, "primary"), credit(3, "featured")]);
    }
}
