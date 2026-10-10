//! `acetate import-navidrome`: moves users and their plays, stars and ratings over from a
//! Navidrome server.
//!
//! Generic: nothing here depends on how the library was imported into Acetate. Things are
//! matched by content:
//! * a Navidrome track (`media_file`) is the Acetate track whose file (current or previous)
//!   was imported from the same path (`files.imported_from`; fast, nothing is read), else
//!   the one whose file has the same bytes: the file is read from disk where Navidrome
//!   says it is (with `--path-map` for Docker mounts and the like) and hashed;
//! * an album, the Acetate album its tracks are on;
//! * an artist, the Acetate artist with that name or alias (its tracks decide between
//!   artists with the same name).
//!
//! Playlists come along too: each owner's, in Navidrome's order, with the tracks found
//! as above (smart playlists, having no fixed tracks, are left out).
//!
//! What can't be matched surely is reported, never guessed. Navidrome's database is only
//! read, and so are the music files. Running it again doesn't count anything twice: values
//! only go up (see `raise`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit};
use anyhow::Context;
use base64::Engine;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use crate::db;
use crate::db::annotations::{Annotation, Item};

/// The key Navidrome encrypts passwords with unless `PasswordEncryptionKey` is set.
pub const DEFAULT_PASSWORD_KEY: &str = "just for obfuscation";
/// How many of an album's or artist's files are hashed at most to find it in Acetate.
const SAMPLE_FILES: i64 = 5;

/// A kind of thing that can be brought over; `--only` picks which.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, clap::ValueEnum)]
pub enum Part {
    /// Accounts (without it, Navidrome's users are matched to existing ones by name).
    Users,
    /// Play counts, last played, stars and ratings.
    Annotations,
    /// Regular playlists.
    Playlists,
    /// The play log: each play with its time.
    Scrobbles,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Users, Part::Annotations, Part::Playlists, Part::Scrobbles];
}

pub struct Options {
    pub db: PathBuf,
    /// What to bring over.
    pub parts: HashSet<Part>,
    /// Navidrome path prefix → where those files are here, longest prefix first.
    pub path_maps: Vec<(String, PathBuf)>,
    /// Only these Navidrome libraries (by name or id), each optionally with where its
    /// folder is here. Empty: all of them.
    pub libraries: Vec<(String, Option<PathBuf>)>,
    pub password_key: String,
    pub dry_run: bool,
}

/// Parses `--library "Music Library"` or `--library "Music Library=/home/me/Music"`.
pub fn parse_library(text: &str) -> (String, Option<PathBuf>) {
    match text.split_once('=') {
        Some((name, path)) => (name.to_string(), Some(PathBuf::from(path))),
        None => (text.to_string(), None),
    }
}

/// Parses `--path-map /music=/home/me/Music`.
pub fn parse_path_map(text: &str) -> anyhow::Result<(String, PathBuf)> {
    let (from, to) = text
        .split_once('=')
        .context("--path-map is FROM=TO, e.g. /music=/home/me/Music")?;
    Ok((from.trim_end_matches('/').to_string(), PathBuf::from(to)))
}

pub async fn run(pool: &PgPool, mut options: Options) -> anyhow::Result<()> {
    let nd = open(&options.db).await?;
    let wants = |part: Part| options.parts.contains(&part);
    let has_libraries = check_schema(&nd).await?;

    // Which libraries, and where they are here.
    let only_libraries: Option<HashSet<i64>> = if options.libraries.is_empty() {
        None
    } else {
        if !has_libraries {
            anyhow::bail!("this Navidrome has no libraries (an older version): use --path-map instead of --library");
        }
        let all: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, name, path FROM library")
            .fetch_all(&nd)
            .await?;
        let mut ids = HashSet::new();
        for (wanted, local_path) in &options.libraries {
            let Some((id, name, path)) = all.iter().find(|(id, name, _)| name == wanted || id.to_string() == *wanted) else {
                let names: Vec<String> = all.iter().map(|(id, name, path)| format!("{name} (id {id}, {path})")).collect();
                anyhow::bail!("no Navidrome library \"{wanted}\"; there are: {}", names.join(", "));
            };
            println!("library {name} (id {id}): Navidrome sees it at {path}{}", match local_path {
                Some(local) => format!(", here at {}", local.display()),
                None => String::new(),
            });
            ids.insert(*id);
            if let Some(local) = local_path {
                options.path_maps.push((path.trim_end_matches('/').to_string(), local.clone()));
            }
        }
        Some(ids)
    };
    options.path_maps.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));

    // ---- Users ----
    let users = sqlx::query("SELECT id, user_name, password, is_admin FROM user ORDER BY user_name")
        .fetch_all(&nd)
        .await?;
    let key = Sha256::digest(options.password_key.as_bytes());

    // Everything in Acetate happens in one transaction: all or nothing, and a dry run just
    // rolls it back.
    let mut tx = db::begin(pool, None).await?;
    let mut user_ids: HashMap<String, Uuid> = HashMap::new();
    let (mut created, mut matched, mut no_password, mut left_out) = (0, 0, Vec::new(), Vec::<String>::new());
    for row in &users {
        let nd_id: String = row.try_get("id")?;
        let name: String = row.try_get("user_name")?;
        let is_admin: bool = row.try_get::<i64, _>("is_admin")? != 0;
        if let Some(id) = db::users::find_user_id(&mut tx, &name).await? {
            matched += 1;
            user_ids.insert(nd_id, id);
            continue;
        }
        if !wants(Part::Users) {
            left_out.push(name);
            continue;
        }
        let encrypted: String = row.try_get("password")?;
        let hash = match decrypt_password(&key, &encrypted) {
            Some(password) => crate::auth::hash_password_async(password).await?,
            None => {
                no_password.push(name.clone());
                // Unusable until an admin sets one.
                crate::auth::hash_password_async(crate::auth::new_token()).await?
            }
        };
        let id = db::users::create_user(&mut tx, &name, &hash).await?;
        let group = if is_admin { "admin" } else { "user" };
        db::users::set_groups(&mut tx, id, &[group.to_string()])
            .await?
            .map_err(anyhow::Error::msg)?;
        println!("user {name}: created ({group})");
        created += 1;
        user_ids.insert(nd_id, id);
    }
    println!("users: {created} created, {matched} already in Acetate (left as they are)");
    if !left_out.is_empty() {
        println!("  not in Acetate, and users aren't being imported, so their data is left out: {}", left_out.join(", "));
    }
    if !no_password.is_empty() {
        println!(
            "  passwords that didn't decrypt (wrong --password-key?), set one by hand: {}",
            no_password.join(", ")
        );
    }

    // ---- What was annotated, found in Acetate ----
    let rows = if wants(Part::Annotations) {
        sqlx::query(
            r#"SELECT user_id, item_id, item_type, play_count, play_date, rating, starred, starred_at, rated_at
               FROM annotation ORDER BY item_type, item_id"#,
        )
        .fetch_all(&nd)
        .await?
    } else {
        Vec::new()
    };
    let mut matcher = Matcher {
        nd: &nd,
        has_libraries,
        only_libraries: only_libraries.clone(),
        path_maps: &options.path_maps,
        tracks: HashMap::new(),
        albums: HashMap::new(),
        artists: HashMap::new(),
        stats: Stats::default(),
    };
    // (Acetate user, Acetate item) → the combined annotation.
    let mut found: HashMap<(Uuid, Uuid), (Item, Annotation)> = HashMap::new();
    let total = rows.len();
    for (n, row) in rows.iter().enumerate() {
        if n % 500 == 0 {
            println!("  matching {n}/{total}");
        }
        let nd_user: String = row.try_get("user_id")?;
        let Some(&user_id) = user_ids.get(&nd_user) else { continue };
        let item_id: String = row.try_get("item_id")?;
        let item_type: String = row.try_get("item_type")?;
        if !matcher.in_libraries(&item_type, &item_id).await? {
            continue;
        }
        let item = match item_type.as_str() {
            "media_file" => matcher.track(&mut tx, &item_id).await?.map(Item::Track),
            "album" => matcher.album(&mut tx, &item_id).await?.map(Item::Album),
            "artist" => matcher.artist(&mut tx, &item_id).await?.map(Item::Artist),
            _ => None,
        };
        let Some(item) = item else { continue };
        let annotation = Annotation {
            play_count: row.try_get::<Option<i64>, _>("play_count")?.unwrap_or(0).clamp(0, i32::MAX as i64) as i32,
            last_played_at: parse_time(row.try_get("play_date")?),
            // Navidrome rates 0 to 5 stars; Acetate 0 to 10 (half stars).
            rating: (row.try_get::<Option<i64>, _>("rating")?.unwrap_or(0).clamp(0, 5) * 2) as i16,
            rated_at: parse_time(row.try_get("rated_at")?),
            starred: row.try_get::<Option<i64>, _>("starred")?.unwrap_or(0) != 0,
            starred_at: parse_time(row.try_get("starred_at")?),
        };
        let key = (user_id, item_id_of(item));
        // Several Navidrome files of one Acetate track (duplicates merged in Acetate) add up.
        let entry = found.entry(key).or_insert((item, Annotation::default()));
        entry.1 = entry.1.combined(&annotation);
    }

    // ---- Playlists ----
    // In Navidrome's order; the owner mapped like annotations. Smart playlists (rules)
    // have no fixed tracks and are left out. One imported before is brought up to date
    // if nobody changed it since, else left as it is (see `playlists::import`).
    let playlist_rows = if wants(Part::Playlists) {
        sqlx::query("SELECT id, name, comment, public, owner_id, COALESCE(rules, '') AS rules FROM playlist ORDER BY name")
            .fetch_all(&nd)
            .await?
    } else {
        Vec::new()
    };
    let mut playlist_lines = Vec::new();
    let (mut playlists_made, mut playlists_refreshed, mut playlists_there, mut smart) = (0, 0, 0, 0);
    for row in &playlist_rows {
        let nd_id: String = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let rules: String = row.try_get("rules")?;
        if !rules.trim().is_empty() {
            smart += 1;
            continue;
        }
        let nd_owner: String = row.try_get("owner_id")?;
        let Some(&owner) = user_ids.get(&nd_owner) else {
            playlist_lines.push(format!("\"{name}\": its owner isn't a user here, left out"));
            continue;
        };
        let comment: String = row.try_get("comment")?;
        let public = row.try_get::<i64, _>("public")? != 0;
        let entries = sqlx::query("SELECT media_file_id FROM playlist_tracks WHERE playlist_id = ? ORDER BY id")
            .bind(&nd_id)
            .fetch_all(&nd)
            .await?;
        let (mut tracks, mut missing, mut elsewhere) = (Vec::new(), 0, 0);
        for entry in &entries {
            let media_file: String = entry.try_get("media_file_id")?;
            if !matcher.in_libraries("media_file", &media_file).await? {
                elsewhere += 1;
                continue;
            }
            match matcher.track(&mut tx, &media_file).await? {
                Some(track) => tracks.push(track),
                None => missing += 1,
            }
        }
        let (_, result) =
            db::playlists::import(&mut tx, owner, &format!("navidrome:{nd_id}"), &name, &comment, public, &tracks).await?;
        use db::playlists::Imported;
        let mut line = match result {
            Imported::Made => {
                playlists_made += 1;
                format!("\"{name}\": {} of {} tracks", tracks.len(), entries.len())
            }
            Imported::Refreshed => {
                playlists_refreshed += 1;
                format!("\"{name}\": imported before, unchanged since: now {} of {} tracks", tracks.len(), entries.len())
            }
            Imported::Kept => {
                playlists_there += 1;
                format!("\"{name}\": imported before and changed since, left as it is")
            }
        };
        if result != Imported::Kept && missing > 0 {
            line += &format!(" ({missing} not matched: not in Acetate, or not on disk here)");
        }
        if result != Imported::Kept && elsewhere > 0 {
            line += &format!(" ({elsewhere} in other libraries, left out)");
        }
        playlist_lines.push(line);
    }

    // ---- Write ----
    let mut written = 0;
    for ((user_id, _), (item, imported)) in &found {
        let existing = db::annotations::get_for(&mut tx, *user_id, *item).await?;
        let raised = raise(&existing, imported);
        if raised != existing {
            db::annotations::put_for(&mut tx, *user_id, *item, &raised).await?;
            written += 1;
        }
    }

    // ---- Plays: each with its time (Navidrome's `scrobbles`, where it has them) ----
    // Only the log: the counts came with the annotations above. Each once, however often
    // this runs (`imported_key`).
    let has_scrobbles: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'scrobbles')")
        .fetch_one(&nd)
        .await?;
    let (mut plays_added, mut plays_unmatched) = (0, 0);
    if wants(Part::Scrobbles) && !has_scrobbles {
        println!("this Navidrome has no play log (scrobbles; Navidrome 0.55 or later): no plays with times to import");
    }
    if wants(Part::Scrobbles) && has_scrobbles {
        let scrobbles = sqlx::query("SELECT media_file_id, user_id, submission_time FROM scrobbles ORDER BY submission_time")
            .fetch_all(&nd)
            .await?;
        for (n, row) in scrobbles.iter().enumerate() {
            if n % 5000 == 0 {
                println!("  plays {n}/{}", scrobbles.len());
            }
            let nd_user: String = row.try_get("user_id")?;
            let Some(&user_id) = user_ids.get(&nd_user) else { continue };
            let media_file: String = row.try_get("media_file_id")?;
            if !matcher.in_libraries("media_file", &media_file).await? {
                continue;
            }
            let Some(track) = matcher.track(&mut tx, &media_file).await? else {
                plays_unmatched += 1;
                continue;
            };
            let time: i64 = row.try_get("submission_time")?;
            let at = DateTime::<Utc>::from_timestamp(time, 0).unwrap_or_default();
            // Navidrome's play log has no id of its own: who, what and when is the identity.
            let added = sqlx::query(
                "INSERT INTO plays (user_id, track_id, played_at, source, imported_key) VALUES ($1, $2, $3, 'navidrome', $4)
                 ON CONFLICT (imported_key) DO NOTHING",
            )
            .bind(user_id)
            .bind(track)
            .bind(at)
            .bind(format!("navidrome:{nd_user}:{media_file}:{time}"))
            .execute(&mut *tx)
            .await?
            .rows_affected();
            plays_added += added;
        }
    }

    let s = &matcher.stats;
    println!(
        "\ntracks:  {} matched ({} by the path they were imported from, the rest by content), {} not in Acetate, \
         {} not on disk, {} can't be read",
        s.tracks_found, s.tracks_found_by_path, s.tracks_not_in_acetate, s.tracks_not_on_disk, s.tracks_unreadable
    );
    println!("albums:  {} matched, {} not found, {} unclear (tracks on several albums)", s.albums_found, s.albums_not_found, s.albums_unclear);
    println!("artists: {} matched, {} not found, {} unclear (several with that name)", s.artists_found, s.artists_not_found, s.artists_unclear);
    for line in s.notes.iter().take(40) {
        println!("  {line}");
    }
    if s.notes.len() > 40 {
        println!("  ... and {} more", s.notes.len() - 40);
    }
    if wants(Part::Annotations) {
        println!("annotations: {written} written ({} matched)", found.len());
    }
    if wants(Part::Scrobbles) && has_scrobbles {
        println!("plays (with their times): {plays_added} added, {plays_unmatched} of tracks not matched");
    }
    if wants(Part::Playlists) {
        println!(
            "playlists: {playlists_made} imported, {playlists_refreshed} brought up to date (unchanged since imported), \
             {playlists_there} left as they are (changed since imported){}",
            if smart > 0 { format!(", {smart} smart playlists left out (no fixed tracks)") } else { String::new() }
        );
        for line in &playlist_lines {
            println!("  {line}");
        }
    }
    if options.dry_run {
        tx.rollback().await?;
        println!("dry run: nothing was changed");
    } else {
        tx.commit().await?;
    }
    Ok(())
}

/// Opens Navidrome's database so it can't be changed: read-only, and "immutable" (no locks,
/// no journal files created next to it).
async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("opening Navidrome's database {}", path.display()))
}

/// Checks the tables and columns used here exist (Navidrome's schema changes between
/// versions). Returns whether it has libraries (paths relative to a library's folder).
async fn check_schema(nd: &SqlitePool) -> anyhow::Result<bool> {
    let needed: &[(&str, &[&str])] = &[
        ("user", &["id", "user_name", "password", "is_admin"]),
        ("annotation", &["user_id", "item_id", "item_type", "play_count", "play_date", "rating", "starred", "starred_at", "rated_at"]),
        ("media_file", &["id", "path", "album_id"]),
        ("artist", &["id", "name"]),
        ("playlist", &["id", "name", "comment", "public", "owner_id", "rules"]),
        ("playlist_tracks", &["id", "playlist_id", "media_file_id"]),
    ];
    let mut missing = Vec::new();
    for (table, columns) in needed {
        let have: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info(?)")
            .bind(*table)
            .fetch_all(nd)
            .await?;
        for column in *columns {
            if !have.iter().any(|c| c == column) {
                missing.push(format!("{table}.{column}"));
            }
        }
    }
    if !missing.is_empty() {
        anyhow::bail!("this Navidrome database doesn't have: {} (a different Navidrome version?)", missing.join(", "));
    }
    let library: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('media_file') WHERE name = 'library_id'")
        .fetch_all(nd)
        .await?;
    Ok(!library.is_empty())
}

/// Navidrome's password encryption: AES-256-GCM with the SHA-256 of the key text, stored
/// as base64 of nonce + ciphertext. None if it doesn't decrypt (another key).
pub fn decrypt_password(key: &[u8], encrypted: &str) -> Option<String> {
    let data = base64::engine::general_purpose::STANDARD.decode(encrypted).ok()?;
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    const NONCE_LEN: usize = 12;
    if data.len() < NONCE_LEN {
        return None;
    }
    let (nonce, ciphertext) = data.split_at(NONCE_LEN);
    let plain = cipher.decrypt(nonce.try_into().ok()?, ciphertext).ok()?;
    String::from_utf8(plain).ok()
}

/// Navidrome's times: `2026-08-24 12:46:49.070923065+00:00` (or RFC 3339).
fn parse_time(text: Option<String>) -> Option<DateTime<Utc>> {
    let text = text?;
    DateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S%.f%:z")
        .or_else(|_| DateTime::parse_from_rfc3339(&text))
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// What Acetate ends up with: the imported values, without lowering anything that's
/// already there (so running the import again, or after playing in Acetate, is safe).
fn raise(existing: &Annotation, imported: &Annotation) -> Annotation {
    let mut a = existing.clone();
    a.play_count = a.play_count.max(imported.play_count);
    a.last_played_at = a.last_played_at.max(imported.last_played_at);
    if a.rating == 0 && imported.rating > 0 {
        (a.rating, a.rated_at) = (imported.rating, imported.rated_at);
    }
    if !a.starred && imported.starred {
        (a.starred, a.starred_at) = (true, imported.starred_at);
    }
    a
}

fn item_id_of(item: Item) -> Uuid {
    match item {
        Item::Track(id) | Item::Album(id) | Item::Artist(id) => id,
    }
}

#[derive(Default)]
struct Stats {
    tracks_found: usize,
    /// Of `tracks_found`, by the path they were imported from (not hashed).
    tracks_found_by_path: usize,
    tracks_not_in_acetate: usize,
    tracks_not_on_disk: usize,
    tracks_unreadable: usize,
    albums_found: usize,
    albums_not_found: usize,
    albums_unclear: usize,
    artists_found: usize,
    artists_not_found: usize,
    artists_unclear: usize,
    notes: Vec<String>,
}

/// Finds Navidrome's things in Acetate, remembering what it found.
struct Matcher<'a> {
    nd: &'a SqlitePool,
    has_libraries: bool,
    /// Only files, albums and artists of these libraries (None: all).
    only_libraries: Option<HashSet<i64>>,
    path_maps: &'a [(String, PathBuf)],
    tracks: HashMap<String, Option<Uuid>>,
    albums: HashMap<String, Option<Uuid>>,
    artists: HashMap<String, Option<Uuid>>,
    stats: Stats,
}

impl Matcher<'_> {
    /// Whether an annotated thing belongs to the chosen libraries (always, without
    /// `--library`). An artist counts if it's in any of them.
    async fn in_libraries(&self, item_type: &str, item_id: &str) -> anyhow::Result<bool> {
        let Some(only) = &self.only_libraries else {
            return Ok(true);
        };
        let libraries: Vec<i64> = match item_type {
            "media_file" => sqlx::query_scalar("SELECT library_id FROM media_file WHERE id = ?")
                .bind(item_id)
                .fetch_all(self.nd)
                .await?,
            "album" => sqlx::query_scalar("SELECT library_id FROM album WHERE id = ?")
                .bind(item_id)
                .fetch_all(self.nd)
                .await?,
            "artist" => sqlx::query_scalar("SELECT library_id FROM library_artist WHERE artist_id = ?")
                .bind(item_id)
                .fetch_all(self.nd)
                .await?,
            _ => Vec::new(),
        };
        Ok(libraries.iter().any(|id| only.contains(id)))
    }

    /// The Acetate track a Navidrome file is: same bytes.
    async fn track(&mut self, conn: &mut PgConnection, media_file_id: &str) -> anyhow::Result<Option<Uuid>> {
        if let Some(found) = self.tracks.get(media_file_id) {
            return Ok(*found);
        }
        let found = self.find_track(conn, media_file_id).await?;
        self.tracks.insert(media_file_id.to_string(), found);
        Ok(found)
    }

    async fn find_track(&mut self, conn: &mut PgConnection, media_file_id: &str) -> anyhow::Result<Option<Uuid>> {
        let Some(path) = self.path_of(media_file_id).await? else {
            return Ok(None);
        };
        // Imported from this very path: that's it, no need to read the file. (Only if
        // it's one track: a path reused for several imports says nothing.)
        if let Some(text) = path.to_str()
            && let [track] = db::ingest::tracks_imported_from(conn, text).await?[..] {
                let track = sqlx::query_scalar!(r#"SELECT COALESCE(merged_into, id) AS "id!" FROM tracks WHERE id = $1"#, track)
                    .fetch_one(&mut *conn)
                    .await?;
                self.stats.tracks_found += 1;
                self.stats.tracks_found_by_path += 1;
                return Ok(Some(track));
            }
        let hashed = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || crate::storage::hash_file(&path)).await?
        };
        let sha = match hashed {
            Ok((sha, _)) => sha,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.stats.tracks_not_on_disk += 1;
                self.stats.notes.push(format!("not on disk: {}", path.display()));
                return Ok(None);
            }
            // There, but it can't be read (permissions, usually): say why.
            Err(error) => {
                self.stats.tracks_unreadable += 1;
                self.stats.notes.push(format!("can't read ({error}): {}", path.display()));
                return Ok(None);
            }
        };
        let track = db::ingest::find_file_by_sha256(conn, &sha).await?.and_then(|f| f.track_id);
        let Some(track) = track else {
            self.stats.tracks_not_in_acetate += 1;
            self.stats.notes.push(format!("not in Acetate: {}", path.display()));
            return Ok(None);
        };
        // A track merged into another counts as that one.
        let track = sqlx::query_scalar!(r#"SELECT COALESCE(merged_into, id) AS "id!" FROM tracks WHERE id = $1"#, track)
            .fetch_one(&mut *conn)
            .await?;
        self.stats.tracks_found += 1;
        Ok(Some(track))
    }

    /// Where a Navidrome file is on this machine.
    async fn path_of(&self, media_file_id: &str) -> anyhow::Result<Option<PathBuf>> {
        let row: Option<(String, Option<String>)> = if self.has_libraries {
            let row: Option<(String, Option<String>, i64)> = sqlx::query_as(
                "SELECT m.path, l.path, m.library_id FROM media_file m LEFT JOIN library l ON l.id = m.library_id WHERE m.id = ?",
            )
            .bind(media_file_id)
            .fetch_optional(self.nd)
            .await?;
            // A file of a library that wasn't chosen isn't looked at (it may not be here).
            match row {
                Some((_, _, library)) if self.only_libraries.as_ref().is_some_and(|only| !only.contains(&library)) => None,
                Some((path, library_path, _)) => Some((path, library_path)),
                None => None,
            }
        } else {
            sqlx::query_as("SELECT path, NULL FROM media_file WHERE id = ?")
                .bind(media_file_id)
                .fetch_optional(self.nd)
                .await?
        };
        let Some((path, library)) = row else {
            return Ok(None);
        };
        let full = match library {
            Some(library) => Path::new(&library).join(&path),
            None => PathBuf::from(&path),
        };
        Ok(Some(map_path(&full, self.path_maps)))
    }

    /// The Acetate album holding a Navidrome album's tracks. A few of its files are
    /// hashed until they agree on one album.
    async fn album(&mut self, conn: &mut PgConnection, album_id: &str) -> anyhow::Result<Option<Uuid>> {
        if let Some(found) = self.albums.get(album_id) {
            return Ok(*found);
        }
        let files: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM media_file WHERE album_id = ? ORDER BY disc_number, track_number LIMIT ?",
        )
        .bind(album_id)
        .bind(SAMPLE_FILES)
        .fetch_all(self.nd)
        .await?;
        // The albums each matched track is on; the answer is on all of them.
        let mut candidates: Option<Vec<Uuid>> = None;
        for file in files {
            let Some(track) = self.track(conn, &file).await? else { continue };
            let albums = sqlx::query_scalar!("SELECT album_id FROM album_tracks WHERE track_id = $1", track)
                .fetch_all(&mut *conn)
                .await?;
            let narrowed: Vec<Uuid> = match candidates {
                None => albums,
                Some(previous) => previous.into_iter().filter(|a| albums.contains(a)).collect(),
            };
            candidates = Some(narrowed.clone());
            if narrowed.len() <= 1 {
                break;
            }
        }
        let name: String = sqlx::query_scalar("SELECT name FROM album WHERE id = ?")
            .bind(album_id)
            .fetch_optional(self.nd)
            .await?
            .unwrap_or_default();
        let found = match candidates.unwrap_or_default().as_slice() {
            [one] => {
                self.stats.albums_found += 1;
                Some(*one)
            }
            [] => {
                self.stats.albums_not_found += 1;
                self.stats.notes.push(format!("album not found: {name}"));
                None
            }
            _ => {
                self.stats.albums_unclear += 1;
                self.stats.notes.push(format!("album unclear (its tracks are on several albums): {name}"));
                None
            }
        };
        self.albums.insert(album_id.to_string(), found);
        Ok(found)
    }

    /// The Acetate artist with a Navidrome artist's name (or an alias of it). If several
    /// have it, the one credited on the Navidrome artist's tracks.
    async fn artist(&mut self, conn: &mut PgConnection, artist_id: &str) -> anyhow::Result<Option<Uuid>> {
        if let Some(found) = self.artists.get(artist_id) {
            return Ok(*found);
        }
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM artist WHERE id = ?")
            .bind(artist_id)
            .fetch_optional(self.nd)
            .await?;
        let name = name.unwrap_or_default();
        let mut candidates = sqlx::query_scalar!(
            r#"
            SELECT DISTINCT id AS "id!" FROM (
                SELECT COALESCE(merged_into, id) AS id FROM artists WHERE name_lower = lower($1)
                UNION
                SELECT artist_id FROM artist_aliases WHERE alias_lower = lower($1)
            ) found
            "#,
            name
        )
        .fetch_all(&mut *conn)
        .await?;
        if candidates.len() > 1 {
            // Which of them is credited on some of this artist's tracks?
            let files: Vec<String> = sqlx::query_scalar(
                "SELECT media_file_id FROM media_file_artists WHERE artist_id = ? LIMIT ?",
            )
            .bind(artist_id)
            .bind(SAMPLE_FILES)
            .fetch_all(self.nd)
            .await
            .unwrap_or_default();
            let mut credited = Vec::new();
            for file in files {
                if let Some(track) = self.track(conn, &file).await? {
                    let on_track = sqlx::query_scalar!(
                        "SELECT artist_id FROM track_credits WHERE track_id = $1 AND artist_id = ANY($2)",
                        track,
                        &candidates
                    )
                    .fetch_all(&mut *conn)
                    .await?;
                    credited.extend(on_track);
                }
            }
            credited.sort();
            credited.dedup();
            if !credited.is_empty() {
                candidates = credited;
            }
        }
        let found = match candidates.as_slice() {
            [one] => {
                self.stats.artists_found += 1;
                Some(*one)
            }
            [] => {
                self.stats.artists_not_found += 1;
                self.stats.notes.push(format!("artist not found: {name}"));
                None
            }
            _ => {
                self.stats.artists_unclear += 1;
                self.stats.notes.push(format!("artist unclear (several with that name): {name}"));
                None
            }
        };
        self.artists.insert(artist_id.to_string(), found);
        Ok(found)
    }
}

/// Rewrites a path from Navidrome's point of view (e.g. inside its Docker container) to
/// this machine's, with the longest matching `--path-map` prefix.
fn map_path(path: &Path, maps: &[(String, PathBuf)]) -> PathBuf {
    let text = path.to_string_lossy();
    for (from, to) in maps {
        if let Some(rest) = text.strip_prefix(from.as_str())
            && (rest.is_empty() || rest.starts_with('/')) {
                return to.join(rest.trim_start_matches('/'));
            }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_like_navidrome() {
        // Encrypted the way Navidrome does it (AES-256-GCM, random nonce in front, base64),
        // with its default key.
        let key = Sha256::digest(DEFAULT_PASSWORD_KEY.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [7u8; 12];
        let mut data = nonce.to_vec();
        data.extend(cipher.encrypt((&nonce).into(), b"hunter22".as_slice()).unwrap());
        let stored = base64::engine::general_purpose::STANDARD.encode(data);

        assert_eq!(decrypt_password(&key, &stored).as_deref(), Some("hunter22"));
        let other = Sha256::digest(b"another key");
        assert_eq!(decrypt_password(&other, &stored), None);
        assert_eq!(decrypt_password(&key, "not base64!"), None);
    }

    #[test]
    fn paths_are_mapped_by_the_longest_prefix() {
        let maps = vec![
            ("/music/jail".to_string(), PathBuf::from("/srv/jail")),
            ("/music".to_string(), PathBuf::from("/home/me/Music")),
        ];
        assert_eq!(map_path(Path::new("/music/a/b.flac"), &maps), PathBuf::from("/home/me/Music/a/b.flac"));
        assert_eq!(map_path(Path::new("/music/jail/x.mp3"), &maps), PathBuf::from("/srv/jail/x.mp3"));
        // Only whole folder names: /musichd isn't /music.
        assert_eq!(map_path(Path::new("/musichd/x.mp3"), &maps), PathBuf::from("/musichd/x.mp3"));
    }

    #[test]
    fn times_and_raising() {
        let t = parse_time(Some("2026-08-24 12:46:49.070923065+00:00".into())).unwrap();
        assert_eq!(t.to_rfc3339(), "2026-08-24T12:46:49.070923065+00:00");
        let existing = Annotation { play_count: 10, rating: 6, ..Default::default() };
        let imported = Annotation { play_count: 4, rating: 10, starred: true, ..Default::default() };
        let raised = raise(&existing, &imported);
        assert_eq!((raised.play_count, raised.rating, raised.starred), (10, 6, true));
    }
}
