//! `acetate own-album-covers`: one-off, for albums without covers of their own (made by
//! hand, or imported before ingest gave new albums the track's covers, 2026-09-27). Such
//! an album shows its first track's main cover; this gives it a copy of that cover (the
//! same image), so it's the album's own and stays when its tracks change. The cover is
//! chosen as the catalog chooses it (`db::library::albums_json`). Read-only unless
//! `apply`.

use sqlx::PgPool;

use crate::db;

pub async fn run(pool: &PgPool, apply: bool) -> anyhow::Result<()> {
    let verb = if apply { "gave" } else { "would give" };
    // As the system: every album, and covers of tracks hidden from anyone.
    let mut tx = db::begin(pool, None).await?;
    let albums = sqlx::query!(
        r#"
        SELECT DISTINCT ON (a.id) a.id, a.title, c.id AS "cover!", t.title AS track_title
        FROM albums a
        JOIN album_tracks at ON at.album_id = a.id
        JOIN tracks t ON t.id = at.track_id
        JOIN covers c ON c.track_id = at.track_id AND c.trashed_at IS NULL
        WHERE a.merged_into IS NULL
          AND NOT EXISTS (SELECT 1 FROM covers o WHERE o.album_id = a.id AND o.trashed_at IS NULL)
        ORDER BY a.id, at.disc_number NULLS FIRST, at.track_number NULLS LAST, c.position, c.id
        "#
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut given = 0;
    for album in &albums {
        if db::album_merges::copy_if_new(&mut tx, album.cover, album.id).await?.is_some() {
            println!("{verb} \"{}\" ({}) a copy of \"{}\"'s cover", album.title, album.id, album.track_title);
            given += 1;
        }
    }
    if apply {
        tx.commit().await?;
        println!("{given} albums have a cover of their own now");
    } else {
        drop(tx);
        println!("{given} albums would get a cover of their own; nothing changed (run with --apply)");
    }
    Ok(())
}
