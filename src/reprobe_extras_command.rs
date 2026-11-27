//! `acetate reprobe-extras`: images in extras added before extras knew images. Back then an
//! image was kept with only its size in bytes; now an image has its dimensions and format
//! (`ingest::video::image_facts`), which is what makes it shown as one and served by
//! `GET /api/images`. Files with no facts at all are looked at; only images are changed.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::storage::BlobStore;

pub struct Options {
    /// Say what would change, but change nothing.
    pub dry_run: bool,
}

pub async fn run(pool: &PgPool, store: &BlobStore, options: Options) -> anyhow::Result<()> {
    let mut tx = db::begin(pool, None).await?;
    let unread = sqlx::query!(
        r#"SELECT id, sha256, name FROM extra_files
           WHERE width IS NULL AND codec IS NULL AND duration_ms IS NULL AND container IS NULL
           ORDER BY added_at, id"#
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let (mut images, mut missing, mut other) = (Vec::<(Uuid, crate::db::extras::FileFacts, String)>::new(), 0usize, 0usize);
    for file in unread {
        let path = store.path_for(&file.sha256).await;
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            missing += 1;
            continue;
        }
        match crate::ingest::video::image_facts(&path).await {
            Some(facts) => images.push((file.id, facts, file.name)),
            None => other += 1,
        }
    }
    for (_, facts, name) in &images {
        println!("{name}: {}×{} {}", facts.width.unwrap_or(0), facts.height.unwrap_or(0), facts.container.as_deref().unwrap_or("?"));
    }
    println!("{} images, {other} other files left as they are, {missing} whose bytes aren't stored here", images.len());
    if options.dry_run {
        println!("(dry run: nothing changed)");
        return Ok(());
    }
    let mut tx = db::begin(pool, None).await?;
    for (id, facts, _) in &images {
        db::extras::set_file_facts(&mut tx, *id, facts).await?;
    }
    tx.commit().await?;
    println!("{} images updated", images.len());
    Ok(())
}
