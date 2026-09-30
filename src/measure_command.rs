//! `acetate measure-covers`: fills in what older rows lack. Covers imported before ingest
//! measured them (only uploads were) get their width and height, read from the image; and
//! blobs stored without a size (covers, thumbnails) get it, for the storage totals. Reads
//! the blobs, never changes them. Read-only unless `apply`.

use sqlx::PgPool;

use crate::db;
use crate::storage::BlobStore;

pub async fn run(pool: &PgPool, store: &BlobStore, apply: bool) -> anyhow::Result<()> {
    // As the system: every cover, hidden tracks' too.
    let mut tx = db::begin(pool, None).await?;
    let unmeasured = sqlx::query_scalar!("SELECT DISTINCT blob_sha256 FROM covers WHERE width IS NULL OR height IS NULL")
        .fetch_all(&mut *tx)
        .await?;
    let (mut measured, mut unreadable) = (0, 0);
    for sha256 in &unmeasured {
        let image = match tokio::fs::read(store.path_for(sha256).await).await {
            Ok(bytes) => crate::model::image_info(&bytes),
            Err(err) => {
                println!("can't read cover image {sha256}: {err}");
                unreadable += 1;
                continue;
            }
        };
        let Some((width, height)) = image.and_then(|i| Some((i.width?, i.height?))) else {
            println!("{sha256} isn't an image this can measure");
            unreadable += 1;
            continue;
        };
        let rows = sqlx::query!(
            "UPDATE covers SET width = $2, height = $3 WHERE blob_sha256 = $1 AND (width IS NULL OR height IS NULL)",
            sha256,
            width,
            height
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        measured += rows;
    }

    let unsized_blobs = sqlx::query_scalar!("SELECT sha256 FROM blobs WHERE size_bytes IS NULL").fetch_all(&mut *tx).await?;
    let mut sized = 0;
    for sha256 in &unsized_blobs {
        if let Some(size) = store.size(sha256).await {
            sqlx::query!("UPDATE blobs SET size_bytes = $2 WHERE sha256 = $1", sha256, size as i64)
                .execute(&mut *tx)
                .await?;
            sized += 1;
        }
    }

    let unreadable = if unreadable > 0 { format!(" ({unreadable} images couldn't be measured)") } else { String::new() };
    if apply {
        tx.commit().await?;
        println!("measured {measured} covers and {sized} blobs' sizes{unreadable}");
    } else {
        drop(tx);
        println!("would measure {measured} covers and {sized} blobs' sizes{unreadable}; nothing changed (run with --apply)");
    }
    Ok(())
}
