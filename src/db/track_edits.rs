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
//!   hidden tracks are there. (Their tracks themselves are never touched.) `sync` makes
//!   them match the track whatever the edit changes (nothing, even): its title, date and
//!   performers, as the track's shown version has them once the edit is written.
//! * **Covers and sources** with the rest of the track, in the same savepoint: its covers as
//!   they're to be (kept, copied from other covers, or images staged before, see
//!   `POST /api/covers/staged`; the ones left out go to the trash), sources added,
//!   corrected (a new snapshot) or put in the trash, and which one its file came from.
//!   The covers are stale like a revision: if they've changed since the client loaded
//!   them, nothing of the track is written. New covers go along too, to the albums it's
//!   alone on that haven't got that image (the track's new main cover becomes theirs).
//! * **A single of its own** (`single`), made last: from the track as the edit leaves it
//!   (title, date, primary artists, covers), whatever albums it's on (the client warns
//!   about those).
//! * **Groupings**: the track put in some (new ones made first, like albums, under
//!   stand-ins) and taken out of others; a single it's alone on goes along, as always
//!   (`db::groupings::add`), the new one too.
//! * **Versions**: the metadata goes to the version edited, or to a new one (`new_variant`,
//!   e.g. what was imported from SoundCloud: its first revision is the data, nothing
//!   copied), and the version edited can be renamed with it (`label`).
//! * **Accounts of artists** (`artist_sources`): sources added to artists, new ones too,
//!   before the tracks (e.g. the SoundCloud account of the artist an import credits).
//! * **Flags by hand**: a track flagged ("Fix later", or a kind people made: new ones are
//!   made first, `new_review_kinds`, under their names) or its open flags of a kind
//!   settled, in its savepoint.
//! * **The flags after it**: each track's open review flags once the checks have run
//!   (they're deferred to the commit; see migration 0030), so a preview says exactly what
//!   would still be missing.
//!
//! A preview is the same run, rolled back by the caller.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use super::covers::{self, CoverOwner};
use super::revisions::{self, EditRejected, Source, Written};
use crate::model::sources::{self as source_shapes, Kind, NewSource, SourceOwner};
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
    /// Groupings to make first, under stand-in ids too, used in the tracks' `groups`.
    #[serde(default)]
    pub new_groupings: Vec<NewGrouping>,
    /// Sources added to artists (or new ones' stand-ins), after those are made. Needs
    /// `edit_sources`.
    #[serde(default)]
    pub artist_sources: Vec<ArtistSource>,
    /// Kinds of flag to make first (their names): the tracks' `flags.add` name them so.
    #[serde(default)]
    pub new_review_kinds: Vec<String>,
    pub tracks: Vec<TrackEdit>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistSource {
    pub artist_id: Uuid,
    pub platform: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct NewGrouping {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
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
    /// Make the albums it's alone on match it, changed by this edit or not.
    #[serde(default)]
    pub sync: bool,
    /// Its covers as they're to be (None: left as they are).
    #[serde(default)]
    pub covers: Option<CoversEdit>,
    /// Its sources' changes, and where its file came from (None: none).
    #[serde(default)]
    pub sources: Option<SourcesEdit>,
    /// A single of its own, made last.
    #[serde(default)]
    pub single: bool,
    /// Groupings it goes in and leaves (None: none).
    #[serde(default)]
    pub groups: Option<GroupsEdit>,
    /// Flags raised and settled by hand (None: none).
    #[serde(default)]
    pub flags: Option<FlagsEdit>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FlagsEdit {
    /// Kinds it's flagged with ("fix_later", one people made, or a new one's name).
    #[serde(default)]
    pub add: Vec<String>,
    /// Kinds whose open flags on it are settled (any kind).
    #[serde(default)]
    pub settle: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct GroupsEdit {
    /// Groupings it goes in (or new ones' stand-ins).
    #[serde(default)]
    pub add: Vec<Uuid>,
    #[serde(default)]
    pub remove: Vec<Uuid>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct CoversEdit {
    /// Its covers as the client had them, in order: if they've changed since, the track is
    /// stale.
    pub base: Vec<Uuid>,
    /// Its covers to be, in order (the first is the main one). One it has that isn't here
    /// goes to the trash.
    pub covers: Vec<CoverItem>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CoverItem {
    /// One it has.
    Kept { id: Uuid },
    /// A copy of another cover the user can see (a track's or an album's): the same image.
    Copy { copy_of: Uuid },
    /// An image the user staged (by its hash).
    Staged { staged: String },
}

#[derive(Debug, Default, Deserialize)]
pub struct SourcesEdit {
    #[serde(default)]
    pub add: Vec<AddSource>,
    /// New snapshots of sources it has.
    #[serde(default)]
    pub correct: Vec<CorrectSource>,
    /// Sources of its to put in the trash.
    #[serde(default)]
    pub trash: Vec<Uuid>,
    /// Its file came from this source: one of its own, or one added here (its stand-in).
    #[serde(default)]
    pub origin: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct AddSource {
    /// A stand-in id, for `origin`.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub platform: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CorrectSource {
    pub source_id: Uuid,
    pub platform: String,
    pub data: serde_json::Value,
}

/// What the user may change besides metadata and facts.
#[derive(Debug, Clone, Copy)]
pub struct May {
    /// `edit_relations`: related tracks.
    pub relations: bool,
    /// `edit_sources`: sources, and where a file came from.
    pub sources: bool,
}

/// The metadata of the version edited (`variant_id` and `base_revision_id`), or of a new
/// one (`new_variant`, its name).
#[derive(Debug, Deserialize)]
pub struct MetaEdit {
    #[serde(default)]
    pub variant_id: Option<Uuid>,
    #[serde(default)]
    pub base_revision_id: Option<Uuid>,
    #[serde(default)]
    pub new_variant: Option<String>,
    /// The version edited, renamed.
    #[serde(default)]
    pub label: Option<String>,
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
    /// Stand-in id → the grouping made.
    pub groupings: HashMap<Uuid, Uuid>,
    /// Name → the kind of flag made.
    pub review_kinds: HashMap<String, String>,
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
    /// The single made for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub single: Option<Uuid>,
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
    /// How many of the track's new covers it got.
    pub covers: usize,
}

/// Runs the edit on `conn` (a transaction acting as the user). `describe` turns a track's
/// error into what the client is told. `may`: what the user may change besides metadata
/// and facts (a track changing something else it may not fails).
///
/// Errors only for what stops everything (making the new artists or albums); a track's own
/// error is in its result.
pub async fn edit(
    conn: &mut PgConnection,
    edit: &Edit,
    may: May,
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
    let mut groupings = HashMap::new();
    for new in &edit.new_groupings {
        groupings.insert(new.id, super::groupings::create(conn, new.kind.trim(), new.name.trim(), "", reason).await?);
    }
    let mut review_kinds = HashMap::new();
    for name in &edit.new_review_kinds {
        review_kinds.insert(name.clone(), super::review::make_kind(conn, name).await?);
    }
    if !edit.artist_sources.is_empty() && !may.sources {
        return Err(EditRejected::Invalid("giving artists sources needs the `edit_sources` permission".into()).into());
    }
    for added in &edit.artist_sources {
        let artist = artists.get(&added.artist_id).copied().unwrap_or(added.artist_id);
        let source = NewSource { platform: added.platform.clone(), source_id: None, captured_at: None, data: added.data.clone() };
        let source = source_shapes::validate(Kind::Artist, source)?;
        if !super::sources::owner_exists(conn, SourceOwner::Artist(artist)).await? {
            return Err(EditRejected::NotFound("artist").into());
        }
        super::sources::add_snapshot(conn, SourceOwner::Artist(artist), &source).await?;
    }

    let mut tracks = Vec::new();
    for track in &edit.tracks {
        let mut savepoint = conn.begin().await?;
        let made = Made { artists: &artists, albums: &albums, groupings: &groupings, review_kinds: &review_kinds };
        let result = edit_track(&mut savepoint, track, &made, reason, may).await;
        let done = match result {
            Ok(done) => {
                savepoint.commit().await?;
                done
            }
            Err(err) => {
                savepoint.rollback().await?;
                TrackEdited { track_id: track.track_id, error: Some(describe(err)), meta: None, facts: None, along: Vec::new(), single: None, flags: Vec::new() }
            }
        };
        tracks.push(done);
    }

    read_flags(conn, &mut tracks).await?;
    Ok(Edited { artists, albums, groupings, review_kinds, tracks })
}

/// Fills in the tracks' `flags`: their open review flags, the checks run first.
pub async fn read_flags(conn: &mut PgConnection, tracks: &mut [TrackEdited]) -> anyhow::Result<()> {
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
    for t in tracks {
        t.flags.clear();
        for flag in open.iter().filter(|f| f.track_id == t.track_id) {
            if !t.flags.contains(&flag.kind) {
                t.flags.push(flag.kind.clone());
            }
        }
    }
    Ok(())
}

/// What the edit made first: stand-in id → the real one.
struct Made<'a> {
    artists: &'a HashMap<Uuid, Uuid>,
    albums: &'a HashMap<Uuid, Uuid>,
    groupings: &'a HashMap<Uuid, Uuid>,
    review_kinds: &'a HashMap<String, String>,
}

async fn edit_track(conn: &mut PgConnection, edit: &TrackEdit, made: &Made<'_>, reason: &str, may: May) -> anyhow::Result<TrackEdited> {
    let (artists, albums) = (made.artists, made.albums);
    let track = edit.track_id;
    let shown = sqlx::query_scalar!("SELECT primary_variant_id FROM tracks WHERE id = $1 AND merged_into IS NULL", track)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("track"))?;

    // Before anything changes: the albums it's alone on, and the shown version's metadata.
    let alone_on = if edit.along { alone_on(conn, track).await? } else { Vec::new() };
    let before = match (&edit.meta, shown) {
        (Some(m), Some(shown)) if m.variant_id == Some(shown) && !alone_on.is_empty() => latest_meta(conn, shown).await?,
        _ => None,
    };

    let mut done = TrackEdited { track_id: track, error: None, meta: None, facts: None, along: Vec::new(), single: None, flags: Vec::new() };
    if let Some(m) = &edit.meta {
        let data = TrackMetadata { credits: real_credits(&m.data.credits, artists), ..m.data.clone() };
        done.meta = Some(match (&m.new_variant, m.variant_id, m.base_revision_id) {
            (Some(label), None, None) => revisions::add_variant(conn, track, label, &data, Source::Edit, reason).await?.1,
            (None, Some(variant), Some(base)) => {
                let written = revisions::write_internal(conn, track, variant, Some(base), &data, Source::Edit, reason, false).await?;
                if let Some(label) = &m.label {
                    revisions::rename_variant(conn, track, variant, label.trim()).await?;
                }
                written
            }
            _ => return Err(EditRejected::Invalid("the metadata is for a version (`variant_id` and `base_revision_id`) or a new one (`new_variant`)".into()).into()),
        });
    }
    if let Some(f) = &edit.facts {
        let mut data = f.data.clone();
        for membership in &mut data.albums {
            if let Some(real) = albums.get(&membership.album_id) {
                membership.album_id = *real;
            }
        }
        if !may.relations {
            let current = revisions::current_relations(conn, track).await?;
            let (added, removed) = diff_relations(&current, &data.relations);
            if !added.is_empty() || !removed.is_empty() {
                return Err(EditRejected::Invalid("changing related tracks needs the `edit_relations` permission".into()).into());
            }
        }
        done.facts = Some(revisions::write_external(conn, track, Some(f.base_revision_id), &data, Source::Edit, reason).await?);
    }

    let new_covers = match &edit.covers {
        Some(c) => write_covers(conn, track, c).await?,
        None => Vec::new(),
    };
    if let Some(s) = &edit.sources {
        if !may.sources {
            return Err(EditRejected::Invalid("changing sources needs the `edit_sources` permission".into()).into());
        }
        write_sources(conn, track, s, reason).await?;
    }

    // The albums along, if it's still on them: what changed on the track, or (`sync`) all
    // of it, as its shown version is now; and its new covers.
    let synced = match (edit.sync && edit.along && !alone_on.is_empty(), shown) {
        (true, Some(shown)) => latest_meta(conn, shown).await?,
        _ => None,
    };
    let changed = match (before, &edit.meta) {
        (Some(before), Some(m)) => Some((before, TrackMetadata { credits: real_credits(&m.data.credits, artists), ..m.data.clone() })),
        _ => None,
    };
    if !alone_on.is_empty() && (synced.is_some() || changed.is_some() || !new_covers.is_empty()) {
        let still_on: Vec<Uuid> = sqlx::query_scalar!("SELECT album_id FROM album_tracks WHERE track_id = $1", track)
            .fetch_all(&mut *conn)
            .await?;
        for album in alone_on.into_iter().filter(|a| still_on.contains(a)) {
            let Some((base, old)) = latest_album(conn, album).await? else { continue };
            let new = match (&synced, &changed) {
                (Some(now), _) => matching(&old, now),
                (None, Some((before, after))) => along(&old, before, after),
                (None, None) => old.clone(),
            };
            if new != old {
                revisions::write_album(conn, album, Some(base), &new, Source::Edit, &along_reason(reason)).await?;
            }
            let covers = covers_along(conn, album, &new_covers).await?;
            if new != old || covers > 0 {
                done.along.push(Along { album_id: album, before: old, after: new, covers });
            }
        }
    }
    // Last: a single of its own (as the track is now), then its groupings (a single it's
    // alone on, the new one too, goes along).
    if edit.single {
        let shown = sqlx::query_scalar!("SELECT primary_variant_id FROM tracks WHERE id = $1", track).fetch_one(&mut *conn).await?;
        let single_reason = if reason.is_empty() { "a single of its own" } else { reason };
        done.single = super::review_bulk::make_single(conn, track, shown, single_reason)
            .await?
            .ok_or_else(|| EditRejected::Invalid("it hasn't the revisions a single is made from".into()))
            .map(Some)?;
    }
    if let Some(groups) = &edit.groups {
        let real = |id: &Uuid| made.groupings.get(id).copied().unwrap_or(*id);
        for grouping in &groups.add {
            super::groupings::add(conn, real(grouping), &[track], &[], reason).await?;
        }
        for grouping in &groups.remove {
            super::groupings::remove(conn, real(grouping), &[track], &[], reason).await?;
        }
    }
    if let Some(flags) = &edit.flags {
        for kind in &flags.add {
            let kind = made.review_kinds.get(kind).unwrap_or(kind);
            super::review::flag_by_hand(conn, track, kind).await?;
        }
        let settled = if reason.is_empty() { "settled in the track editor" } else { reason };
        for kind in &flags.settle {
            super::review::settle_kind(conn, track, kind, settled).await?;
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

/// The album matching the track: its title, release date and performers.
pub fn matching(album: &AlbumData, track: &TrackMetadata) -> AlbumData {
    AlbumData { title: track.title.clone(), release_date: track.release_date.clone(), credits: performers(&track.credits), ..album.clone() }
}

/// Primary and featured credits, in order: what an album credits.
fn performers(credits: &[Credit]) -> Vec<Credit> {
    credits.iter().filter(|c| c.role == "primary" || c.role == "featured").cloned().collect()
}

/// The track's covers made as `edit` says: the ones left out to the trash, the new ones
/// made, all in the order given. Returns the new ones: (id, whether it's the main one).
async fn write_covers(conn: &mut PgConnection, track: Uuid, edit: &CoversEdit) -> anyhow::Result<Vec<(Uuid, bool)>> {
    let current = sqlx::query_scalar!("SELECT id FROM covers WHERE track_id = $1 AND trashed_at IS NULL ORDER BY position, id FOR UPDATE", track)
        .fetch_all(&mut *conn)
        .await?;
    if current != edit.base {
        return Err(EditRejected::Stale { latest: None }.into());
    }
    let mut kept = Vec::new();
    for item in &edit.covers {
        if let CoverItem::Kept { id } = item {
            if !current.contains(id) || kept.contains(id) {
                return Err(EditRejected::Invalid("each cover kept must be one of its own, listed once".into()).into());
            }
            kept.push(*id);
        }
    }
    for gone in current.iter().filter(|c| !kept.contains(c)) {
        covers::trash(conn, *gone).await?;
    }
    let owner = CoverOwner::Track(track);
    let (mut order, mut new) = (Vec::new(), Vec::new());
    for (n, item) in edit.covers.iter().enumerate() {
        let id = match item {
            CoverItem::Kept { id } => *id,
            CoverItem::Copy { copy_of } => covers::copy(conn, owner, *copy_of).await?,
            CoverItem::Staged { staged } => covers::add_staged(conn, owner, staged).await?,
        };
        if !matches!(item, CoverItem::Kept { .. }) {
            new.push((id, n == 0));
        }
        order.push(id);
    }
    if order != current {
        covers::set_order(conn, owner, &order).await?;
    }
    Ok(new)
}

/// The track's new covers on an album it's alone on, where it hasn't got that image (the
/// track's new main cover becomes the album's too). How many it got.
async fn covers_along(conn: &mut PgConnection, album: Uuid, new: &[(Uuid, bool)]) -> anyhow::Result<usize> {
    let mut added = 0;
    for (cover, main) in new {
        let has_it = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM covers c JOIN covers n ON n.id = $2 AND n.blob_sha256 = c.blob_sha256
                              WHERE c.album_id = $1 AND c.trashed_at IS NULL) AS "e!""#,
            album,
            cover
        )
        .fetch_one(&mut *conn)
        .await?;
        if has_it {
            continue;
        }
        let copy = covers::copy(conn, CoverOwner::Album(album), *cover).await?;
        added += 1;
        if *main {
            let mut order = sqlx::query_scalar!("SELECT id FROM covers WHERE album_id = $1 AND trashed_at IS NULL ORDER BY position, id", album)
                .fetch_all(&mut *conn)
                .await?;
            order.retain(|c| *c != copy);
            order.insert(0, copy);
            covers::set_order(conn, CoverOwner::Album(album), &order).await?;
        }
    }
    Ok(added)
}

/// The track's sources changed as `edit` says: corrected, added, where its file came from,
/// then the ones to go in the trash (after: its file may have come from one of them).
async fn write_sources(conn: &mut PgConnection, track: Uuid, edit: &SourcesEdit, reason: &str) -> anyhow::Result<()> {
    let owner = SourceOwner::Track(track);
    let source = |platform: &str, source_id: Option<Uuid>, data: &serde_json::Value| {
        source_shapes::validate(Kind::Track, NewSource { platform: platform.to_string(), source_id, captured_at: None, data: data.clone() })
    };
    let own = async |conn: &mut PgConnection, id: Uuid| -> anyhow::Result<()> {
        let own = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM sources WHERE id = $1 AND track_id = $2) AS "e!""#, id, track)
            .fetch_one(&mut *conn)
            .await?;
        if own { Ok(()) } else { Err(EditRejected::Invalid("that source isn't one of its own".into()).into()) }
    };
    for c in &edit.correct {
        own(conn, c.source_id).await?;
        super::sources::add_snapshot(conn, owner, &source(&c.platform, Some(c.source_id), &c.data)?).await?;
    }
    let mut made = HashMap::new();
    for a in &edit.add {
        let added = super::sources::add_snapshot(conn, owner, &source(&a.platform, None, &a.data)?).await?;
        if let Some(id) = a.id {
            made.insert(id, added.source_id);
        }
    }
    if let Some(origin) = edit.origin {
        let origin = made.get(&origin).copied().unwrap_or(origin);
        let file = sqlx::query_scalar!("SELECT file_id FROM tracks WHERE id = $1", track)
            .fetch_one(&mut *conn)
            .await?
            .ok_or_else(|| EditRejected::Invalid("it has no file to have come from somewhere".into()))?;
        super::sources::set_file_origin(conn, file, super::sources::OriginTarget::Source(origin), reason).await?;
    }
    // (Not one a source added here turned out to be: the server matched it to one it had.)
    for gone in edit.trash.iter().filter(|g| !made.values().any(|m| m == *g)) {
        own(conn, *gone).await?;
        super::sources::trash(conn, *gone).await?;
    }
    Ok(())
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
            }, { "track_id": Uuid::from_u128(5), "along": true,
                 "covers": { "base": [Uuid::from_u128(6)], "covers": [{ "staged": "ab" }, { "id": Uuid::from_u128(6) }, { "copy_of": Uuid::from_u128(7) }] },
                 "sources": { "add": [{ "id": Uuid::from_u128(10), "platform": "custom", "data": { "label": "vk" } }], "trash": [Uuid::from_u128(11)], "origin": Uuid::from_u128(10) } }],
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
        assert!(t.covers.is_none() && t.sources.is_none());
        let covers = edit.tracks[1].covers.as_ref().unwrap();
        assert_eq!(
            covers.covers,
            [CoverItem::Staged { staged: "ab".into() }, CoverItem::Kept { id: Uuid::from_u128(6) }, CoverItem::Copy { copy_of: Uuid::from_u128(7) }]
        );
        let sources = edit.tracks[1].sources.as_ref().unwrap();
        assert_eq!((sources.add[0].id, sources.trash.clone(), sources.origin), (Some(Uuid::from_u128(10)), vec![Uuid::from_u128(11)], Some(Uuid::from_u128(10))));
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
