//! `acetate recheck-duplicates`: looks at the open "possible duplicate" flags again with
//! today's rule for titles, and settles the pairs it rules out (another version of the
//! song: "Song" and "Song (Instrumental)", "Song 2"). For flags raised before the rule.

use sqlx::PgPool;

use crate::db;

pub async fn run(pool: &PgPool, dry_run: bool) -> anyhow::Result<()> {
    // One transaction, rolled back in a dry run: what it lists is exactly what a real run does.
    let mut tx = db::begin(pool, None).await?;
    let settled = db::review::recheck_duplicates(&mut tx, "another version of the song (recheck-duplicates)").await?;
    for pair in &settled {
        println!(
            "{}  \"{}\" {}  /  \"{}\" {}",
            if dry_run { "would settle" } else { "settled" },
            pair.track.1,
            pair.track.0,
            pair.other.1,
            pair.other.0
        );
    }
    if dry_run {
        tx.rollback().await?;
        println!("\n{} pairs would be settled\ndry run: nothing was changed", settled.len());
    } else {
        tx.commit().await?;
        println!("\n{} pairs settled", settled.len());
    }
    Ok(())
}
