//! `acetate verify`: checks, without changing anything, that files are safely in Acetate.
//!
//! * `acetate verify <dir>`: for every file below `<dir>` (audio or not), is the same
//!   content stored in Acetate? Meant for before deleting an old library by hand: only
//!   files reported as stored are safe to delete. Nothing under `<dir>` is ever written,
//!   moved or deleted.
//! * `acetate verify --blobs`: re-hashes every blob the database uses, and lists blobs
//!   that are missing, damaged, or that nothing uses anymore.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use sqlx::PgPool;
use tokio::task::JoinSet;

use crate::storage::{self, BlobStore};

/// How many files are hashed at the same time (disks read faster with a few requests
/// in flight).
const PARALLEL_HASHES: usize = 8;
/// Hashes looked up in the database per query.
const LOOKUP_BATCH: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    /// Stored as a track's file (current or previous), and its blob is there.
    InLibrary,
    /// Kept for the Review page (it didn't import), and its blob is there.
    ForReview,
    /// Not stored.
    Missing,
    /// The database has it, but the blob is gone or has the wrong size.
    BlobDamaged,
}

pub struct Options {
    /// Write the paths of files that are safely stored to this file (one per line).
    pub list_ok: Option<PathBuf>,
    /// Write the paths of files that aren't to this file.
    pub list_missing: Option<PathBuf>,
}

pub async fn verify_dir(pool: &PgPool, store: &BlobStore, dir: &Path, options: Options) -> anyhow::Result<()> {
    let root = std::fs::canonicalize(dir).with_context(|| format!("opening {}", dir.display()))?;
    // The folder asked for must be readable (else there's nothing to check); anything
    // below it that isn't is skipped with a warning, and reported.
    std::fs::read_dir(&root).with_context(|| format!("reading {}", root.display()))?;
    let mut files = Vec::new();
    let mut unreadable_dirs = Vec::new();
    list_files(&root, &mut files, &mut unreadable_dirs);
    files.sort();
    println!("{} files in {} (hashing, this reads every file once)", files.len(), root.display());

    let hashed = hash_all(files).await;
    let mut results: Vec<(PathBuf, Status)> = Vec::new();
    let mut unreadable = Vec::new();
    let mut ok_hashes = Vec::new();
    for (path, result) in hashed {
        match result {
            Ok((sha, size)) => ok_hashes.push((path, sha, size)),
            Err(err) => unreadable.push((path, err.to_string())),
        }
    }

    for batch in ok_hashes.chunks(LOOKUP_BATCH) {
        let shas: Vec<String> = batch.iter().map(|(_, sha, _)| sha.clone()).collect();
        let found = lookup(pool, &shas).await?;
        for ((path, sha, size), (in_library, for_review)) in batch.iter().zip(found) {
            let status = if !in_library && !for_review {
                Status::Missing
            } else if store.size(sha).await != Some(*size) {
                Status::BlobDamaged
            } else if in_library {
                Status::InLibrary
            } else {
                Status::ForReview
            };
            results.push((path.clone(), status));
        }
    }

    // Hashing finishes in any order; list by path.
    results.sort();
    unreadable.sort();
    unreadable_dirs.sort();
    report(&root, &results, &unreadable, &unreadable_dirs);
    let ok = |s: Status| matches!(s, Status::InLibrary | Status::ForReview);
    if let Some(path) = &options.list_ok {
        write_list(path, results.iter().filter(|(_, s)| ok(*s)).map(|(p, _)| p))?;
        println!("safely stored files listed in {}", path.display());
    }
    if let Some(path) = &options.list_missing {
        let not_ok = results.iter().filter(|(_, s)| !ok(*s)).map(|(p, _)| p);
        let skipped = unreadable.iter().chain(&unreadable_dirs).map(|(p, _)| p);
        write_list(path, not_ok.chain(skipped))?;
        println!("files not stored listed in {}", path.display());
    }
    Ok(())
}

/// For each hash: (stored as a track's file, kept for review). As the owner (the CLI's
/// pool), so everything counts, hidden tracks too.
async fn lookup(pool: &PgPool, shas: &[String]) -> anyhow::Result<Vec<(bool, bool)>> {
    let rows = sqlx::query!(
        r#"
        SELECT h.n AS "n!",
               (EXISTS (SELECT 1 FROM files f WHERE f.sha256 = h.sha256 AND f.purged_at IS NULL)
                -- An extra's file (a stem, a sample…) is stored too.
                OR EXISTS (SELECT 1 FROM extra_files x WHERE x.sha256 = h.sha256)) AS "in_library!",
               EXISTS (SELECT 1 FROM ingest_problems p
                       WHERE p.sha256 = h.sha256 AND p.resolved_at IS NULL) AS "for_review!"
        FROM unnest($1::text[]) WITH ORDINALITY AS h(sha256, n)
        ORDER BY h.n
        "#,
        shas
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.in_library, r.for_review)).collect())
}

fn report(root: &Path, results: &[(PathBuf, Status)], unreadable: &[(PathBuf, String)], unreadable_dirs: &[(PathBuf, String)]) {
    let count = |status: Status| results.iter().filter(|(_, s)| *s == status).count();
    println!();
    println!("stored in Acetate:        {}", count(Status::InLibrary));
    println!("kept for review:          {}  (on the Review page: didn't import, audio kept)", count(Status::ForReview));
    println!("NOT stored:               {}", count(Status::Missing));
    println!("stored, blob damaged:     {}", count(Status::BlobDamaged));
    println!("couldn't be read:         {}", unreadable.len());
    if !unreadable_dirs.is_empty() {
        println!("folders that couldn't be read: {}  (the files in them weren't checked at all)", unreadable_dirs.len());
    }

    let not_ok: Vec<&PathBuf> = results
        .iter()
        .filter(|(_, s)| matches!(s, Status::Missing | Status::BlobDamaged))
        .map(|(p, _)| p)
        .chain(unreadable.iter().map(|(p, _)| p))
        .collect();
    if not_ok.is_empty() && unreadable_dirs.is_empty() {
        println!("\nEverything here is stored in Acetate.");
        return;
    }
    // What kinds of files are missing: often covers, lyrics files, playlists...
    let mut by_extension: BTreeMap<String, usize> = BTreeMap::new();
    for path in &not_ok {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "(no extension)".to_string());
        *by_extension.entry(ext).or_default() += 1;
    }
    println!("\nnot stored, by extension:");
    for (ext, n) in &by_extension {
        println!("  {ext:<16} {n}");
    }
    println!("\nnot stored (first 50; --list-missing <file> for all):");
    for path in not_ok.iter().take(50) {
        let shown = path.strip_prefix(root).unwrap_or(path);
        println!("  {}", shown.display());
    }
    for (path, err) in unreadable.iter().take(10) {
        println!("  couldn't read {}: {err}", path.display());
    }
    for (path, err) in unreadable_dirs.iter().take(20) {
        println!("  couldn't read the folder {}: {err}", path.display());
    }
    for (path, _) in results.iter().filter(|(_, s)| *s == Status::BlobDamaged).take(10) {
        println!("  DAMAGED: {} is in the database but its blob is missing or the wrong size", path.display());
    }
}

/// `acetate verify --blobs`: every blob the database uses is there (on some volume) and
/// has the right content, and is recorded on the volume it's really on; blobs nothing uses
/// are listed. Nothing is deleted or moved.
pub async fn verify_blobs(pool: &PgPool, store: &BlobStore) -> anyhow::Result<()> {
    let used: HashSet<String> = sqlx::query_scalar!(
        r#"
        SELECT sha256 AS "sha256!" FROM files WHERE purged_at IS NULL
        UNION SELECT blob_sha256 FROM covers
        UNION SELECT sha256 FROM ingest_problems WHERE resolved_at IS NULL
        UNION SELECT sha256 FROM extra_files
        UNION SELECT thumb_sha256 FROM extra_files WHERE thumb_sha256 IS NOT NULL
        "#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    // Which volume each blob is recorded on.
    let recorded: HashMap<String, String> = sqlx::query!("SELECT sha256, volume FROM blobs")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.sha256, r.volume))
        .collect();

    // What's on each volume's disk: (volume, path).
    let mut on_disk: Vec<(String, PathBuf)> = Vec::new();
    for volume in store.volumes() {
        let mut files = Vec::new();
        let mut unreadable = Vec::new();
        list_files(volume.blobs_dir(), &mut files, &mut unreadable);
        if !unreadable.is_empty() {
            println!("volume {}: {} folders couldn't be read (their blobs weren't checked)", volume.name, unreadable.len());
        }
        println!("volume {}: {} blobs in {}", volume.name, files.len(), volume.blobs_dir().display());
        on_disk.extend(files.into_iter().map(|p| (volume.name.clone(), p)));
    }
    println!("{} blobs in use (re-hashing them all)", used.len());

    let name_of = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let found: HashMap<String, String> = on_disk.iter().map(|(v, p)| (name_of(p), v.clone())).collect();
    let stray: Vec<&PathBuf> = on_disk.iter().filter(|(_, p)| !used.contains(&name_of(p))).map(|(_, p)| p).collect();
    let used_on_disk: Vec<PathBuf> = on_disk.iter().filter(|(_, p)| used.contains(&name_of(p))).map(|(_, p)| p.clone()).collect();
    let mut missing: Vec<&String> = used.iter().filter(|sha| !found.contains_key(*sha)).collect();
    missing.sort();
    let mut misrecorded = Vec::new();
    for sha in &used {
        match (recorded.get(sha), found.get(sha)) {
            (None, Some(on)) => misrecorded.push(format!("{sha}: not recorded (it's on {on})")),
            (Some(rec), Some(on)) if rec != on => misrecorded.push(format!("{sha}: recorded on {rec}, but it's on {on}")),
            _ => {}
        }
    }
    misrecorded.sort();

    let mut damaged = Vec::new();
    for (path, result) in hash_all(used_on_disk).await {
        let name = name_of(&path);
        match result {
            Ok((sha, _)) if sha == name => {}
            Ok((sha, _)) => damaged.push(format!("{}: content hashes to {sha}", path.display())),
            Err(err) => damaged.push(format!("{}: {err}", path.display())),
        }
    }

    println!("\nmissing (used, on no volume): {}", missing.len());
    for sha in missing.iter().take(50) {
        println!("  {sha}");
    }
    println!("damaged (content doesn't match its name): {}", damaged.len());
    for line in damaged.iter().take(50) {
        println!("  {line}");
    }
    println!("recorded on the wrong volume, or not recorded: {}", misrecorded.len());
    for line in misrecorded.iter().take(50) {
        println!("  {line}");
    }
    println!("unused (nothing refers to them; not deleted): {}", stray.len());
    for path in stray.iter().take(50) {
        println!("  {}", path.display());
    }
    Ok(())
}

/// Hashes files, a few at a time. The results come back in any order.
async fn hash_all(paths: Vec<PathBuf>) -> Vec<(PathBuf, std::io::Result<(String, u64)>)> {
    let total = paths.len();
    let mut results = Vec::with_capacity(total);
    let mut queue = paths.into_iter();
    let mut running = JoinSet::new();
    loop {
        while running.len() < PARALLEL_HASHES {
            let Some(path) = queue.next() else { break };
            running.spawn_blocking(move || {
                let result = storage::hash_file(&path);
                (path, result)
            });
        }
        let Some(done) = running.join_next().await else { break };
        match done {
            Ok(result) => results.push(result),
            Err(err) => tracing::error!("a hashing task failed: {err}"),
        }
        if results.len() % 5000 == 0 {
            println!("  {}/{total}", results.len());
        }
    }
    results
}

/// All files below `dir`, recursively. Symlinked folders aren't followed. A folder (or
/// an entry) that can't be read is skipped with a warning and added to `unreadable`:
/// one unreadable folder doesn't stop the rest.
fn list_files(dir: &Path, out: &mut Vec<PathBuf>, unreadable: &mut Vec<(PathBuf, String)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("warning: can't read the folder {} ({err}); skipping it", dir.display());
            unreadable.push((dir.to_path_buf(), err.to_string()));
            return;
        }
    };
    for entry in entries {
        let kind = entry.and_then(|e| Ok((e.file_type()?, e.path())));
        match kind {
            Ok((kind, path)) if kind.is_dir() => list_files(&path, out, unreadable),
            Ok((kind, path)) if kind.is_file() || kind.is_symlink() => out.push(path),
            Ok(_) => {}
            Err(err) => {
                eprintln!("warning: can't read an entry in {} ({err}); skipping it", dir.display());
                unreadable.push((dir.to_path_buf(), err.to_string()));
            }
        }
    }
}

fn write_list<'a>(path: &Path, paths: impl Iterator<Item = &'a PathBuf>) -> anyhow::Result<()> {
    let mut text = String::new();
    for p in paths {
        text.push_str(&p.display().to_string());
        text.push('\n');
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn an_unreadable_folder_is_skipped_and_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ok")).unwrap();
        std::fs::create_dir_all(dir.path().join("locked/inner")).unwrap();
        std::fs::write(dir.path().join("ok/a.txt"), "a").unwrap();
        std::fs::write(dir.path().join("locked/inner/b.txt"), "b").unwrap();
        std::fs::set_permissions(dir.path().join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();

        let (mut files, mut unreadable) = (Vec::new(), Vec::new());
        super::list_files(dir.path(), &mut files, &mut unreadable);
        // (As root the folder is readable anyway: then nothing is skipped.)
        let running_as_root = std::fs::read_dir(dir.path().join("locked")).is_ok();
        std::fs::set_permissions(dir.path().join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(files.contains(&dir.path().join("ok/a.txt")), "the rest is still listed");
        if !running_as_root {
            assert_eq!(unreadable.len(), 1);
            assert_eq!(unreadable[0].0, dir.path().join("locked"));
        }
    }
}
