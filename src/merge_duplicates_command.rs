//! `acetate merge-duplicates`: merges the "possible duplicate" pairs from review flags that
//! are clearly the same track: exactly the same title and about the same length. For
//! libraries with too many flags to go through by hand. The Review page's bulk tools do
//! the same for the flags shown (see `db::review_bulk::merge_duplicates`).

use sqlx::PgPool;

use crate::db;
use crate::db::review_bulk::{MergedPair, SameTrack};

pub struct Options {
    pub same: SameTrack,
    pub dry_run: bool,
}

pub async fn run(pool: &PgPool, options: Options) -> anyhow::Result<()> {
    // One transaction, rolled back in a dry run: what it lists is exactly what a real run does.
    let mut tx = db::begin(pool, None).await?;
    let done = db::review_bulk::merge_duplicates(&mut tx, None, options.same, "same title and length (merge-duplicates)").await?;
    let shown = |pair: &MergedPair| {
        format!(
            "\"{}\" {} <- {} ({} file)",
            pair.kept.title,
            pair.kept.id,
            pair.merged.id,
            if pair.merged_file_kept { "the merged track's" } else { "its own" }
        )
    };
    for pair in &done.merged {
        println!("{}  {}", if options.dry_run { "would merge" } else { "merged     " }, shown(pair));
    }
    for failed in &done.failed {
        eprintln!("not merged   {}: {}", shown(&failed.pair), failed.error);
    }
    if options.dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    println!(
        "\n{} {}, {} left as they are (different title or length), {} failed{}",
        done.merged.len(),
        if options.dry_run { "would be merged" } else { "merged" },
        done.different,
        done.failed.len(),
        if options.dry_run { "\ndry run: nothing was changed" } else { "" }
    );
    Ok(())
}
