//! Automated ingestion: what a client knows about a file beyond its tags, and how the
//! server uses it.
//!
//! The track's metadata always comes from the file's tags (see `ingest`). A description
//! only adds *hints*: platform ids (a NetEase artist, album or song id...) and explicit
//! Acetate ids. They decide which existing artist or album a name from the tags means,
//! and get attached as sources so the next file finds them by id instead of by name.
//!
//! Order, for each artist named in the tags: the explicit id, else an artist that already
//! has one of the given platform ids, else the name (`artists_named`). An album:
//! the explicit id, else the platform id, else title + album artists + release date
//! (`find_album`). See the README, "Automated ingestion".

use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::db;
use crate::db::covers::{CoverOwner, NewCover};
use crate::db::ingest::FoundArtist;
use crate::db::revisions::{EditRejected, Source};
use crate::model::sources::{self, Kind, NewSource, SourceOwner, ValidSource};
use crate::model::ImageInfo;

/// Everything a description adds to a file. Empty (the default) for plain uploads.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    pub artists: Vec<ArtistHint>,
    pub album: Option<AlbumHint>,
    /// An artist hint whose name isn't in the tags may go to the one tag artist without a
    /// hint (see `lax_artist_names`). Off unless asked for (`--lax-artist-name`).
    pub lax_artist_names: bool,
    /// The release date, used only if the file's tags have none (e.g. from an id map), or
    /// instead of theirs with `release_date_over_tags`. The first revision says so.
    pub release_date: Option<String>,
    /// `release_date` wins over the tags' date (e.g. SoundCloud's own date, where the tags
    /// have whatever the uploader's software wrote).
    pub release_date_over_tags: bool,
    /// Ids for the first artist in the ARTIST tag, whatever its name: e.g. the uploader of a
    /// SoundCloud song, whose folder has only an id. Added as a hint for that name.
    pub first_artist_sources: Vec<ValidSource>,
    /// Images to add as the track's covers, after the embedded ones.
    pub track_covers: Vec<Vec<u8>>,
    /// Leave out the file's embedded pictures (e.g. when they're resized copies of the
    /// `track_covers`).
    pub skip_embedded_covers: bool,
    /// The original file's name (`files.original_filename`), if the tags have none: e.g.
    /// from the SoundCloud download's own record of it.
    pub original_filename: Option<String>,
    /// Don't credit COMPOSER as producers (e.g. store downloads, whose COMPOSER is the
    /// songwriter's legal name). `--no-composers`.
    pub no_composers: bool,
}

/// For `Hints::first_artist_sources`: the hints with those ids given to `name` (the first
/// ARTIST), added to its hint if there's one already.
pub fn with_first_artist(hints: &Hints, name: &str) -> Hints {
    let mut hints = hints.clone();
    let sources = std::mem::take(&mut hints.first_artist_sources);
    match hints.artists.iter_mut().find(|h| h.name.to_lowercase() == name.to_lowercase()) {
        Some(hint) => hint.sources.extend(sources),
        None => hints.artists.push(ArtistHint { name: name.to_string(), id: None, sources, platform_name: None }),
    }
    hints
}

/// For `Hints::lax_artist_names`: if exactly one hint's name isn't among the file's tag
/// artists (ARTIST and ALBUM ARTIST), and exactly one tag artist has no hint, that hint
/// goes to that tag artist. E.g. a folder "jaywren - 1001554" for a file tagged
/// "mothfecal" (another name of the same artist). The hint's own name is kept as
/// `platform_name`: it decides which artist it is, and the tag's spelling becomes an alias
/// of it. Anything less clear-cut is left as it is. Returns the hints and what was done.
pub fn lax_artist_names(hints: &Hints, tag_artists: &[String]) -> (Hints, Option<String>) {
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    let unmatched: Vec<usize> = (0..hints.artists.len())
        .filter(|&i| !tag_artists.iter().any(|t| same(t, &hints.artists[i].name)))
        .collect();
    let mut without_hint: Vec<&String> =
        tag_artists.iter().filter(|t| !hints.artists.iter().any(|h| same(&h.name, t))).collect();
    without_hint.dedup_by(|a, b| same(a, b));
    let ([hint_index], [tag_name]) = (unmatched.as_slice(), without_hint.as_slice()) else {
        return (hints.clone(), None);
    };
    let mut lax = hints.clone();
    let hint = &mut lax.artists[*hint_index];
    let note = format!(
        "the ids given for \"{}\" went to the tag artist \"{tag_name}\" (--lax-artist-name)",
        hint.name
    );
    hint.platform_name = Some(std::mem::replace(&mut hint.name, tag_name.to_string()));
    (lax, Some(note))
}

/// About one artist named in the file's ARTIST or ALBUM ARTIST tags.
#[derive(Debug, Clone)]
pub struct ArtistHint {
    /// The name exactly as in the tags (after splitting on `;`).
    pub name: String,
    /// "It's this artist": no matching at all.
    pub id: Option<Uuid>,
    /// Where the artist can be found, e.g. their NetEase artist id.
    pub sources: Vec<ValidSource>,
    /// The artist's name on the platform, when the tags call it something else (see
    /// `lax_artist_names`). Matching by name then uses this name, and the tags' spelling
    /// becomes an alias.
    pub platform_name: Option<String>,
}

/// About the album in the file's ALBUM tag.
#[derive(Debug, Clone, Default)]
pub struct AlbumHint {
    pub id: Option<Uuid>,
    pub sources: Vec<ValidSource>,
    /// Images to add as the album's covers (skipped if the album already has them).
    pub covers: Vec<Vec<u8>>,
    /// The track's position on the album, instead of the TRACK tag's.
    pub track_number: Option<i16>,
    /// Whether the album is a single (only the first artist primary, the others featured;
    /// on an album, see `ingest`). None: decided from the tags (a track count of 1).
    pub single: Option<bool>,
    /// The album's own release date, instead of the track's (for finding and creating it).
    pub release_date: Option<String>,
}

/// How an artist or album was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    /// The description gave its Acetate id.
    Id,
    /// It already had one of the platform ids the description gave.
    PlatformId,
    /// By name (an album: by title, album artists and release date).
    Name,
    /// Nothing matched, so it was created.
    Created,
}

#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    /// The name from the tags (an album: its title).
    pub name: String,
    /// None if it's one the user can't see (only possible for a track restricted away
    /// from the uploader).
    pub id: Option<Uuid>,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Created,
    /// These exact bytes were already stored (the description's sources were added to
    /// that track; nothing else changed).
    Duplicate,
}

/// What ingesting one file did (or, in a dry run, would do).
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub status: ReportStatus,
    /// For a duplicate of a track the user can't see: None.
    pub track_id: Option<Uuid>,
    /// Tracks that already had one of this file's platform song ids, so they're probably
    /// the same song in another file. The new track is flagged for review with them.
    pub same_song_as: Vec<Uuid>,
    pub artists: Vec<Resolved>,
    pub album: Option<Resolved>,
    pub album_covers_added: usize,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn duplicate(track_id: Option<Uuid>) -> Report {
        Report {
            status: ReportStatus::Duplicate,
            track_id,
            same_song_as: Vec::new(),
            artists: Vec::new(),
            album: None,
            album_covers_added: 0,
            warnings: Vec::new(),
        }
    }
}

/// Resolves the artists and album of one file, remembering what it found (the same name
/// can be both a track artist and an album artist) and what to report.
#[derive(Default)]
pub struct Resolver {
    artists: Vec<(String, FoundArtist)>,
    pub report_artists: Vec<Resolved>,
    pub report_album: Option<Resolved>,
    pub warnings: Vec<String>,
}

impl Resolver {
    /// The artist a name from the tags means. Runs as the system (inside ingest).
    pub async fn artist(
        &mut self,
        conn: &mut PgConnection,
        hints: &Hints,
        name: &str,
    ) -> anyhow::Result<FoundArtist> {
        if let Some((_, found)) = self.artists.iter().find(|(n, _)| n == name) {
            return Ok(found.clone());
        }
        // The hint for this name: spelled exactly the same, else the same ignoring case
        // (a folder "foo - 123" for the tag "FOO").
        let hint = hints
            .artists
            .iter()
            .find(|h| h.name == name)
            .or_else(|| hints.artists.iter().find(|h| h.name.to_lowercase() == name.to_lowercase()));
        let empty = Vec::new();
        let hint_sources = hint.map_or(&empty, |h| &h.sources);

        let (found, matched_by) = if let Some(id) = hint.and_then(|h| h.id) {
            let id = artist_target(conn, id).await?;
            (self.credit_as(conn, id, name).await?, MatchedBy::Id)
        } else if let Some(id) = self.by_platform_id(conn, Kind::Artist, hint_sources).await? {
            (self.credit_as(conn, id, name).await?, MatchedBy::PlatformId)
        } else {
            // By name: the platform's name for the artist if the tags call it something
            // else (see `lax_artist_names`), else the tags'. An artist with a *different*
            // id on the file's platform is another artist with the same name, so it's
            // ruled out before anything is written (no alias added to it, e.g. "FOO" to a
            // different artist "Foo").
            let lookup = hint.and_then(|h| h.platform_name.as_deref()).unwrap_or(name);
            let mut usable = Vec::new();
            let mut conflict = None;
            for candidate in db::ingest::artists_named(conn, lookup).await? {
                match conflicting_id(conn, SourceOwner::Artist(candidate.artist_id), hint_sources).await? {
                    Some(platform) => conflict = Some(platform),
                    None => usable.push(candidate),
                }
            }
            if !usable.is_empty() {
                let found = db::ingest::use_match(conn, lookup, &usable).await?;
                if lookup == name {
                    (found, MatchedBy::Name)
                } else {
                    // Credited under the tags' spelling, as an alias.
                    (self.credit_as(conn, found.id, name).await?, MatchedBy::Name)
                }
            } else {
                if let Some(platform) = conflict {
                    self.warnings.push(format!(
                        "created a new artist \"{name}\": the existing one with that name has a \
                         different {platform} id"
                    ));
                }
                let created = db::ingest::create_named_artist(conn, lookup).await?;
                if lookup == name {
                    (created, MatchedBy::Created)
                } else {
                    (self.credit_as(conn, created.id, name).await?, MatchedBy::Created)
                }
            }
        };

        self.attach(conn, SourceOwner::Artist(found.id), hint_sources, name).await?;
        self.report_artists.push(Resolved {
            name: name.to_string(),
            id: Some(found.id),
            matched_by,
        });
        self.artists.push((name.to_string(), found.clone()));
        Ok(found)
    }

    /// The album the ALBUM tag means (creating it if nothing matches), with the hint's
    /// sources and covers attached.
    pub async fn album(
        &mut self,
        conn: &mut PgConnection,
        hints: &Hints,
        title: &str,
        album_artists: &[FoundArtist],
        single: bool,
        release_date: Option<&str>,
    ) -> anyhow::Result<Uuid> {
        let empty = AlbumHint::default();
        let hint = hints.album.as_ref().unwrap_or(&empty);

        let (id, matched_by) = if let Some(id) = hint.id {
            (album_target(conn, id).await?, MatchedBy::Id)
        } else if let Some(id) = self.by_platform_id(conn, Kind::Album, &hint.sources).await? {
            (id, MatchedBy::PlatformId)
        } else {
            let artist_ids: Vec<Uuid> = album_artists.iter().map(|a| a.id).collect();
            let existing = db::ingest::find_album(conn, title, &artist_ids, release_date).await?;
            let conflict = match existing {
                Some(id) => conflicting_id(conn, SourceOwner::Album(id), &hint.sources).await?,
                None => None,
            };
            match (existing, conflict) {
                (Some(id), None) => (id, MatchedBy::Name),
                (existing, conflict) => {
                    if existing.is_some() {
                        self.warnings.push(format!(
                            "created a new album \"{title}\": the existing one with that title, \
                             artists and date has a different {} id",
                            conflict.unwrap_or_default()
                        ));
                    }
                    let id = db::ingest::create_album(conn, title, album_artists, single, release_date)
                        .await?;
                    (id, MatchedBy::Created)
                }
            }
        };

        self.attach(conn, SourceOwner::Album(id), &hint.sources, title).await?;
        self.report_album = Some(Resolved {
            name: title.to_string(),
            id: Some(id),
            matched_by,
        });
        Ok(id)
    }

    /// Warns about artist hints for names that aren't in the tags (nothing was done with them).
    pub fn check_unused_hints(&mut self, hints: &Hints) {
        for hint in &hints.artists {
            // Ignoring case, like `artist` finds a hint.
            if !self.artists.iter().any(|(name, _)| name.to_lowercase() == hint.name.to_lowercase()) {
                self.warnings.push(format!(
                    "\"{}\" isn't in the file's ARTIST or ALBUM ARTIST tags, so what was given \
                     about it wasn't used",
                    hint.name
                ));
            }
        }
    }

    /// The artist or album that already has one of these platform ids, if any. If several
    /// do (a duplicate that should be merged), the oldest is used, with a warning.
    async fn by_platform_id(
        &mut self,
        conn: &mut PgConnection,
        kind: Kind,
        hint_sources: &[ValidSource],
    ) -> anyhow::Result<Option<Uuid>> {
        for source in hint_sources {
            let Some(external_id) = &source.external_id else {
                continue;
            };
            let owners =
                db::ingest::owners_with_platform_id(conn, kind, &source.platform, external_id).await?;
            if owners.len() > 1 {
                self.warnings.push(format!(
                    "several {}s have {} id {external_id}; used the oldest",
                    kind_name(kind),
                    source.platform
                ));
            }
            // Ids are UUIDv7 and sorted, so the first is the oldest.
            if let Some(id) = owners.first() {
                return Ok(Some(*id));
            }
        }
        Ok(None)
    }

    /// Credits `artist_id` under the tags' spelling of its name: no alias if it's the
    /// artist's own name, else the alias with that spelling (added if it's new).
    async fn credit_as(
        &mut self,
        conn: &mut PgConnection,
        artist_id: Uuid,
        name: &str,
    ) -> anyhow::Result<FoundArtist> {
        let own_name = sqlx::query_scalar!("SELECT name FROM artists WHERE id = $1", artist_id)
            .fetch_one(&mut *conn)
            .await?;
        let alias_id = if own_name == name {
            None
        } else {
            let alias = db::revisions::ensure_alias(
                conn,
                artist_id,
                name,
                Source::Import,
                "spelling from the tags",
            )
            .await?;
            Some(alias)
        };
        Ok(FoundArtist {
            id: artist_id,
            alias_id,
            ambiguous: Vec::new(),
        })
    }

    /// Adds the hint's sources to what was found. Not a source that was trashed there
    /// before: someone marked it wrong, and ingest shouldn't quietly bring it back.
    async fn attach(
        &mut self,
        conn: &mut PgConnection,
        owner: SourceOwner,
        hint_sources: &[ValidSource],
        name: &str,
    ) -> anyhow::Result<()> {
        for source in hint_sources {
            if let Some(external_id) = &source.external_id {
                let trashed =
                    db::ingest::platform_ids_of(conn, owner, &source.platform, true).await?;
                if trashed.contains(external_id) {
                    self.warnings.push(format!(
                        "didn't add {} id {external_id} to \"{name}\": it was put in the trash there",
                        source.platform
                    ));
                    continue;
                }
            }
            db::sources::add_snapshot(conn, owner, source).await?;
        }
        Ok(())
    }
}

/// Adds covers to an album, skipping images it already has (also ones in its trash:
/// someone removed those on purpose). Returns how many were added.
pub async fn add_album_covers(
    conn: &mut PgConnection,
    album_id: Uuid,
    covers: &[(String, ImageInfo)],
    volume: &str,
) -> anyhow::Result<usize> {
    let mut added = 0;
    for (sha256, image) in covers {
        let has_it = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM covers WHERE album_id = $1 AND blob_sha256 = $2) AS "e!""#,
            album_id,
            sha256
        )
        .fetch_one(&mut *conn)
        .await?;
        if has_it {
            continue;
        }
        let cover = NewCover {
            blob_sha256: sha256,
            mime: Some(image.mime),
            width: image.width,
            height: image.height,
            source: "import",
        };
        db::blobs::register(conn, sha256, volume, None).await?;
        db::covers::add(conn, CoverOwner::Album(album_id), &cover).await?;
        added += 1;
    }
    Ok(added)
}

/// For a file that's already stored: the album the description means, if it's one the
/// user can see and that's found by its explicit id or a platform id (nothing is created
/// or matched by name). Lets a re-run add covers that were missing the first time.
/// Runs with the user's own access.
pub async fn known_album(conn: &mut PgConnection, hint: &AlbumHint) -> anyhow::Result<Option<Uuid>> {
    if let Some(id) = hint.id {
        return Ok(Some(album_target(conn, id).await?)); // checked by `check_explicit_ids`
    }
    for source in &hint.sources {
        let Some(external_id) = &source.external_id else {
            continue;
        };
        let albums =
            db::ingest::owners_with_platform_id(conn, Kind::Album, &source.platform, external_id).await?;
        if let Some(id) = albums.first() {
            return Ok(Some(*id));
        }
    }
    Ok(None)
}

/// The artist an explicit id means (an artist that was merged leads to where it went).
async fn artist_target(conn: &mut PgConnection, artist_id: Uuid) -> anyhow::Result<Uuid> {
    let target = sqlx::query_scalar!(
        r#"SELECT COALESCE(merged_into, id) AS "id!" FROM artists WHERE id = $1"#,
        artist_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    target.ok_or_else(|| EditRejected::NotFound("artist").into())
}

/// The album an explicit id means (an album that was merged leads to where it went).
async fn album_target(conn: &mut PgConnection, album_id: Uuid) -> anyhow::Result<Uuid> {
    let target = sqlx::query_scalar!(
        r#"SELECT COALESCE(merged_into, id) AS "id!" FROM albums WHERE id = $1"#,
        album_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    target.ok_or_else(|| EditRejected::NotFound("album").into())
}

/// If `owner` already has an id on one of the hint's platforms, but not the hint's id,
/// it's a different artist/album that happens to have the same name. Returns the platform.
async fn conflicting_id(
    conn: &mut PgConnection,
    owner: SourceOwner,
    hint_sources: &[ValidSource],
) -> anyhow::Result<Option<String>> {
    for source in hint_sources {
        let Some(external_id) = &source.external_id else {
            continue;
        };
        let ids = db::ingest::platform_ids_of(conn, owner, &source.platform, false).await?;
        if !ids.is_empty() && !ids.contains(external_id) {
            return Ok(Some(source.platform.clone()));
        }
    }
    Ok(None)
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "track",
        Kind::Album => "album",
        Kind::Artist => "artist",
    }
}

/// Explicit ids must be ones the uploader can see. Called before ingest switches to the
/// system, so this goes through row-level security.
pub async fn check_explicit_ids(conn: &mut PgConnection, hints: &Hints) -> anyhow::Result<()> {
    for hint in &hints.artists {
        if let Some(id) = hint.id {
            if !db::sources::owner_exists(conn, SourceOwner::Artist(id)).await? {
                return Err(EditRejected::NotFound("artist").into());
            }
        }
    }
    if let Some(id) = hints.album.as_ref().and_then(|a| a.id) {
        if !db::sources::owner_exists(conn, SourceOwner::Album(id)).await? {
            return Err(EditRejected::NotFound("album").into());
        }
    }
    Ok(())
}

/// Blanks out ids in the report the user can't see. Ingest resolves as the system, and
/// what it matched may be credited only on tracks hidden from the uploader. Called after
/// going back to the user's role.
pub async fn hide_invisible(conn: &mut PgConnection, report: &mut Report) -> anyhow::Result<()> {
    for resolved in &mut report.artists {
        if let Some(id) = resolved.id {
            if !db::sources::owner_exists(conn, SourceOwner::Artist(id)).await? {
                resolved.id = None;
            }
        }
    }
    if let Some(album) = &mut report.album {
        if let Some(id) = album.id {
            if !db::sources::owner_exists(conn, SourceOwner::Album(id)).await? {
                album.id = None;
            }
        }
    }
    let mut visible = Vec::new();
    for id in &report.same_song_as {
        if db::sources::owner_exists(conn, SourceOwner::Track(*id)).await? {
            visible.push(*id);
        }
    }
    report.same_song_as = visible;
    Ok(())
}

// ---- The JSON form (API) ----

/// A source as a client sends it (the inverse of `sources::validate`).
fn source_json(source: &ValidSource) -> serde_json::Value {
    serde_json::json!({
        "platform": source.platform,
        "source_id": source.source_id,
        "captured_at": source.captured_at,
        "data": source.data,
    })
}

/// What a file was imported with, as a description (`NewDescription`'s JSON), to import
/// it again later the same way (see `db::problems`). Album cover images aren't included.
pub fn description_json(
    origin: &ValidSource,
    status: &str,
    track_sources: &[ValidSource],
    hints: &Hints,
    restrict_to: Option<&[Uuid]>,
) -> serde_json::Value {
    let artists: Vec<serde_json::Value> = hints
        .artists
        .iter()
        .map(|a| {
            serde_json::json!({
                "name": a.name,
                "id": a.id,
                "sources": a.sources.iter().map(source_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let album = hints.album.as_ref().map(|a| {
        serde_json::json!({
            "id": a.id,
            "sources": a.sources.iter().map(source_json).collect::<Vec<_>>(),
            "track_number": a.track_number,
            "single": a.single,
        })
    });
    serde_json::json!({
        "origin": source_json(origin),
        "status": status,
        "track": { "sources": track_sources.iter().map(source_json).collect::<Vec<_>>() },
        "album": album,
        "artists": artists,
        "restrict_to": restrict_to,
        "no_composers": hints.no_composers,
    })
}

/// A description as a client sends it (`POST /api/ingest`). See the README for an example.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewDescription {
    /// Where the file came from (required), as for uploads.
    pub origin: NewSource,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub track: NewTrackHint,
    #[serde(default)]
    pub album: Option<NewAlbumHint>,
    #[serde(default)]
    pub artists: Vec<NewArtistHint>,
    /// Create the track restricted to these groups (needs `manage_access`).
    #[serde(default)]
    pub restrict_to: Option<Vec<Uuid>>,
    /// The storage volume for the audio (admins only; left out: the default one).
    #[serde(default)]
    pub volume: Option<String>,
    /// Don't credit COMPOSER as producers (see `Hints::no_composers`).
    #[serde(default)]
    pub no_composers: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTrackHint {
    #[serde(default)]
    pub sources: Vec<NewSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAlbumHint {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub sources: Vec<NewSource>,
    /// Names of the form's image parts to add as album covers.
    #[serde(default)]
    pub covers: Vec<String>,
    /// The track's position on the album, instead of the TRACK tag's.
    #[serde(default)]
    pub track_number: Option<i16>,
    /// Whether it's a single (true) or an album (false). Left out: decided from the tags'
    /// track count ("1/1" is a single).
    #[serde(default)]
    pub single: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewArtistHint {
    pub name: String,
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub sources: Vec<NewSource>,
}

/// A description with its sources validated. `covers` are the form's image parts by name.
pub struct ValidDescription {
    pub origin: ValidSource,
    pub status: Option<String>,
    pub track_sources: Vec<ValidSource>,
    pub hints: Hints,
    pub restrict_to: Option<Vec<Uuid>>,
    pub volume: Option<String>,
}

impl NewDescription {
    /// Validates every source against its platform's shape. `take_cover` hands over the
    /// form part with a given name. Errors are messages for the client.
    pub fn validate(
        self,
        mut take_cover: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Result<ValidDescription, String> {
        let check = |kind: Kind, list: Vec<NewSource>| -> Result<Vec<ValidSource>, String> {
            list.into_iter()
                .map(|s| sources::validate(kind, s).map_err(|err| err.to_string()))
                .collect()
        };
        let origin = sources::validate(Kind::Track, self.origin).map_err(|err| err.to_string())?;
        let track_sources = check(Kind::Track, self.track.sources)?;

        let mut artists = Vec::new();
        for artist in self.artists {
            if artists.iter().any(|a: &ArtistHint| a.name == artist.name) {
                return Err(format!("artist \"{}\" is listed twice", artist.name));
            }
            artists.push(ArtistHint {
                name: artist.name,
                id: artist.id,
                sources: check(Kind::Artist, artist.sources)?,
                platform_name: None,
            });
        }

        let album = match self.album {
            None => None,
            Some(album) => {
                let mut covers = Vec::new();
                for name in &album.covers {
                    let Some(bytes) = take_cover(name) else {
                        return Err(format!("album cover \"{name}\": no form part with that name"));
                    };
                    covers.push(bytes);
                }
                Some(AlbumHint {
                    id: album.id,
                    sources: check(Kind::Album, album.sources)?,
                    covers,
                    track_number: album.track_number,
                    single: album.single,
                    release_date: None,
                })
            }
        };

        Ok(ValidDescription {
            origin,
            status: self.status,
            track_sources,
            hints: Hints { artists, album, no_composers: self.no_composers, ..Default::default() },
            restrict_to: self.restrict_to,
            volume: self.volume,
        })
    }
}
