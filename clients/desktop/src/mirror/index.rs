//! What the mirror knows, in `<root>/mirror.sqlite`:
//!
//! - `have`: the blobs here, each with its size, how it came and when its hash was last
//!   checked (0: not yet: taken in from a folder by its path and size, without hashing).
//! - `mirrors`: each server mirrored (by its identity, `Mirror::server`; an earlier version used the address): what to keep (`Config`), and the
//!   version of the list last had from it.
//! - `listed`: that list: every blob the server has, its volume, size and state.
//! - `pins`: blobs kept for a reason of their own, mirror or not (offline downloads of
//!   single tracks will be these). Never offered for removal.
//!
//! What's *wanted* is worked out from these (the `wanted_now` view): what an enabled
//! mirror keeps (its states and volumes) of what its server lists, and anything pinned.
//! That's kept in the `wanted` table, made again whenever a list, a mirror's settings or a
//! pin changes (a fraction of a second for a big library), so asking what's missing or
//! removable is quick. Missing = wanted, not had. Removable = had, not wanted: offered,
//! never removed unasked.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// What a mirror keeps of a server's blobs. Off until turned on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    /// The server's volumes mirrored (by name). A blob on another isn't fetched.
    pub volumes: Vec<String>,
    /// Also tracks' previous files (replaced ones, kept on the server). Off by default:
    /// they can be many, and aren't played.
    pub previous: bool,
    /// Folders on this machine to look in for wanted blobs before downloading them (e.g.
    /// the music folder the library was imported from): taken in as reflinks.
    #[serde(deserialize_with = "lenient")]
    pub look_in: Vec<LookIn>,
    /// Hash each file taken in from a folder before it counts (else its path and size are
    /// trusted, and `rehash` checks it later). Downloads are always hashed as they come.
    pub check_hashes: bool,
}

/// A folder on this machine that has the same files as a folder the server imported from:
/// a blob imported from `<server_folder>/a/b.flac` is looked for at `<folder>/a/b.flac`,
/// and taken in if it's there with the right size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LookIn {
    pub folder: PathBuf,
    /// As the server has it (its paths, `/` or `\\` as they were).
    pub server_folder: String,
}

impl LookIn {
    /// Where a blob imported from `server_path` would be here, if it's under this folder.
    pub fn local_path(&self, server_path: &str) -> Option<PathBuf> {
        let (path, folder) = (server_path.replace('\\', "/"), self.server_folder.replace('\\', "/"));
        let folder = folder.trim_end_matches('/');
        let rest = path.strip_prefix(folder)?.strip_prefix('/')?;
        let mut local = self.folder.clone();
        for part in rest.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return None;
            }
            local.push(part);
        }
        Some(local)
    }
}

/// A setting that can't be read is left empty, not the whole configuration lost.
fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

pub use acetate_core::api::{Listed, ServerVolume};

/// A blob here that nothing wants anymore, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Removable {
    pub sha256: String,
    pub size: i64,
    /// Its state on the server ("trashed", "previous"…), "gone" if the server doesn't
    /// list it; None if no mirror of a server lists it at all.
    pub state: Option<String>,
    pub volume: Option<String>,
}

/// What a server has on one of its volumes, as listed: what a mirror would keep, and
/// the previous files besides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VolumeTotal {
    pub name: String,
    pub kept: Count,
    pub previous: Count,
}

/// Blobs here nothing wants, for one reason: their state on the server ("gone" if it
/// doesn't list them, None if no mirror of a server lists them at all) and volume.
#[derive(Debug, Clone, PartialEq)]
pub struct RemovableGroup {
    pub state: Option<String>,
    pub volume: Option<String>,
    pub count: Count,
}

/// How many blobs, and their bytes (of those whose size is known).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Count {
    pub blobs: u64,
    pub bytes: u64,
}

pub struct Index {
    conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS have (
    sha256      TEXT PRIMARY KEY,
    size        INTEGER NOT NULL,
    -- 'downloaded', 'reflinked', 'copied', or 'found' (in the blobs folder already).
    how         TEXT NOT NULL,
    added_at    INTEGER NOT NULL,
    verified_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS mirrors (
    server         TEXT PRIMARY KEY,
    config         TEXT NOT NULL,
    version        TEXT,
    server_volumes TEXT NOT NULL DEFAULT '[]',
    synced_at      INTEGER
);
CREATE TABLE IF NOT EXISTS listed (
    server TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    volume TEXT,
    size   INTEGER,
    path   TEXT,
    state  TEXT NOT NULL,
    PRIMARY KEY (server, sha256)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS listed_sha256 ON listed (sha256);
CREATE TABLE IF NOT EXISTS pins (
    sha256 TEXT NOT NULL,
    reason TEXT NOT NULL,
    PRIMARY KEY (sha256, reason)
) WITHOUT ROWID;
DROP TABLE IF EXISTS scanned;
CREATE TABLE IF NOT EXISTS wanted (
    sha256 TEXT PRIMARY KEY,
    size   INTEGER
) WITHOUT ROWID;
DROP VIEW IF EXISTS wanted_now;
CREATE VIEW wanted_now AS
    SELECT l.sha256, max(l.size) AS size
    FROM listed l JOIN mirrors m ON m.server = l.server
    WHERE json_extract(m.config, '$.enabled')
      AND (l.state IN ('audio', 'extra', 'cover', 'not_imported')
           OR (l.state = 'previous' AND json_extract(m.config, '$.previous')))
      AND l.volume IN (SELECT value FROM json_each(m.config, '$.volumes'))
    GROUP BY l.sha256
    UNION
    SELECT sha256, NULL FROM pins;
"#;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Index {
    pub fn open(path: &Path) -> anyhow::Result<Index> {
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        // WAL: the app reading while a sync writes doesn't wait.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        // `wanted` was a view before it was kept as a table.
        let was_view: bool = conn.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'wanted' AND type = 'view'", [], |r| r.get(0))?;
        if was_view {
            conn.execute_batch("DROP VIEW wanted")?;
        }
        conn.execute_batch(SCHEMA)?;
        // An index from before paths were listed: the column added, and the whole list
        // asked for again to fill it in.
        let has_path: bool = conn.query_row("SELECT count(*) FROM pragma_table_info('listed') WHERE name = 'path'", [], |r| r.get(0))?;
        if !has_path {
            conn.execute_batch("ALTER TABLE listed ADD COLUMN path TEXT; UPDATE mirrors SET version = NULL;")?;
        }
        let index = Index { conn };
        index.rebuild_wanted()?;
        Ok(index)
    }

    /// A second connection that only reads (for showing what's there): with the WAL,
    /// reading never waits for the mirror's writing, nor holds it up.
    pub fn open_reader(path: &Path) -> anyhow::Result<Index> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        Ok(Index { conn })
    }

    /// Works out what's wanted again (see the module's notes).
    fn rebuild_wanted(&self) -> anyhow::Result<()> {
        self.conn.execute_batch("DELETE FROM wanted; INSERT INTO wanted (sha256, size) SELECT sha256, max(size) FROM wanted_now GROUP BY sha256;")?;
        Ok(())
    }

    // ---- Mirrors ----

    /// What's kept under the name `from` (a server's address, before servers had an
    /// identity) is kept under `to`. If `to` has its own (it shouldn't), it's replaced: the
    /// lists are only copies of what the server says, and fetched again.
    pub fn rekey(&mut self, from: &str, to: &str) -> anyhow::Result<()> {
        if from == to {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let has: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM mirrors WHERE server = ?1)", [from], |r| r.get(0))?;
        if has {
            tx.execute("DELETE FROM listed WHERE server = ?1", [to])?;
            tx.execute("DELETE FROM mirrors WHERE server = ?1", [to])?;
            tx.execute("UPDATE listed SET server = ?2 WHERE server = ?1", [from, to])?;
            tx.execute("UPDATE mirrors SET server = ?2 WHERE server = ?1", [from, to])?;
        }
        tx.commit()?;
        self.rebuild_wanted()
    }

    pub fn config(&self, server: &str) -> anyhow::Result<Config> {
        let config: Option<String> =
            self.conn.query_row("SELECT config FROM mirrors WHERE server = ?1", [server], |r| r.get(0)).optional()?;
        Ok(config.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default())
    }

    pub fn set_config(&mut self, server: &str, config: &Config) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO mirrors (server, config) VALUES (?1, ?2) ON CONFLICT (server) DO UPDATE SET config = excluded.config",
            params![server, serde_json::to_string(config)?],
        )?;
        tx.commit()?;
        self.rebuild_wanted()
    }

    /// The version of the list last had from `server` (to ask what changed since).
    pub fn version(&self, server: &str) -> anyhow::Result<Option<String>> {
        Ok(self.conn.query_row("SELECT version FROM mirrors WHERE server = ?1", [server], |r| r.get(0)).optional()?.flatten())
    }

    /// The server's volumes as last listed.
    pub fn server_volumes(&self, server: &str) -> anyhow::Result<Vec<ServerVolume>> {
        let json: Option<String> =
            self.conn.query_row("SELECT server_volumes FROM mirrors WHERE server = ?1", [server], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())
    }

    /// When the list was last brought up to date (seconds since 1970).
    pub fn synced_at(&self, server: &str) -> anyhow::Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT synced_at FROM mirrors WHERE server = ?1", [server], |r| r.get(0)).optional()?.flatten())
    }

    /// Takes in a list from `server`: the whole list (`full`: what isn't in it is gone),
    /// or what changed (a blob "gone" is dropped). In one transaction, with the version.
    pub fn apply_listing(
        &mut self,
        server: &str,
        version: &str,
        full: bool,
        volumes: &[ServerVolume],
        blobs: &[Listed],
    ) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO mirrors (server, config) VALUES (?1, ?2) ON CONFLICT (server) DO NOTHING",
            params![server, serde_json::to_string(&Config::default())?],
        )?;
        if full {
            tx.execute("DELETE FROM listed WHERE server = ?1", [server])?;
        }
        {
            let mut upsert = tx.prepare(
                "INSERT INTO listed (server, sha256, volume, size, path, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (server, sha256) DO UPDATE SET volume = excluded.volume, size = excluded.size,
                     path = excluded.path, state = excluded.state",
            )?;
            let mut drop = tx.prepare("DELETE FROM listed WHERE server = ?1 AND sha256 = ?2")?;
            for blob in blobs {
                if blob.state == "gone" {
                    drop.execute(params![server, blob.sha256])?;
                } else {
                    upsert.execute(params![server, blob.sha256, blob.volume, blob.size, blob.path, blob.state])?;
                }
            }
        }
        tx.execute(
            "UPDATE mirrors SET version = ?2, server_volumes = ?3, synced_at = ?4 WHERE server = ?1",
            params![server, version, serde_json::to_string(volumes)?, now()],
        )?;
        tx.commit()?;
        self.rebuild_wanted()
    }

    // ---- What's here ----

    /// A blob here whose hash starts so (a cover's key: the start of its image's hash).
    pub fn find_by_start(&self, start: &str) -> anyhow::Result<Option<String>> {
        // (Hex: every hash starting so sorts between it and it followed by "g".)
        let found = self.conn.query_row("SELECT sha256 FROM have WHERE sha256 >= ?1 AND sha256 < ?1 || 'g' LIMIT 1", [start], |r| r.get(0)).optional()?;
        Ok(found)
    }

    pub fn has(&self, sha256: &str) -> anyhow::Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM have WHERE sha256 = ?1", [sha256], |_| Ok(())).optional()?.is_some())
    }

    /// A blob came in: its hash checked, or (`checked` false) taken by its path and size.
    pub fn add(&self, sha256: &str, size: u64, how: &str, checked: bool) -> anyhow::Result<()> {
        let at = now();
        self.conn.execute(
            "INSERT INTO have (sha256, size, how, added_at, verified_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (sha256) DO UPDATE SET size = excluded.size, verified_at = excluded.verified_at",
            params![sha256, size as i64, how, at, if checked { at } else { 0 }],
        )?;
        Ok(())
    }

    /// Blobs that came in (`add`), in one transaction: (sha256, size, how).
    pub fn add_many(&mut self, blobs: &[(String, u64, &str)], checked: bool) -> anyhow::Result<()> {
        let at = now();
        let tx = self.conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO have (sha256, size, how, added_at, verified_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (sha256) DO UPDATE SET size = excluded.size, verified_at = excluded.verified_at",
            )?;
            for (sha256, size, how) in blobs {
                insert.execute(params![sha256, *size as i64, how, at, if checked { at } else { 0 }])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Blobs here whose hash wasn't checked yet.
    pub fn unchecked(&self) -> anyhow::Result<Count> {
        self.count("SELECT count(*), coalesce(sum(size), 0) FROM have WHERE verified_at = 0")
    }

    /// The size a server lists a blob with (any server), if one does.
    pub fn listed_size(&self, sha256: &str) -> anyhow::Result<Option<u64>> {
        let size: Option<Option<i64>> =
            self.conn.query_row("SELECT max(size) FROM listed WHERE sha256 = ?1", [sha256], |r| r.get(0)).optional()?;
        Ok(size.flatten().map(|s| s as u64))
    }

    /// Every blob `server` lists with a path: (path, size).
    pub fn listed_paths(&self, server: &str) -> anyhow::Result<Vec<(String, u64)>> {
        let mut query = self.conn.prepare("SELECT path, size FROM listed WHERE server = ?1 AND path IS NOT NULL AND size IS NOT NULL")?;
        let rows = query.query_map([server], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Missing blobs with a known path and size: (sha256, size, path where imported from).
    pub fn missing_with_paths(&self) -> anyhow::Result<Vec<(String, u64, String)>> {
        let mut query = self.conn.prepare(
            "SELECT w.sha256, w.size, (SELECT l.path FROM listed l WHERE l.sha256 = w.sha256 AND l.path IS NOT NULL LIMIT 1) AS path
             FROM wanted w
             WHERE w.size IS NOT NULL AND path IS NOT NULL AND NOT EXISTS (SELECT 1 FROM have h WHERE h.sha256 = w.sha256)",
        )?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, String>(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Its hash was checked again, and is right.
    pub fn verified(&self, sha256: &str) -> anyhow::Result<()> {
        self.conn.execute("UPDATE have SET verified_at = ?2 WHERE sha256 = ?1", params![sha256, now()])?;
        Ok(())
    }

    pub fn forget(&self, sha256: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM have WHERE sha256 = ?1", [sha256])?;
        Ok(())
    }

    /// Every blob here: (sha256, size).
    pub fn all_had(&self) -> anyhow::Result<Vec<(String, u64)>> {
        let mut query = self.conn.prepare("SELECT sha256, size FROM have ORDER BY sha256")?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn had(&self) -> anyhow::Result<Count> {
        self.count("SELECT count(*), coalesce(sum(size), 0) FROM have")
    }

    fn count(&self, sql: &str) -> anyhow::Result<Count> {
        Ok(self.conn.query_row(sql, [], |r| Ok(Count { blobs: r.get::<_, i64>(0)? as u64, bytes: r.get::<_, i64>(1)? as u64 }))?)
    }

    // ---- Wanted, missing, removable ----

    pub fn wanted(&self) -> anyhow::Result<Count> {
        self.count("SELECT count(*), coalesce(sum(size), 0) FROM wanted")
    }

    /// Wanted and not here: (sha256, size if known), smallest first (covers before
    /// albums' worth of audio, so a mirror is useful soonest).
    pub fn missing(&self) -> anyhow::Result<Vec<(String, Option<u64>)>> {
        let mut query = self.conn.prepare(
            "SELECT w.sha256, w.size FROM wanted w WHERE NOT EXISTS (SELECT 1 FROM have h WHERE h.sha256 = w.sha256)
             ORDER BY w.size IS NULL, w.size, w.sha256",
        )?;
        let rows = query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?.map(|s| s as u64))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What's missing, in all: (blobs and bytes, blobs of unknown size).
    pub fn missing_totals(&self) -> anyhow::Result<(Count, u64)> {
        Ok(self.conn.query_row(
            "SELECT count(*), coalesce(sum(w.size), 0), count(*) FILTER (WHERE w.size IS NULL)
             FROM wanted w WHERE NOT EXISTS (SELECT 1 FROM have h WHERE h.sha256 = w.sha256)",
            [],
            |r| Ok((Count { blobs: r.get::<_, i64>(0)? as u64, bytes: r.get::<_, i64>(1)? as u64 }, r.get::<_, i64>(2)? as u64)),
        )?)
    }

    /// Per volume of `server`: what a mirror keeps of it, and its previous files.
    pub fn volume_totals(&self, server: &str) -> anyhow::Result<Vec<VolumeTotal>> {
        let mut query = self.conn.prepare(
            "SELECT volume,
                    count(*) FILTER (WHERE state IN ('audio', 'extra', 'cover', 'not_imported')),
                    coalesce(sum(size) FILTER (WHERE state IN ('audio', 'extra', 'cover', 'not_imported')), 0),
                    count(*) FILTER (WHERE state = 'previous'),
                    coalesce(sum(size) FILTER (WHERE state = 'previous'), 0)
             FROM listed WHERE server = ?1 AND volume IS NOT NULL GROUP BY volume ORDER BY volume",
        )?;
        let rows = query.query_map([server], |r| {
            Ok(VolumeTotal {
                name: r.get(0)?,
                kept: Count { blobs: r.get::<_, i64>(1)? as u64, bytes: r.get::<_, i64>(2)? as u64 },
                previous: Count { blobs: r.get::<_, i64>(3)? as u64, bytes: r.get::<_, i64>(4)? as u64 },
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What `removable` lists, grouped by why.
    pub fn removable_groups(&self, server: &str) -> anyhow::Result<Vec<RemovableGroup>> {
        let mut groups: Vec<RemovableGroup> = Vec::new();
        for r in self.removable(server)? {
            let size = r.size.max(0) as u64;
            match groups.iter_mut().find(|g| g.state == r.state && g.volume == r.volume) {
                Some(group) => {
                    group.count.blobs += 1;
                    group.count.bytes += size;
                }
                None => groups.push(RemovableGroup { state: r.state, volume: r.volume, count: Count { blobs: 1, bytes: size } }),
            }
        }
        Ok(groups)
    }

    /// Here and not wanted, with why (as `server` lists it, if it does).
    pub fn removable(&self, server: &str) -> anyhow::Result<Vec<Removable>> {
        let mut query = self.conn.prepare(
            "SELECT h.sha256, h.size,
                    CASE WHEN l.state IS NOT NULL THEN l.state
                         WHEN EXISTS (SELECT 1 FROM mirrors WHERE server = ?1) THEN 'gone' END,
                    l.volume
             FROM have h
             LEFT JOIN listed l ON l.server = ?1 AND l.sha256 = h.sha256
             WHERE NOT EXISTS (SELECT 1 FROM wanted w WHERE w.sha256 = h.sha256)
             ORDER BY h.sha256",
        )?;
        let rows = query.query_map([server], |r| {
            Ok(Removable { sha256: r.get(0)?, size: r.get(1)?, state: r.get(2)?, volume: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Whether a blob is wanted (checked again right before removing it).
    pub fn is_wanted(&self, sha256: &str) -> anyhow::Result<bool> {
        Ok(self.conn.query_row("SELECT 1 FROM wanted WHERE sha256 = ?1", [sha256], |_| Ok(())).optional()?.is_some())
    }

    // ---- Pins ----

    // For offline downloads of single tracks (next): kept wanted whatever the mirror says.
    #[allow(dead_code)]
    pub fn pin(&self, sha256: &str, reason: &str) -> anyhow::Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO pins (sha256, reason) VALUES (?1, ?2)", params![sha256, reason])?;
        self.rebuild_wanted()
    }

    #[allow(dead_code)]
    pub fn unpin(&self, sha256: &str, reason: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM pins WHERE sha256 = ?1 AND reason = ?2", params![sha256, reason])?;
        self.rebuild_wanted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn listed(c: char, volume: &str, size: i64, state: &str) -> Listed {
        Listed { sha256: sha(c), volume: Some(volume.into()), size: Some(size), path: None, state: state.into() }
    }

    #[test]
    fn a_blob_is_found_by_the_start_of_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let index = Index::open(&dir.path().join("mirror.sqlite")).unwrap();
        index.add(&sha('a'), 1, "downloaded", true).unwrap();
        index.add(&sha('b'), 1, "downloaded", true).unwrap();
        assert_eq!(index.find_by_start(&sha('b')[..16]).unwrap(), Some(sha('b')));
        assert_eq!(index.find_by_start(&sha('c')[..16]).unwrap(), None);
    }

    #[test]
    fn a_servers_mirror_moves_to_its_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = Index::open(&dir.path().join("mirror.sqlite")).unwrap();
        let volumes = [ServerVolume { name: "main".into(), default: true }];
        index.apply_listing("http://nas:3000", "v1", true, &volumes, &[listed('a', "main", 10, "audio")]).unwrap();
        index.set_config("http://nas:3000", &Config { enabled: true, volumes: vec!["main".into()], ..Config::default() }).unwrap();
        index.rekey("http://nas:3000", "the-id").unwrap();
        assert!(index.config("the-id").unwrap().enabled);
        assert_eq!(index.version("the-id").unwrap().as_deref(), Some("v1"));
        assert_eq!(index.listed_size(&sha('a')).unwrap(), Some(10));
        assert!(!index.config("http://nas:3000").unwrap().enabled);
        assert_eq!(index.version("http://nas:3000").unwrap(), None);
        // Again, and to itself: nothing changes.
        index.rekey("http://nas:3000", "the-id").unwrap();
        index.rekey("the-id", "the-id").unwrap();
        assert!(index.config("the-id").unwrap().enabled);
    }

    #[test]
    fn wanted_is_what_the_mirror_keeps_and_nothing_is_removed_unasked() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = Index::open(&dir.path().join("mirror.sqlite")).unwrap();
        let server = "http://nas:3000";
        let volumes = [ServerVolume { name: "main".into(), default: true }, ServerVolume { name: "archive".into(), default: false }];
        index
            .apply_listing(server, "v1", true, &volumes, &[
                listed('a', "main", 10, "audio"),
                listed('b', "main", 20, "cover"),
                listed('c', "archive", 30, "audio"),
                listed('d', "main", 40, "previous"),
                listed('e', "main", 50, "trashed"),
                listed('f', "main", 60, "not_imported"),
            ])
            .unwrap();
        assert_eq!(index.version(server).unwrap().as_deref(), Some("v1"));
        assert_eq!(index.server_volumes(server).unwrap(), volumes);

        // Not enabled yet: nothing wanted.
        assert_eq!(index.wanted().unwrap(), Count::default());

        index.set_config(server, &Config { enabled: true, volumes: vec!["main".into()], ..Config::default() }).unwrap();
        let missing: Vec<_> = index.missing().unwrap().into_iter().map(|(s, _)| s).collect();
        assert_eq!(missing, [sha('a'), sha('b'), sha('f')], "kept states, mirrored volume, smallest first");

        index.set_config(server, &Config { enabled: true, volumes: vec!["main".into(), "archive".into()], previous: true, ..Config::default() }).unwrap();
        assert_eq!(index.wanted().unwrap(), Count { blobs: 5, bytes: 160 });

        // Had: a, d. Then d's file is trashed on the server and a is purged: offered, not removed.
        index.add(&sha('a'), 10, "downloaded", true).unwrap();
        index.add(&sha('d'), 40, "reflinked", false).unwrap();
        assert_eq!(index.unchecked().unwrap(), Count { blobs: 1, bytes: 40 });
        assert!(index.removable(server).unwrap().is_empty());
        index.apply_listing(server, "v2", false, &volumes, &[
            listed('d', "main", 40, "trashed"),
            Listed { sha256: sha('a'), volume: None, size: None, path: None, state: "gone".into() },
        ]).unwrap();
        let removable = index.removable(server).unwrap();
        let why: Vec<_> = removable.iter().map(|r| (r.sha256.clone(), r.state.clone())).collect();
        assert_eq!(why, [(sha('a'), Some("gone".into())), (sha('d'), Some("trashed".into()))]);
        assert!(index.has(&sha('a')).unwrap(), "still here until removed");

        // A pin keeps one wanted whatever the mirror says.
        index.pin(&sha('a'), "offline").unwrap();
        assert!(index.is_wanted(&sha('a')).unwrap());
        assert_eq!(index.removable(server).unwrap().len(), 1);
        index.unpin(&sha('a'), "offline").unwrap();

        // The whole list again: anything not in it is gone.
        index.apply_listing(server, "v3", true, &volumes, &[listed('b', "main", 20, "cover")]).unwrap();
        assert_eq!(index.wanted().unwrap(), Count { blobs: 1, bytes: 20 });
    }

    #[test]
    fn a_folder_here_stands_for_a_folder_on_the_server() {
        let look = LookIn { folder: PathBuf::from("/home/me/Music"), server_folder: "/srv/music/".into() };
        assert_eq!(look.local_path("/srv/music/Artist/01 a.flac"), Some(PathBuf::from("/home/me/Music/Artist/01 a.flac")));
        assert_eq!(look.local_path("/srv/musicals/x.flac"), None, "a whole folder name, not a prefix");
        assert_eq!(look.local_path("/srv/music/../etc/passwd"), None);
        let windows = LookIn { folder: PathBuf::from("/mnt/d"), server_folder: "D:\\Music".into() };
        assert_eq!(windows.local_path("D:\\Music\\A\\b.mp3"), Some(PathBuf::from("/mnt/d/A/b.mp3")));
    }

    #[test]
    fn a_setting_that_cant_be_read_doesnt_lose_the_others() {
        let config: Config = serde_json::from_str(r#"{"enabled": true, "volumes": ["main"], "look_in": ["/old/form"]}"#).unwrap();
        assert!(config.enabled && config.look_in.is_empty());
    }
}

/// Timing at scale: `cargo test --release mirror_index_at_scale -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn mirror_index_at_scale() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = Index::open(&dir.path().join("mirror.sqlite")).unwrap();
        let server = "http://nas:3000";
        let blobs: Vec<Listed> = (0..200_000u64)
            .map(|i| Listed {
                sha256: format!("{:064x}", i * 7919),
                volume: Some(if i % 10 == 0 { "archive" } else { "main" }.into()),
                size: Some(1_000_000 + i as i64),
                path: Some(format!("/srv/music/artist {}/album {}/{i:05} track.flac", i / 300, i / 12)),
                state: if i % 4 == 0 { "cover" } else { "audio" }.into(),
            })
            .collect();
        let t = std::time::Instant::now();
        index.apply_listing(server, "v1", true, &[], &blobs).unwrap();
        println!("apply 200k: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        index.set_config(server, &Config { enabled: true, volumes: vec!["main".into()], ..Config::default() }).unwrap();
        println!("set_config (wanted made again): {:?}", t.elapsed());
        for b in blobs.iter().take(100_000) {
            index.add(&b.sha256, 1, "downloaded", true).unwrap();
        }
        let time = |name: &str, f: &dyn Fn()| {
            let t = std::time::Instant::now();
            f();
            println!("{name}: {:?}", t.elapsed());
        };
        time("wanted", &|| { let _ = index.wanted().unwrap(); });
        time("had", &|| { let _ = index.had().unwrap(); });
        time("missing_totals", &|| { let _ = index.missing_totals().unwrap(); });
        time("missing", &|| drop(index.missing().unwrap()));
        time("missing_with_paths", &|| drop(index.missing_with_paths().unwrap()));
        time("removable_groups", &|| drop(index.removable_groups(server).unwrap()));
        time("volume_totals", &|| drop(index.volume_totals(server).unwrap()));
        time("unchecked", &|| { let _ = index.unchecked().unwrap(); });
        let t = std::time::Instant::now();
        let server_ = server;
        {
            let groups = index.removable_groups(server_).unwrap();
            let _ = (index.wanted().unwrap(), index.had().unwrap(), index.missing_totals().unwrap(), index.unchecked().unwrap(), index.volume_totals(server_).unwrap(), groups);
        }
        println!("a whole snapshot's queries: {:?}", t.elapsed());
        time("100 adds", &|| for b in blobs.iter().skip(150_000).take(100) { index.add(&b.sha256, 1, "reflinked", false).unwrap() });
    }
}
