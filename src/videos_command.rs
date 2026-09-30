//! `acetate videos <dir>`: music videos from the server's disk, each an extra of kind
//! "video" (named after its file, linked to nothing yet: link them from their pages, or
//! Review's "Unlinked extras"). No upload limit: the files are read here. Files already
//! stored (the same bytes) are skipped, so it can be run again. Never writes, moves or
//! deletes anything in <dir>.

use std::path::{Path, PathBuf};

use anyhow::Context;
use sqlx::PgPool;

use crate::db;
use crate::ingest::{self, video};
use crate::storage::BlobStore;

pub async fn run(pool: &PgPool, store: &BlobStore, dir: &Path, kind: &str, dry_run: bool) -> anyhow::Result<()> {
    let mut files = Vec::new();
    videos_below(dir, &mut files)?;
    files.sort();
    let (mut added, mut already, mut failed) = (0, 0, 0);
    for path in &files {
        match one(pool, store, path, kind, dry_run).await {
            Ok(true) => added += 1,
            Ok(false) => already += 1,
            Err(err) => {
                failed += 1;
                eprintln!("{}: {err:#}", path.display());
            }
        }
    }
    let verb = if dry_run { "would add" } else { "added" };
    println!("{} videos: {verb} {added}, {already} stored already, {failed} failed", files.len());
    Ok(())
}

/// One file. True: added (or, dry run, would be); false: stored already.
async fn one(pool: &PgPool, store: &BlobStore, path: &Path, kind: &str, dry_run: bool) -> anyhow::Result<bool> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let pending = ingest::receive_path(store, path, store.default_volume()).await?;
    let (sha256, size) = (pending.sha256.clone(), pending.size as i64);
    let mut tx = db::begin(pool, None).await?;
    let stored = sqlx::query_scalar!("SELECT extra_id FROM extra_files WHERE sha256 = $1 LIMIT 1", sha256).fetch_optional(&mut *tx).await?;
    if let Some(extra) = stored {
        println!("already stored: {} (extra {extra})", path.display());
        return Ok(false);
    }
    if dry_run {
        println!("would add: {}", path.display());
        return Ok(true);
    }
    let (volume, stored_at) = store.commit(pending, store.default_volume()).await?;
    db::blobs::register(&mut tx, &sha256, &volume, Some(size)).await?;
    let mut facts = video::examine(store, &stored_at, &name).await;
    facts.imported_from = Some(path.to_string_lossy().to_string());
    let title = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| name.clone());
    let about = db::extras::About { description: String::new(), source: String::new() };
    let (extra, _) = db::extras::create(&mut tx, &title, kind, &about, &[], None, "imported from disk").await?;
    db::extras::add_file(&mut tx, extra, &sha256, &name, size, &facts, "imported from disk").await?;
    tx.commit().await?;
    let res = match (facts.width, facts.height) {
        (Some(w), Some(h)) => format!("{w}×{h}"),
        _ => "not read (is ffprobe installed?)".into(),
    };
    println!("added: {} ({res})", path.display());
    Ok(true)
}

/// The video files below `dir` (by their names).
fn videos_below(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            videos_below(&path, out)?;
        } else if video::is_video_name(&path.to_string_lossy()) {
            out.push(path);
        }
    }
    Ok(())
}
