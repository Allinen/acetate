//! Automated ingestion: what a client knows about a file beyond its tags, and how the
//! server uses it.
//!
//! The track's metadata always comes from the file's tags (see `ingest`). A description
//! only adds *hints*: platform ids (an artist, album or song id there...) and explicit
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
    /// A file already stored only gets the new origin (not merged, see
    /// `ingest::merge_duplicate`). Quicker when files are never stored twice.
    /// `--no-merge-duplicates`.
    pub no_duplicate_merge: bool,
    /// Title and artists given with the description, e.g. from a platform's lookup, for
    /// files whose tags lack them (see `GivenValues`).
    pub given: GivenValues,
    /// Put the new track on the Review page with this note ("fix later": `--flag`).
    pub flag: Option<String>,
    /// All of the metadata, from the description: the file's tags aren't used for anything
    /// (see `Described`). The artists' and album's ids are in `artists` and `album`.
    pub described: Option<Described>,
}

/// The metadata of a file, given whole with its description (`metadata`), for files
/// straight from a platform: without tags, or with tags that can't be trusted. The file's
/// own tags are then used for nothing (not its title, pictures, lyrics or original
/// filename); they're kept with the file, verbatim, as always.
#[derive(Debug, Clone, Default)]
pub struct Described {
    pub title: String,
    /// In order.
    pub credits: Vec<DescribedCredit>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub comment: Option<String>,
    pub album: Option<DescribedAlbum>,
    pub lyrics: Vec<crate::model::LyricsData>,
}

#[derive(Debug, Clone, Default)]
pub struct DescribedAlbum {
    pub title: String,
    /// `album`, `single`, `ep`... (the `album_kinds`).
    pub kind: String,
    pub artists: Vec<DescribedCredit>,
    pub disc_number: Option<i16>,
}

/// One credit of described metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct DescribedCredit {
    /// The name it's credited under.
    pub name: String,
    pub role: String,
    /// Which of `Hints::artists` says who it is (its ids). Two credits with the same name
    /// and different ids (two artists with one name) have different ones. None: found by
    /// the name alone (a name typed in on the Review page).
    pub artist: Option<usize>,
}

/// Values given with a description instead of (or where there are none) the tags': the
/// first revision says each one's origin, so it's clear they're not from the file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GivenValues {
    /// Used only if the tags have no title.
    pub title: Option<String>,
    /// Where it's from, for the revision ("Traktrain", "the file name").
    pub title_from: String,
    /// Used if the tags have no artist, or always with `artists_over_tags`.
    pub artists: Vec<String>,
    pub artists_from: String,
    /// The given artists are the track's (and its album's), whatever the tags say: e.g.
    /// an instrumental's producer, where ARTIST is the uploading channel.
    pub artists_over_tags: bool,
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
    /// Where the artist can be found, e.g. their artist id on a platform.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolved {
    /// The name from the tags (an album: its title).
    pub name: String,
    /// None if it's one the user can't see (only possible for a track restricted away
    /// from the uploader).
    pub id: Option<Uuid>,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Created,
    /// These exact bytes were already stored (the description's sources were added to
    /// that track; nothing else changed).
    Duplicate,
}

/// What ingesting one file did (or, in a dry run, would do).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// For a file that was already stored: what this import added to its track (see
    /// `ingest::merge_duplicate`), e.g. "on the album \"X\"". Empty: nothing.
    #[serde(default)]
    pub added: Vec<String>,
    /// Names in the tags that are collectives (one of their platform ids is a grouping's
    /// account): the track went into those groupings, and no one was credited as them.
    #[serde(default)]
    pub collectives: Vec<Resolved>,
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
            added: Vec::new(),
            collectives: Vec::new(),
        }
    }
}

/// Resolves the artists and album of one file, remembering what it found (the same name
/// can be both a track artist and an album artist) and what to report.
#[derive(Default)]
pub struct Resolver {
    /// (name, who its hint says it is: `hint_key`, found).
    artists: Vec<(String, String, FoundArtist)>,
    /// Names that are collectives: (name, grouping). See `collective`.
    pub collectives: Vec<(String, String, Uuid)>,
    pub report_collectives: Vec<Resolved>,
    pub report_artists: Vec<Resolved>,
    pub report_album: Option<Resolved>,
    pub warnings: Vec<String>,
}

impl Resolver {
    /// The collective a name from the tags is, if it is one: a grouping has one of the
    /// platform ids the description gives for the name (the account the song was posted
    /// under). Then it isn't an artist: no one is credited as it, and the track goes in
    /// the grouping. A name given an Acetate artist id is that artist, whatever else.
    pub async fn collective(&mut self, conn: &mut PgConnection, hints: &Hints, name: &str) -> anyhow::Result<Option<Uuid>> {
        self.collective_for(conn, hint_named(hints, name), name).await
    }

    /// `collective`, with the hint that says who `name` is (see `DescribedCredit::artist`).
    pub async fn collective_for(&mut self, conn: &mut PgConnection, hint: Option<&ArtistHint>, name: &str) -> anyhow::Result<Option<Uuid>> {
        let key = hint_key(hint);
        if let Some((_, _, grouping)) = self.collectives.iter().find(|(n, k, _)| n == name && *k == key) {
            return Ok(Some(*grouping));
        }
        let Some(hint) = hint else {
            return Ok(None);
        };
        if hint.id.is_some() {
            return Ok(None);
        }
        for source in &hint.sources {
            let Some(external_id) = &source.external_id else { continue };
            if let Some(&grouping) = db::groupings::with_platform_id(conn, &source.platform, external_id).await?.first() {
                self.collectives.push((name.to_string(), key, grouping));
                self.report_collectives.push(Resolved { name: name.to_string(), id: Some(grouping), matched_by: MatchedBy::PlatformId });
                return Ok(Some(grouping));
            }
        }
        Ok(None)
    }

    /// The artist a name from the tags means. Runs as the system (inside ingest).
    pub async fn artist(
        &mut self,
        conn: &mut PgConnection,
        hints: &Hints,
        name: &str,
    ) -> anyhow::Result<FoundArtist> {
        self.artist_for(conn, hint_named(hints, name), name).await
    }

    /// `artist`, with the hint that says who `name` is: two artists with the same name each
    /// have their own (see `DescribedCredit::artist`).
    pub async fn artist_for(
        &mut self,
        conn: &mut PgConnection,
        hint: Option<&ArtistHint>,
        name: &str,
    ) -> anyhow::Result<FoundArtist> {
        let key = hint_key(hint);
        if let Some((_, _, found)) = self.artists.iter().find(|(n, k, _)| n == name && *k == key) {
            return Ok(found.clone());
        }
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
        self.artists.push((name.to_string(), key, found.clone()));
        Ok(found)
    }

    /// The album the ALBUM tag means (creating it if nothing matches), with the hint's
    /// sources and covers attached.
    pub async fn album(
        &mut self,
        conn: &mut PgConnection,
        hints: &Hints,
        title: &str,
        album_artists: &[(FoundArtist, String)],
        kind: &str,
        release_date: Option<&str>,
    ) -> anyhow::Result<Uuid> {
        let empty = AlbumHint::default();
        let hint = hints.album.as_ref().unwrap_or(&empty);

        let (id, matched_by) = if let Some(id) = hint.id {
            (album_target(conn, id).await?, MatchedBy::Id)
        } else if let Some(id) = self.by_platform_id(conn, Kind::Album, &hint.sources).await? {
            (id, MatchedBy::PlatformId)
        } else {
            let artist_ids: Vec<Uuid> = album_artists.iter().map(|(a, _)| a.id).collect();
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
                    let id = db::ingest::create_album(conn, title, album_artists, kind, release_date)
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
            let used = |name: &String| name.to_lowercase() == hint.name.to_lowercase();
            if !self.artists.iter().any(|(name, _, _)| used(name)) && !self.collectives.iter().any(|(name, _, _)| used(name)) {
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
        db::blobs::register(conn, sha256, volume, Some(image.size)).await?;
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
        if let Some(id) = hint.id
            && !db::sources::owner_exists(conn, SourceOwner::Artist(id)).await? {
                return Err(EditRejected::NotFound("artist").into());
            }
    }
    if let Some(id) = hints.album.as_ref().and_then(|a| a.id)
        && !db::sources::owner_exists(conn, SourceOwner::Album(id)).await? {
            return Err(EditRejected::NotFound("album").into());
        }
    Ok(())
}

/// Blanks out ids in the report the user can't see. Ingest resolves as the system, and
/// what it matched may be credited only on tracks hidden from the uploader. Called after
/// going back to the user's role.
pub async fn hide_invisible(conn: &mut PgConnection, report: &mut Report) -> anyhow::Result<()> {
    for resolved in &mut report.artists {
        if let Some(id) = resolved.id
            && !db::sources::owner_exists(conn, SourceOwner::Artist(id)).await? {
                resolved.id = None;
            }
    }
    if let Some(album) = &mut report.album
        && let Some(id) = album.id
            && !db::sources::owner_exists(conn, SourceOwner::Album(id)).await? {
                album.id = None;
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
/// it again later the same way (see `db::problems`), or to send it to a server
/// (`acetate ingest --server`). Cover images aren't included (they're form parts), nor
/// the path (`imported_from`, kept separately).
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
            "release_date": a.release_date,
        })
    });
    if let Some(described) = &hints.described {
        return serde_json::json!({
            "origin": source_json(origin),
            "status": status,
            "track": {
                "sources": track_sources.iter().map(source_json).collect::<Vec<_>>(),
                "original_filename": hints.original_filename,
            },
            "restrict_to": restrict_to,
            "no_duplicate_merge": hints.no_duplicate_merge,
            "flag": hints.flag,
            "metadata": metadata_json(described, hints),
        });
    }
    let first_artist = (!hints.first_artist_sources.is_empty()).then(|| {
        serde_json::json!({ "sources": hints.first_artist_sources.iter().map(source_json).collect::<Vec<_>>() })
    });
    serde_json::json!({
        "origin": source_json(origin),
        "status": status,
        "track": {
            "sources": track_sources.iter().map(source_json).collect::<Vec<_>>(),
            "skip_embedded_covers": hints.skip_embedded_covers,
            "release_date": hints.release_date,
            "release_date_over_tags": hints.release_date_over_tags,
            "original_filename": hints.original_filename,
        },
        "album": album,
        "artists": artists,
        "first_artist": first_artist,
        "restrict_to": restrict_to,
        "no_composers": hints.no_composers,
        "no_duplicate_merge": hints.no_duplicate_merge,
        "given": hints.given,
        "flag": hints.flag,
        "lax_artist_names": hints.lax_artist_names,
    })
}

/// How a stored description names a cover kept as a blob (a file that didn't import, see
/// `ingest::record_problem`): this, then its sha256.
pub const STORED_COVER: &str = "sha256:";

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
    /// A file already stored only gets the new origin (see `Hints::no_duplicate_merge`).
    #[serde(default)]
    pub no_duplicate_merge: bool,
    /// Title and artists given instead of the tags' (see `GivenValues`).
    #[serde(default)]
    pub given: GivenValues,
    /// Put the new track on the Review page with this note (see `Hints::flag`).
    #[serde(default)]
    pub flag: Option<String>,
    /// See `Hints::lax_artist_names`.
    #[serde(default)]
    pub lax_artist_names: bool,
    /// Ids for the first artist in the ARTIST tag, whatever its name (see
    /// `Hints::first_artist_sources`).
    #[serde(default)]
    pub first_artist: Option<NewFirstArtistHint>,
    /// The file's path where it's sent from (`files.imported_from`).
    #[serde(default)]
    pub imported_from: Option<String>,
    /// All of the metadata: the file's tags aren't used (see `Described`).
    #[serde(default)]
    pub metadata: Option<NewMetadata>,
}

/// `metadata` in a description: everything about the track, instead of its tags.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewMetadata {
    pub title: String,
    /// At least one primary artist.
    pub credits: Vec<NewCredit>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub album: Option<NewMetadataAlbum>,
    #[serde(default)]
    pub lyrics: Vec<NewLyrics>,
}

/// A credit: the name it's credited under (an alias of the artist, or its name), the role,
/// and how to find the artist (its Acetate id, or its ids on platforms; else by the name).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewCredit {
    pub name: String,
    #[serde(default = "primary")]
    pub role: String,
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub sources: Vec<NewSource>,
}

fn primary() -> String {
    "primary".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewMetadataAlbum {
    pub title: String,
    /// `album` (the default), `single`, `ep`, `compilation`, `mixtape`.
    #[serde(default)]
    pub kind: Option<String>,
    /// The album's own credits. Left out: the track's primary artists.
    #[serde(default)]
    pub artists: Vec<NewCredit>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub disc_number: Option<i16>,
    #[serde(default)]
    pub track_number: Option<i16>,
    /// "It's this album": no matching.
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub sources: Vec<NewSource>,
    /// Names of the form's image parts to add as the album's covers.
    #[serde(default)]
    pub covers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLyrics {
    pub body: String,
    /// From the artist or label (not transcribed by someone).
    #[serde(default)]
    pub official: bool,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewTrackHint {
    #[serde(default)]
    pub sources: Vec<NewSource>,
    /// Names of the form's image parts to add as the track's covers.
    #[serde(default)]
    pub covers: Vec<String>,
    /// Leave out the file's embedded pictures (see `Hints::skip_embedded_covers`).
    #[serde(default)]
    pub skip_embedded_covers: bool,
    /// See `Hints::release_date` and `release_date_over_tags`.
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub release_date_over_tags: bool,
    /// The original file's name, if the tags have none (see `Hints::original_filename`).
    #[serde(default)]
    pub original_filename: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFirstArtistHint {
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
    /// The album's own release date (see `AlbumHint::release_date`).
    #[serde(default)]
    pub release_date: Option<String>,
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
    pub imported_from: Option<String>,
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
        if let Some(metadata) = self.metadata {
            // Parts that only mean something next to the tags aren't taken with `metadata`,
            // rather than quietly left out.
            let refused = [
                ("artists", !self.artists.is_empty(), "give each artist's ids on its credit (metadata.credits)"),
                ("album", self.album.is_some(), "the album goes in metadata.album"),
                ("first_artist", self.first_artist.is_some(), "give the ids on the credit (metadata.credits)"),
                ("given", self.given.title.is_some() || !self.given.artists.is_empty(), "metadata has the values"),
                ("lax_artist_names", self.lax_artist_names, "credits are matched by their own ids"),
                ("no_composers", self.no_composers, "producers are only the credits given"),
                ("track.release_date", self.track.release_date.is_some(), "use metadata.release_date"),
                ("track.release_date_over_tags", self.track.release_date_over_tags, "the tags aren't used"),
                ("track.skip_embedded_covers", self.track.skip_embedded_covers, "embedded pictures aren't used with metadata"),
            ];
            if let Some((name, _, instead)) = refused.iter().find(|(_, given, _)| *given) {
                return Err(format!("`{name}` isn't used with `metadata` ({instead})"));
            }
            let mut track_covers = Vec::new();
            for name in &self.track.covers {
                let Some(bytes) = take_cover(name) else {
                    return Err(format!("track cover \"{name}\": no form part with that name"));
                };
                track_covers.push(bytes);
            }
            let (described, artists, album) = validate_metadata(metadata, &mut take_cover)?;
            return Ok(ValidDescription {
                origin,
                status: self.status,
                track_sources,
                hints: Hints {
                    artists,
                    album,
                    track_covers,
                    skip_embedded_covers: true,
                    original_filename: self.track.original_filename,
                    no_duplicate_merge: self.no_duplicate_merge,
                    flag: self.flag.filter(|f| !f.trim().is_empty()),
                    described: Some(described),
                    ..Hints::default()
                },
                restrict_to: self.restrict_to,
                volume: self.volume,
                imported_from: self.imported_from,
            });
        }

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
                    release_date: album.release_date,
                })
            }
        };

        let mut track_covers = Vec::new();
        for name in &self.track.covers {
            let Some(bytes) = take_cover(name) else {
                return Err(format!("track cover \"{name}\": no form part with that name"));
            };
            track_covers.push(bytes);
        }
        let first_artist_sources = match self.first_artist {
            Some(first) => check(Kind::Artist, first.sources)?,
            None => Vec::new(),
        };

        Ok(ValidDescription {
            origin,
            status: self.status,
            track_sources,
            hints: Hints {
                artists,
                album,
                lax_artist_names: self.lax_artist_names,
                release_date: self.track.release_date,
                release_date_over_tags: self.track.release_date_over_tags,
                first_artist_sources,
                track_covers,
                skip_embedded_covers: self.track.skip_embedded_covers,
                original_filename: self.track.original_filename,
                no_composers: self.no_composers,
                no_duplicate_merge: self.no_duplicate_merge,
                given: self.given,
                flag: self.flag.filter(|f| !f.trim().is_empty()),
                described: None,
            },
            restrict_to: self.restrict_to,
            volume: self.volume,
            imported_from: self.imported_from,
        })
    }
}

/// `metadata`, checked: what it says, the artists' ids (as hints by name) and the album's.
fn validate_metadata(
    metadata: NewMetadata,
    take_cover: &mut impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<(Described, Vec<ArtistHint>, Option<AlbumHint>), String> {
    let title = metadata.title;
    if title.trim().is_empty() {
        return Err("metadata.title is empty".to_string());
    }
    let mut hints: Vec<ArtistHint> = Vec::new();
    // Who each credit is: the same name twice is the same artist (e.g. one who produced their
    // own track), unless their ids say otherwise (different Acetate ids, or different ids on
    // the same platform): then they're two artists with one name.
    let mut add_hint = |credit: NewCredit, what: &str| -> Result<DescribedCredit, String> {
        let name = credit.name;
        if name.trim().is_empty() {
            return Err(format!("{what}: a credit without a name"));
        }
        if credit.role.trim().is_empty() {
            return Err(format!("{what} \"{name}\": no role"));
        }
        let sources = credit
            .sources
            .into_iter()
            .map(|s| sources::validate(Kind::Artist, s).map_err(|err| err.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let same = hints.iter().position(|h| h.name == name && !ids_differ(h, credit.id, &sources));
        let artist = match same {
            Some(index) => {
                let hint = &mut hints[index];
                hint.id = hint.id.or(credit.id);
                for source in sources {
                    if !hint.sources.iter().any(|s| s.platform == source.platform && s.data == source.data) {
                        hint.sources.push(source);
                    }
                }
                index
            }
            None => {
                hints.push(ArtistHint { name: name.clone(), id: credit.id, sources, platform_name: None });
                hints.len() - 1
            }
        };
        Ok(DescribedCredit { name, role: credit.role, artist: Some(artist) })
    };

    let mut credits: Vec<DescribedCredit> = Vec::new();
    for credit in metadata.credits {
        let credit = add_hint(credit, "metadata.credits")?;
        if credits.contains(&credit) {
            return Err(format!("metadata.credits: \"{}\" is credited twice as {}", credit.name, credit.role));
        }
        credits.push(credit);
    }
    if !credits.iter().any(|c| c.role == "primary") {
        return Err("metadata.credits: at least one primary artist is needed".to_string());
    }

    let mut album_hint = None;
    let album = match metadata.album {
        None => None,
        Some(album) => {
            if album.title.trim().is_empty() {
                return Err("metadata.album.title is empty".to_string());
            }
            let kind = album.kind.unwrap_or_else(|| "album".to_string());
            let mut artists = Vec::new();
            for credit in album.artists {
                let credit = add_hint(credit, "metadata.album.artists")?;
                if !artists.contains(&credit) {
                    artists.push(credit);
                }
            }
            if artists.is_empty() {
                artists = credits.iter().filter(|c| c.role == "primary").cloned().collect();
            }
            let mut covers = Vec::new();
            for name in &album.covers {
                let Some(bytes) = take_cover(name) else {
                    return Err(format!("album cover \"{name}\": no form part with that name"));
                };
                covers.push(bytes);
            }
            album_hint = Some(AlbumHint {
                id: album.id,
                sources: album
                    .sources
                    .into_iter()
                    .map(|s| sources::validate(Kind::Album, s).map_err(|err| err.to_string()))
                    .collect::<Result<Vec<_>, _>>()?,
                covers,
                track_number: album.track_number,
                single: Some(kind == "single"),
                release_date: album.release_date,
            });
            Some(DescribedAlbum { title: album.title, kind, artists, disc_number: album.disc_number })
        }
    };
    let lyrics = metadata
        .lyrics
        .into_iter()
        .filter(|l| !l.body.trim().is_empty())
        .map(|l| crate::model::LyricsData { body: l.body, is_official: l.official, language: l.language, removed: false })
        .collect();
    let described = Described {
        title,
        credits,
        release_date: metadata.release_date,
        genres: metadata.genres,
        comment: metadata.comment,
        album,
        lyrics,
    };
    Ok((described, hints, album_hint))
}

/// `metadata` as a client sends it (the inverse of `validate_metadata`), for
/// `description_json`. Each credit carries its artist's ids.
fn metadata_json(described: &Described, hints: &Hints) -> serde_json::Value {
    let credit = |credit: &DescribedCredit| {
        let hint = credit.artist.and_then(|i| hints.artists.get(i));
        serde_json::json!({
            "name": credit.name,
            "role": credit.role,
            "id": hint.and_then(|h| h.id),
            "sources": hint.map(|h| h.sources.iter().map(source_json).collect::<Vec<_>>()).unwrap_or_default(),
        })
    };
    let album = described.album.as_ref().map(|album| {
        let hint = hints.album.clone().unwrap_or_default();
        serde_json::json!({
            "title": album.title,
            "kind": album.kind,
            "artists": album.artists.iter().map(credit).collect::<Vec<_>>(),
            "release_date": hint.release_date,
            "disc_number": album.disc_number,
            "track_number": hint.track_number,
            "id": hint.id,
            "sources": hint.sources.iter().map(source_json).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({
        "title": described.title,
        "credits": described.credits.iter().map(credit).collect::<Vec<_>>(),
        "release_date": described.release_date,
        "genres": described.genres,
        "comment": described.comment,
        "album": album,
        "lyrics": described.lyrics.iter().map(|l| serde_json::json!({
            "body": l.body, "official": l.is_official, "language": l.language,
        })).collect::<Vec<_>>(),
    })
}

/// The credits of described metadata, in order. `primaries`: the primary artists the track
/// ends up with. They're the description's, unless others were typed in on the Review page
/// (a file that didn't import): then those replace its primary credits.
pub fn described_credits(described: &Described, primaries: &[String]) -> Vec<DescribedCredit> {
    let own: Vec<&String> = described.credits.iter().filter(|c| c.role == "primary").map(|c| &c.name).collect();
    if own.len() == primaries.len() && own.iter().zip(primaries).all(|(a, b)| *a == b) {
        return described.credits.clone();
    }
    primaries
        .iter()
        .map(|name| DescribedCredit { name: name.clone(), role: "primary".to_string(), artist: None })
        .chain(described.credits.iter().filter(|c| c.role != "primary").cloned())
        .collect()
}

/// Whether ids say a hint and these are two different artists: different Acetate ids, or
/// different ids on the same platform.
fn ids_differ(hint: &ArtistHint, id: Option<Uuid>, sources: &[ValidSource]) -> bool {
    if hint.id.is_some() && id.is_some() && hint.id != id {
        return true;
    }
    hint.sources.iter().any(|a| {
        sources.iter().any(|b| {
            a.platform == b.platform && a.external_id.is_some() && b.external_id.is_some() && a.external_id != b.external_id
        })
    })
}

/// The hint for a name from the tags: spelled exactly the same, else the same ignoring case
/// (a folder "foo - 123" for the tag "FOO").
pub fn hint_named<'a>(hints: &'a Hints, name: &str) -> Option<&'a ArtistHint> {
    hints
        .artists
        .iter()
        .find(|h| h.name == name)
        .or_else(|| hints.artists.iter().find(|h| h.name.to_lowercase() == name.to_lowercase()))
}

/// Who a hint says an artist is, to tell two artists with one name apart in `Resolver`:
/// its Acetate id and platform ids.
fn hint_key(hint: Option<&ArtistHint>) -> String {
    let Some(hint) = hint else { return String::new() };
    let mut ids: Vec<String> = hint
        .sources
        .iter()
        .filter_map(|s| s.external_id.as_ref().map(|id| format!("{}:{id}", s.platform)))
        .collect();
    ids.sort();
    format!("{:?} {}", hint.id, ids.join(","))
}

/// Refuses described metadata with a credit role or album kind the server doesn't have,
/// before anything is looked up or made.
pub async fn check_described_vocab(conn: &mut PgConnection, described: &Described) -> anyhow::Result<()> {
    let roles = sqlx::query_scalar!(r#"SELECT word AS "word!" FROM vocabulary WHERE list = 'credit_role' ORDER BY word"#).fetch_all(&mut *conn).await?;
    let album_artists = described.album.iter().flat_map(|a| &a.artists);
    for DescribedCredit { name, role, .. } in described.credits.iter().chain(album_artists) {
        if !roles.contains(role) {
            return Err(super::Rejected::Invalid(format!(
                "\"{name}\": there's no credit role \"{role}\" (there are: {})",
                roles.join(", ")
            ))
            .into());
        }
    }
    if let Some(album) = &described.album {
        let kinds = sqlx::query_scalar!(r#"SELECT word AS "word!" FROM vocabulary WHERE list = 'album_kind' ORDER BY word"#).fetch_all(&mut *conn).await?;
        if !kinds.contains(&album.kind) {
            return Err(super::Rejected::Invalid(format!(
                "metadata.album: there's no album kind \"{}\" (there are: {})",
                album.kind,
                kinds.join(", ")
            ))
            .into());
        }
    }
    Ok(())
}
