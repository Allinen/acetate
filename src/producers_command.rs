//! `acetate producers-from-composer`: producer credits from the COMPOSER tag, for tracks
//! imported before ingest read it (2026-09-26). New imports credit COMPOSER's names as
//! producers themselves (see `ingest`); this gives older tracks the same.
//!
//! A track is only changed if nothing says COMPOSER was left out on purpose or dealt with
//! by hand: its metadata came from its file's tags (not a description), no revision of it
//! ever credited a producer, and it was imported before `before`. Its stored file is read
//! the way ingest reads it, and the names are matched to artists like ingest matches them
//! (made if they don't exist; collectives aren't credited). The credits are added after
//! the others, in a new revision of its main version. Read-only unless `apply`.

use chrono::NaiveDate;
use sqlx::PgPool;

use crate::db;
use crate::db::revisions::Source;
use crate::ingest::describe::{Hints, Resolver};
use crate::ingest::tags;
use crate::model::Credit;
use crate::storage::BlobStore;

const REASON: &str = "producer credits from the file's COMPOSER tag (added later: ingest didn't read it when this was imported)";

pub async fn run(pool: &PgPool, store: &BlobStore, before: NaiveDate, apply: bool) -> anyhow::Result<()> {
    let verb = if apply { "credited" } else { "would credit" };
    let before = before.and_hms_opt(0, 0, 0).unwrap().and_utc();
    // As the system: every track, hidden ones included.
    let mut tx = db::begin(pool, None).await?;
    // The tags in the database narrow it down (a COMPOSER-like key somewhere); the file
    // itself says exactly.
    let candidates = sqlx::query!(
        r#"
        SELECT t.id, t.title, t.primary_variant_id AS "variant!", f.sha256, f.uploaded_filename
        FROM tracks t
        JOIN files f ON f.id = t.file_id
        JOIN track_variants v ON v.id = t.primary_variant_id
        WHERE t.merged_into IS NULL AND t.became_extra_id IS NULL
          AND t.created_at < $1
          AND v.label LIKE 'File tags%'
          AND (f.embedded_tags::text ILIKE '%compos%' OR f.embedded_tags::text LIKE '%TCOM%' OR f.embedded_tags::text LIKE '%wrt%')
          AND NOT EXISTS (
              SELECT 1 FROM track_revisions r
              WHERE r.track_id = t.id AND r.data -> 'credits' @> '[{"role": "producer"}]'
          )
        ORDER BY t.id
        "#,
        before
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut resolver = Resolver::default();
    let hints = Hints::default();
    let (mut tracks, mut credits_added, mut unreadable) = (0, 0, 0);
    for track in candidates {
        let path = store.path_for(&track.sha256).await;
        let extension = std::path::Path::new(&track.uploaded_filename)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase);
        let read = tokio::task::spawn_blocking(move || tags::read_tags(&path, extension.as_deref())).await?;
        let composers = match read {
            Ok(tags) => tags.composers,
            Err(err) => {
                println!("can't read \"{}\" ({}): {err:#}", track.title, track.id);
                unreadable += 1;
                continue;
            }
        };
        if composers.is_empty() {
            continue;
        }
        let Some((_, mut meta)) = db::revisions::latest_internal(&mut tx, track.id, track.variant).await? else {
            continue;
        };
        let mut added = Vec::new();
        for name in &composers {
            if resolver.collective(&mut tx, &hints, name).await?.is_some() {
                continue;
            }
            let artist = resolver.artist(&mut tx, &hints, name).await?;
            if meta.credits.iter().any(|c| c.artist_id == artist.id && c.role == "producer") {
                continue;
            }
            meta.credits.push(Credit { artist_id: artist.id, role: "producer".to_string(), alias_id: artist.alias_id });
            added.push(name.as_str());
        }
        if added.is_empty() {
            continue;
        }
        println!("{verb} {} as producer on \"{}\" ({})", added.join(", "), track.title, track.id);
        db::revisions::write_internal(&mut tx, track.id, track.variant, None, &meta, Source::Import, REASON, false).await?;
        tracks += 1;
        credits_added += added.len();
    }
    let unreadable = if unreadable > 0 { format!(" ({unreadable} files couldn't be read)") } else { String::new() };
    if apply {
        tx.commit().await?;
        println!("{credits_added} producer credits on {tracks} tracks{unreadable}");
    } else {
        // Nothing kept: the artists it would make, and the revisions, go too.
        drop(tx);
        println!("{credits_added} producer credits on {tracks} tracks would be added{unreadable}; nothing changed (run with --apply)");
    }
    Ok(())
}
