//! Tracks that play a video file. A track made from a video extra used to play the video
//! itself, which only plays where the player can decode its container (webm does, mkv
//! doesn't). Now its sound is taken out of the video; this finds the tracks made before
//! that, and `extract` repairs them: the sound becomes the track's file (the video file
//! stays as a previous file) and the video, an extra, is linked to the track.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::ingest::{AddAs, ReplaceOptions, video};
use crate::model::sources::ValidSource;
use crate::storage::BlobStore;

use super::{Context, Finding};

/// A track and the video extra whose file it plays.
struct Found {
    track: Uuid,
    title: String,
    extra: Uuid,
    extra_hidden: bool,
    file_name: String,
    sha256: String,
    /// The track's file (its origin says where the new file came from).
    file: Uuid,
}

async fn find(pool: &PgPool) -> anyhow::Result<Vec<Found>> {
    let mut tx = db::begin(pool, None).await?;
    let system = db::as_system(&mut tx).await?;
    let found = sqlx::query_as!(
        Found,
        r#"SELECT t.id AS track, t.title, e.id AS extra, e.hidden_at IS NOT NULL AS "extra_hidden!",
                  xf.name AS file_name, xf.sha256, f.id AS file
           FROM tracks t
           JOIN files f ON f.id = t.file_id
           JOIN extra_files xf ON xf.sha256 = f.sha256 AND xf.width IS NOT NULL
           JOIN extras e ON e.id = xf.extra_id
           WHERE t.merged_into IS NULL AND t.became_extra_id IS NULL
           ORDER BY t.title, t.id"#
    )
    .fetch_all(&mut *tx)
    .await?;
    system.end(&mut tx).await?;
    Ok(found)
}

pub(super) async fn check(ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    Ok(find(&ctx.pool).await?.into_iter().map(|f| Finding::note(format!("“{}” plays a video file ({}); track {}", f.title, f.file_name, f.track))).collect())
}

/// What `extract` did to one track.
#[derive(Debug, serde::Serialize)]
pub struct Extracted {
    pub track_id: Uuid,
    pub title: String,
    /// `done`, `would` (a preview) or `failed`.
    pub status: &'static str,
    pub message: String,
}

/// Repairs the tracks `check` finds, for `user`. With `preview`, only says what could be
/// done. A track that fails is left as it was, and the others go on.
pub async fn extract(pool: &PgPool, store: &BlobStore, user: Uuid, preview: bool) -> anyhow::Result<Vec<Extracted>> {
    let mut done = Vec::new();
    for found in find(pool).await? {
        let result = if preview { check_one(store, &found).await } else { repair(pool, store, user, &found).await };
        let (status, message) = match result {
            Ok(message) => (if preview { "would" } else { "done" }, message),
            Err(why) => ("failed", why),
        };
        done.push(Extracted { track_id: found.track, title: found.title, status, message });
    }
    Ok(done)
}

/// Whether the sound can be taken out, without doing it.
async fn check_one(store: &BlobStore, found: &Found) -> Result<String, String> {
    let facts = video::probe(&store.path_for(&found.sha256).await).await.ok_or("the video can't be read (is ffprobe installed?)")?;
    let codec = facts.audio_codec.ok_or("the video has no sound")?;
    if !video::can_copy_out(&codec) {
        return Err(format!("its sound is {codec}, which can't be copied out as it is"));
    }
    Ok(format!("its {codec} sound would be taken out of {}", found.file_name))
}

async fn repair(pool: &PgPool, store: &BlobStore, user: Uuid, found: &Found) -> Result<String, String> {
    let failed = |err: &dyn std::fmt::Display| err.to_string();
    let (staged, name) = video::stage_audio(store, &found.sha256, &found.file_name).await.map_err(|e| failed(&e))?;
    let origin = {
        let mut tx = db::begin(pool, None).await.map_err(|e| failed(&e))?;
        let system = db::as_system(&mut tx).await.map_err(|e| failed(&e))?;
        let row = sqlx::query!(
            r#"SELECT src.platform, src.external_id, ss.data
               FROM files f JOIN source_snapshots ss ON ss.id = f.origin_snapshot_id JOIN sources src ON src.id = ss.source_id
               WHERE f.id = $1"#,
            found.file
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| failed(&e))?;
        system.end(&mut tx).await.map_err(|e| failed(&e))?;
        let row = row.ok_or("the track's file has no origin recorded")?;
        ValidSource { platform: row.platform, source_id: None, captured_at: None, external_id: row.external_id, data: row.data }
    };
    let reason = "sound taken out of its video";
    let options = ReplaceOptions { user: Some(user), uploaded_filename: name.clone(), origin, reason: reason.into(), volume: None, original_filename: None };
    crate::ingest::add_file_to_track(pool, &staged, found.track, AddAs::CurrentFile, &options).await.map_err(|e| format!("{e:#}"))?;
    // The video stays an extra, seen again and linked to the track.
    let mut tx = db::begin(pool, Some(user)).await.map_err(|e| failed(&e))?;
    if found.extra_hidden {
        db::extras::set_hidden(&mut tx, found.extra, false, reason).await.map_err(|e| failed(&e))?;
    }
    db::extras::link(&mut tx, found.extra, db::extras::Link::Track(found.track), reason).await.map_err(|e| failed(&e))?;
    tx.commit().await.map_err(|e| failed(&e))?;
    Ok(format!("now plays {name}; {} is an extra of the track", found.file_name))
}
