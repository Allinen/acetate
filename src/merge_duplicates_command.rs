//! `acetate merge-duplicates`: merges the "possible duplicate" pairs from review flags that
//! are clearly the same track: exactly the same title and about the same length. For
//! libraries with too many flags to go through by hand.
//!
//! Each merge is an ordinary merge (`revisions::merge_tracks`): revisioned, settles the
//! pair's flags, and can be undone on the kept track's page like any other.

use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::db::revisions::{MergePicks, Side};

pub struct Options {
    /// How far apart the two lengths may be, in milliseconds.
    pub max_length_difference_ms: i64,
    /// Titles that differ only in case ("Intro" and "INTRO") count as the same.
    pub ignore_case: bool,
    pub dry_run: bool,
}

/// What a merge needs to know about one track.
struct Candidate {
    id: Uuid,
    title: String,
    duration_ms: Option<i64>,
    size_bytes: i64,
}

pub async fn run(pool: &PgPool, options: Options) -> anyhow::Result<()> {
    // Every pair an open flag names: (flagged track, one of the tracks it looks like).
    let pairs = sqlx::query!(
        r#"
        SELECT rf.track_id, listed.id::uuid AS "other!"
        FROM review_flags rf
        CROSS JOIN jsonb_array_elements_text(rf.related -> 'tracks') AS listed(id)
        WHERE rf.kind = 'possible_duplicate' AND rf.resolved_at IS NULL
        ORDER BY rf.id, listed.id
        "#
    )
    .fetch_all(pool)
    .await?;
    println!(
        "{} pairs in open \"possible duplicate\" flags{}",
        pairs.len(),
        if options.dry_run { " (dry run: nothing will be changed)" } else { "" }
    );

    let (mut merged, mut different, mut failed) = (0, 0, 0);
    let mut seen = HashSet::new();
    for pair in pairs {
        // Earlier merges may have made either one a redirect: go where it leads.
        let (Some(a), Some(b)) = (current(pool, pair.track_id).await?, current(pool, pair.other).await?) else {
            continue;
        };
        if a.id == b.id {
            continue; // merged already (a chain)
        }
        // Flagged both ways (A looks like B, B looks like A): once is enough.
        if !seen.insert((a.id.min(b.id), a.id.max(b.id))) {
            continue;
        }
        let length_ok = match (a.duration_ms, b.duration_ms) {
            (Some(x), Some(y)) => (x - y).abs() <= options.max_length_difference_ms,
            _ => false, // unknown length: not clear enough to merge without a look
        };
        let same_title = if options.ignore_case {
            a.title.to_lowercase() == b.title.to_lowercase()
        } else {
            a.title == b.title
        };
        if !same_title || !length_ok {
            different += 1;
            continue;
        }

        // The older track is kept (UUIDv7: the smaller id was created first), with its
        // metadata and its covers first (the merged one's follow). The bigger file is kept:
        // same length, so more bytes is the better copy (lossless over lossy, a higher
        // bitrate).
        let (kept, dupe) = if a.id < b.id { (&a, &b) } else { (&b, &a) };
        let file = if dupe.size_bytes > kept.size_bytes { Side::Right } else { Side::Left };
        let shown = format!(
            "\"{}\" {} <- {} ({} file)",
            kept.title,
            kept.id,
            dupe.id,
            if file == Side::Right { "the merged track's" } else { "its own" }
        );
        if options.dry_run {
            println!("would merge  {shown}");
            merged += 1;
            continue;
        }

        let picks = MergePicks { file, ..Default::default() };
        let mut tx = db::begin(pool, None).await?;
        match db::revisions::merge_tracks(&mut tx, kept.id, dupe.id, &picks, None, None, "same title and length (merge-duplicates)").await {
            Ok(_) => {
                tx.commit().await?;
                println!("merged       {shown}");
                merged += 1;
            }
            Err(err) => {
                drop(tx); // rolls back
                eprintln!("not merged   {shown}: {err:#}");
                failed += 1;
            }
        }
    }

    println!(
        "\n{merged} {}, {different} left as they are (different title or length), {failed} failed{}",
        if options.dry_run { "would be merged" } else { "merged" },
        if options.dry_run {
            "\ndry run: nothing was changed (pairs chained through a merge are only found in a real run)"
        } else {
            ""
        }
    );
    Ok(())
}

/// The track as it is now: following merge redirects to the track it ended up in.
async fn current(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Candidate>> {
    let row = sqlx::query!(
        r#"
        SELECT t.id, t.title, f.duration_ms, f.size_bytes
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        WHERE t.id = COALESCE((SELECT merged_into FROM tracks WHERE id = $1), $1)
        "#,
        id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Candidate {
        id: r.id,
        title: r.title,
        duration_ms: r.duration_ms,
        size_bytes: r.size_bytes,
    }))
}
