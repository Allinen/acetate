//! `acetate duplicate-albums`: finds albums that look like the same album twice: about the
//! same number of tracks, about the same total length, a similar title and an album artist
//! in common (e.g. split by
//! ingest because the album artists or the date were written differently). Lists them;
//! with `--merge`, merges each pair (`db::album_merges::merge_album`) into the older album.
//!
//! Listing is read-only. A merge is revisioned for every track it moves, like a merge in
//! the web UI.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::db;

pub struct Options {
    /// How many tracks the two may differ by.
    pub max_track_difference: i64,
    /// How far apart the two total lengths may be, in milliseconds.
    pub max_length_difference_ms: i64,
    /// How alike the titles must be: pg_trgm `similarity`, 0 to 1 (1 = the same, ignoring
    /// case).
    pub min_title_similarity: f32,
    /// Also pairs without an album artist in common. Off by default: singles with a
    /// common title ("Intro") by different artists would look alike.
    pub any_artists: bool,
    pub merge: bool,
}

/// What's shown about one album of a pair.
struct AlbumInfo {
    id: Uuid,
    title: String,
    release_date: Option<String>,
    artists: String,
    tracks: i64,
    total_ms: i64,
}

pub async fn run(pool: &PgPool, options: Options) -> anyhow::Result<()> {
    let pairs = find_pairs(pool, &options).await?;
    println!(
        "{} pairs of albums alike: at most {} tracks and {} s apart, titles at least {} alike{}{}\n",
        pairs.len(),
        options.max_track_difference,
        options.max_length_difference_ms as f64 / 1000.0,
        options.min_title_similarity,
        if options.any_artists { "" } else { ", an album artist in common" },
        if options.merge { "" } else { " (listing only: --merge merges them)" }
    );

    let (mut merged, mut skipped, mut failed) = (0, 0, 0);
    let mut conn = pool.acquire().await?;
    for (a_id, b_id, similarity) in pairs {
        // The older album is kept (UUIDv7: the smaller id was created first). Pairs are
        // found with a < b.
        let kept = describe(&mut conn, a_id).await?;
        let dupe = describe(&mut conn, b_id).await?;
        let matching = matching_track_titles(&mut conn, a_id, b_id).await?;
        println!("  {}", shown(&kept));
        println!("  {}", shown(&dupe));
        println!(
            "  titles {:.2} alike, {matching} of {} track titles also on the other",
            similarity, dupe.tracks
        );
        if !options.merge {
            println!();
            continue;
        }

        // Earlier merges in this run may have merged either one already.
        let (Some(into), Some(from)) = (current(&mut conn, kept.id).await?, current(&mut conn, dupe.id).await?)
        else {
            skipped += 1;
            println!("  skipped: not found\n");
            continue;
        };
        if into == from {
            skipped += 1;
            println!("  skipped: merged together already\n");
            continue;
        }
        let mut tx = db::begin(pool, None).await?;
        match db::album_merges::merge_album(&mut tx, from, into, "same album (duplicate-albums)").await {
            Ok(result) => {
                tx.commit().await?;
                merged += 1;
                println!("  merged the second into the first ({} tracks moved)\n", result.tracks);
            }
            Err(err) => {
                drop(tx); // rolls back
                failed += 1;
                eprintln!("  not merged: {err:#}\n");
            }
        }
    }
    if options.merge {
        println!("{merged} merged, {skipped} skipped, {failed} failed");
    }
    Ok(())
}

/// Every pair of albums (not merged, every track's length known, an album artist in
/// common unless `any_artists`) that fits the options,
/// as (older id, newer id, title similarity).
async fn find_pairs(pool: &PgPool, options: &Options) -> anyhow::Result<Vec<(Uuid, Uuid, f32)>> {
    let mut tx = pool.begin().await?;
    // `%` (similar titles) uses the trigram index, with this threshold. `true`: only for
    // this transaction.
    sqlx::query_scalar!(
        "SELECT set_config('pg_trgm.similarity_threshold', $1, true)",
        options.min_title_similarity.to_string()
    )
    .fetch_one(&mut *tx)
    .await?;
    let rows = sqlx::query!(
        r#"
        WITH stats AS MATERIALIZED (
            SELECT at.album_id, count(*) AS tracks, count(f.duration_ms) AS known,
                   sum(f.duration_ms)::bigint AS total_ms
            FROM album_tracks at
            JOIN tracks t ON t.id = at.track_id
            JOIN files f ON f.id = t.file_id
            GROUP BY at.album_id
        )
        SELECT a.id AS "a!", b.id AS "b!", similarity(a.title, b.title) AS "similarity!"
        FROM albums a
        JOIN stats sa ON sa.album_id = a.id
        JOIN albums b ON b.title % a.title AND b.id > a.id
        JOIN stats sb ON sb.album_id = b.id
        WHERE a.merged_into IS NULL AND b.merged_into IS NULL
          AND sa.known = sa.tracks AND sb.known = sb.tracks
          AND abs(sa.tracks - sb.tracks) <= $1
          AND abs(sa.total_ms - sb.total_ms) <= $2
          AND ($3 OR EXISTS (SELECT 1 FROM album_credits ca
                             JOIN album_credits cb ON cb.artist_id = ca.artist_id
                             WHERE ca.album_id = a.id AND cb.album_id = b.id))
        ORDER BY a.id, b.id
        "#,
        options.max_track_difference,
        options.max_length_difference_ms,
        options.any_artists
    )
    .fetch_all(&mut *tx)
    .await?;
    Ok(rows.into_iter().map(|r| (r.a, r.b, r.similarity)).collect())
}

async fn describe(conn: &mut PgConnection, id: Uuid) -> anyhow::Result<AlbumInfo> {
    let row = sqlx::query!(
        r#"
        SELECT al.id, al.title, al.release_date,
               COALESCE((SELECT string_agg(ar.name, '; ' ORDER BY c.position, ar.name)
                         FROM album_credits c JOIN artists ar ON ar.id = c.artist_id
                         WHERE c.album_id = al.id), '') AS "artists!",
               (SELECT count(*) FROM album_tracks at WHERE at.album_id = al.id) AS "tracks!",
               (SELECT COALESCE(sum(f.duration_ms), 0)::bigint FROM album_tracks at
                JOIN tracks t ON t.id = at.track_id JOIN files f ON f.id = t.file_id
                WHERE at.album_id = al.id) AS "total_ms!"
        FROM albums al WHERE al.id = $1
        "#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(AlbumInfo {
        id: row.id,
        title: row.title,
        release_date: row.release_date,
        artists: row.artists,
        tracks: row.tracks,
        total_ms: row.total_ms,
    })
}

/// How many of `b`'s tracks have a title (ignoring case) that's also on `a`. Low for
/// e.g. an instrumental version of an album ("Song (Instrumental)").
async fn matching_track_titles(conn: &mut PgConnection, a: Uuid, b: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar!(
        r#"
        SELECT count(*) AS "n!"
        FROM album_tracks bt JOIN tracks t ON t.id = bt.track_id
        WHERE bt.album_id = $2
          AND lower(t.title) IN (SELECT lower(t2.title) FROM album_tracks at
                                 JOIN tracks t2 ON t2.id = at.track_id
                                 WHERE at.album_id = $1)
        "#,
        a,
        b
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// The album as it is now: following a merge redirect.
async fn current(conn: &mut PgConnection, id: Uuid) -> anyhow::Result<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"SELECT COALESCE(merged_into, id) AS "id!" FROM albums WHERE id = $1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

fn shown(album: &AlbumInfo) -> String {
    let seconds = album.total_ms / 1000;
    format!(
        "\"{}\"{} by {} ({} tracks, {}:{:02}) {}",
        album.title,
        album.release_date.as_deref().map(|d| format!(" ({d})")).unwrap_or_default(),
        if album.artists.is_empty() { "nobody" } else { &album.artists },
        album.tracks,
        seconds / 60,
        seconds % 60,
        album.id
    )
}

/// `acetate fix-merged-covers`: one-off, for merges made before 2026-09-26.
/// * Track merges put the covers of the side not picked in the trash (e.g. a single's
///   cover). They're taken back out, after the covers of the track holding them now.
/// * Album merges (not undone), recorded in each so an undo takes it back too: covers left
///   on the merged album (their image was already on the other one) move after the other
///   album's covers; and a merged album without covers of its own (e.g. a single, whose
///   cover is on its track) showed one from its tracklist: the other album gets a copy.
///
/// A dry run does all of it and then rolls back, so it reports exactly what would happen.
pub async fn fix_merged_covers(pool: &PgPool, dry_run: bool) -> anyhow::Result<()> {
    let verb = if dry_run { "would" } else { "did" };
    let mut tx = db::begin(pool, None).await?;

    // Tracks first: the album fix then finds those covers where they are now.
    let track_merges = sqlx::query!(
        r#"
        SELECT m.id, k.title AS kept_title, d.title AS merged_title, m.reason
        FROM track_merges m
        JOIN tracks k ON k.id = m.into_track_id
        JOIN tracks d ON d.id = m.from_track_id
        WHERE m.undone_at IS NULL
        ORDER BY m.id
        "#
    )
    .fetch_all(&mut *tx)
    .await?;
    let mut restored_total = 0;
    let mut reimports = Vec::new();
    for merge in track_merges {
        let restored = db::revisions::restore_trashed_covers(&mut tx, merge.id).await?;
        if !restored.is_empty() {
            println!(
                "{verb} take {} covers out of the trash, after \"{}\"'s own (merging \"{}\" into it put them there; track merge {}: {})",
                restored.len(),
                merge.kept_title,
                merge.merged_title,
                merge.id,
                merge.reason
            );
            restored_total += restored.len();
        }
        reimports.push(merge);
    }

    // Albums made by importing a stored file again (e.g. the single), without covers:
    // the covers that import came with. Before the album merges, which then carry them
    // along like any cover.
    let mut reimport_total = 0;
    for merge in &reimports {
        for (album, count) in db::revisions::copy_reimport_covers_to_albums(&mut tx, merge.id).await? {
            println!(
                "{verb} copy {count} covers to \"{album}\", from importing \"{}\" again (track merge {})",
                merge.merged_title, merge.id
            );
            reimport_total += count;
        }
    }

    // Albums, oldest first: if A was merged into C after B into A, B's covers reach A
    // first, and then go on to C with A's.
    let album_merges = sqlx::query!(
        r#"
        SELECT m.id, f.title AS from_title, i.title AS into_title
        FROM album_merges m
        JOIN albums f ON f.id = m.from_album_id
        JOIN albums i ON i.id = m.into_album_id
        WHERE m.undone_at IS NULL
        ORDER BY m.id
        "#
    )
    .fetch_all(&mut *tx)
    .await?;
    let (mut moved_total, mut copied_total) = (0, 0);
    for merge in album_merges {
        let moved = db::album_merges::move_leftover_covers(&mut tx, merge.id).await?;
        if moved > 0 {
            println!(
                "{verb} move {moved} covers of album \"{}\" to \"{}\" (album merge {})",
                merge.from_title, merge.into_title, merge.id
            );
            moved_total += moved;
        }
        if db::album_merges::copy_tracklist_cover(&mut tx, merge.id).await?.is_some() {
            println!(
                "{verb} copy the cover album \"{}\" showed from its tracklist to \"{}\" (album merge {})",
                merge.from_title, merge.into_title, merge.id
            );
            copied_total += 1;
        }
    }

    if dry_run {
        drop(tx); // rolls back
    } else {
        tx.commit().await?;
    }
    println!(
        "track covers taken out of the trash: {restored_total}; covers copied to albums from re-imports: {reimport_total}; \
         album covers moved: {moved_total}, copied: {copied_total}{}",
        if dry_run { " (dry run: nothing was changed)" } else { "" }
    );
    Ok(())
}
