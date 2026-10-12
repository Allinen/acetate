//! The checks on the blob store: what the database says is stored, against what is on the
//! disks.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::db;
use crate::storage::BlobStore;
use crate::verify_command::{hash_all, list_files};

use super::{Context, Finding};

/// What is on the volumes' disks: each blob's volume and path, by its name (its hash).
pub struct Scan {
    /// Blobs by hash. A hash on two volumes is kept once (`twice` has the other).
    found: HashMap<String, (String, PathBuf)>,
    twice: Vec<(String, String, String)>,
    /// Files whose name isn't a hash, and folders that couldn't be read.
    odd: Vec<PathBuf>,
    unreadable: usize,
}

fn is_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Scan {
    pub(super) async fn list(store: &BlobStore) -> anyhow::Result<Scan> {
        let store = store.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let mut scan = Scan { found: HashMap::new(), twice: Vec::new(), odd: Vec::new(), unreadable: 0 };
            for volume in store.volumes() {
                let mut files = Vec::new();
                let mut unreadable = Vec::new();
                list_files(volume.blobs_dir(), &mut files, &mut unreadable);
                scan.unreadable += unreadable.len();
                for path in files {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if !is_hash(&name) {
                        scan.odd.push(path);
                    } else if let Some((first, _)) = scan.found.get(&name) {
                        scan.twice.push((name, first.clone(), volume.name.clone()));
                    } else {
                        scan.found.insert(name, (volume.name.clone(), path));
                    }
                }
            }
            scan
        })
        .await?)
    }
}

/// Entries that something uses (not "other", and not deleted on purpose).
fn in_use(state: &str) -> bool {
    state != "other" && state != "gone"
}

pub(super) async fn present(ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    ctx.progress.step("Listing the blob store", 0);
    let scan = ctx.scan().await?;
    let entries = ctx.entries().await?;
    let mut findings = Vec::new();
    if scan.unreadable > 0 {
        findings.push(Finding::problem(format!("{} folders in the blob store couldn't be read; files in them count as missing below", scan.unreadable)));
    }
    let mut missing = Vec::new();
    let mut elsewhere = Vec::new();
    for e in entries.iter().filter(|e| in_use(e.state)) {
        match scan.found.get(&e.sha256) {
            None => missing.push(format!("{} ({}) is used but is in no volume", e.sha256, e.state)),
            Some((volume, _)) if Some(volume) != e.volume.as_ref() => {
                elsewhere.push(format!("{} is recorded on {} but is on {volume}", e.sha256, e.volume.as_deref().unwrap_or("no volume")));
            }
            Some(_) => {}
        }
    }
    missing.sort();
    elsewhere.sort();
    findings.extend(missing.into_iter().map(Finding::problem));
    findings.extend(elsewhere.into_iter().map(Finding::note));
    for (sha, a, b) in &scan.twice {
        findings.push(Finding::note(format!("{sha} is on two volumes: {a} and {b}")));
    }
    Ok(findings)
}

pub(super) async fn unused(ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    ctx.progress.step("Listing the blob store", 0);
    let scan = ctx.scan().await?;
    let entries = ctx.entries().await?;
    let used: HashSet<&str> = entries.iter().filter(|e| in_use(e.state)).map(|e| e.sha256.as_str()).collect();
    // An image in the editor is waiting for its edit: not unused yet.
    let staged: HashSet<String> = {
        let mut tx = db::begin(&ctx.pool, None).await?;
        let system = db::as_system(&mut tx).await?;
        let staged = sqlx::query_scalar!("SELECT DISTINCT blob_sha256 FROM staged_covers").fetch_all(&mut *tx).await?;
        system.end(&mut tx).await?;
        staged.into_iter().collect()
    };
    let size_of: HashMap<&str, Option<i64>> = entries.iter().map(|e| (e.sha256.as_str(), e.size)).collect();
    let mut stray: Vec<String> = scan
        .found
        .iter()
        .filter(|(sha, _)| !used.contains(sha.as_str()) && !staged.contains(*sha))
        .map(|(sha, (volume, _))| match size_of.get(sha.as_str()).copied().flatten() {
            Some(size) => format!("{sha} on {volume} ({})", acetate_size(size)),
            None => format!("{sha} on {volume}"),
        })
        .collect();
    stray.sort();
    let mut findings: Vec<Finding> = stray.into_iter().map(Finding::note).collect();
    findings.extend(scan.odd.iter().map(|p| Finding::note(format!("{} isn't a blob (its name isn't a hash)", p.display()))));
    Ok(findings)
}

fn acetate_size(bytes: i64) -> String {
    let bytes = bytes.max(0) as f64;
    if bytes >= 1e9 {
        format!("{:.1} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.1} MB", bytes / 1e6)
    } else {
        format!("{:.0} kB", bytes / 1e3)
    }
}

pub(super) async fn staged_covers(ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    let entries = ctx.entries().await?;
    let state_of: HashMap<&str, &str> = entries.iter().map(|e| (e.sha256.as_str(), e.state)).collect();
    let mut tx = db::begin(&ctx.pool, None).await?;
    let system = db::as_system(&mut tx).await?;
    let rows = sqlx::query!(
        r#"
        SELECT u.username, s.blob_sha256, extract(epoch FROM now() - s.staged_at)::bigint AS "age!"
        FROM staged_covers s JOIN users u ON u.id = s.user_id
        WHERE s.staged_at < now() - interval '1 day'
        ORDER BY s.staged_at
        "#
    )
    .fetch_all(&mut *tx)
    .await?;
    system.end(&mut tx).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let days = r.age / 86_400;
            let note = match state_of.get(r.blob_sha256.as_str()) {
                Some(&state) if in_use(state) => " (the image is used elsewhere; only this staging record is left)",
                _ => "",
            };
            Finding::note(format!("{} staged {} {days} days ago and no edit used it{note}", r.username, r.blob_sha256))
        })
        .collect())
}

pub(super) async fn intact(ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    ctx.progress.step("Listing the blob store", 0);
    let scan = ctx.scan().await?;
    let entries = ctx.entries().await?;
    let paths: Vec<PathBuf> = entries.iter().filter(|e| in_use(e.state)).filter_map(|e| scan.found.get(&e.sha256)).map(|(_, p)| p.clone()).collect();
    ctx.progress.step("Reading the files", paths.len() as u64);
    let progress = ctx.progress.clone();
    let hashed = hash_all(paths, &move |done, _| progress.set_done(done as u64)).await;
    let mut findings = Vec::new();
    for (path, result) in hashed {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match result {
            Ok((sha, _)) if sha == name => {}
            Ok((sha, _)) => findings.push(Finding::problem(format!("{}: its content hashes to {sha}", path.display()))),
            Err(err) => findings.push(Finding::problem(format!("{}: {err}", path.display()))),
        }
    }
    findings.sort_by(|a, b| a.message.cmp(&b.message));
    Ok(findings)
}
