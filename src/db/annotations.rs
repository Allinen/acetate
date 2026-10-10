//! Annotations (migration 0017): a user's plays, rating and star on a track, album or
//! artist. Personal, so everything here acts on the current user's own rows (row-level
//! security makes sure of that), except the merge helpers, which run as the system.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::EditRejected;

/// What gets annotated.
#[derive(Debug, Clone, Copy)]
pub enum Item {
    Track(Uuid),
    Album(Uuid),
    Artist(Uuid),
}

impl Item {
    fn columns(self) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
        match self {
            Item::Track(id) => (Some(id), None, None),
            Item::Album(id) => (None, Some(id), None),
            Item::Artist(id) => (None, None, Some(id)),
        }
    }

    fn id(self) -> Uuid {
        match self {
            Item::Track(id) | Item::Album(id) | Item::Artist(id) => id,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Item::Track(_) => "track",
            Item::Album(_) => "album",
            Item::Artist(_) => "artist",
        }
    }
}

/// One user's annotation of one thing. The default is "never touched".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Annotation {
    pub play_count: i32,
    pub last_played_at: Option<DateTime<Utc>>,
    /// 0 to 10 (half stars); 0 = not rated.
    pub rating: i16,
    pub rated_at: Option<DateTime<Utc>>,
    pub starred: bool,
    pub starred_at: Option<DateTime<Utc>>,
}

impl Annotation {
    /// Two annotations of the same thing combined (duplicates merged, or several files of
    /// one track): plays added up, the latest play, the higher rating, starred if either
    /// was (since the earlier star).
    pub fn combined(&self, other: &Annotation) -> Annotation {
        let (rating, rated_at) = if other.rating > self.rating {
            (other.rating, other.rated_at)
        } else {
            (self.rating, self.rated_at)
        };
        let starred_at = match (self.starred, other.starred) {
            (true, true) => self.starred_at.min(other.starred_at).or(self.starred_at).or(other.starred_at),
            (true, false) => self.starred_at,
            (false, true) => other.starred_at,
            (false, false) => self.starred_at.max(other.starred_at),
        };
        Annotation {
            play_count: self.play_count.saturating_add(other.play_count),
            last_played_at: self.last_played_at.max(other.last_played_at),
            rating,
            rated_at,
            starred: self.starred || other.starred,
            starred_at,
        }
    }
}

/// The current user's annotation of a thing (the default if they never touched it).
pub async fn get(conn: &mut PgConnection, item: Item) -> sqlx::Result<Annotation> {
    let row = sqlx::query_as!(
        Annotation,
        r#"
        SELECT play_count, last_played_at, rating, rated_at, starred, starred_at
        FROM annotations
        WHERE user_id = current_app_user() AND (track_id = $1 OR album_id = $1 OR artist_id = $1)
        "#,
        item.id()
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.unwrap_or_default())
}

/// Writes the current user's annotation of a thing (creating the row if needed). Fails
/// with NotFound if it's not something the user can see.
async fn put(conn: &mut PgConnection, item: Item, a: &Annotation) -> anyhow::Result<()> {
    let (track_id, album_id, artist_id) = item.columns();
    let result = sqlx::query!(
        r#"
        INSERT INTO annotations (track_id, album_id, artist_id, play_count, last_played_at,
                                 rating, rated_at, starred, starred_at)
        SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9
        -- Only for things the user can see (the policy would refuse it anyway; this says
        -- "not found" instead of "not allowed").
        WHERE EXISTS (SELECT 1 FROM tracks WHERE id = $1)
           OR EXISTS (SELECT 1 FROM albums WHERE id = $2)
           OR EXISTS (SELECT 1 FROM artists WHERE id = $3)
        ON CONFLICT (user_id, (COALESCE(track_id, album_id, artist_id))) DO UPDATE SET
            play_count = EXCLUDED.play_count, last_played_at = EXCLUDED.last_played_at,
            rating = EXCLUDED.rating, rated_at = EXCLUDED.rated_at,
            starred = EXCLUDED.starred, starred_at = EXCLUDED.starred_at
        "#,
        track_id,
        album_id,
        artist_id,
        a.play_count,
        a.last_played_at,
        a.rating,
        a.rated_at,
        a.starred,
        a.starred_at,
    )
    .execute(conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(EditRejected::NotFound(item.name()).into());
    }
    Ok(())
}

/// Stars or unstars a thing for the current user.
pub async fn set_starred(conn: &mut PgConnection, item: Item, starred: bool) -> anyhow::Result<Annotation> {
    let mut a = get(conn, item).await?;
    if a.starred != starred {
        a.starred = starred;
        // `starred_at` is when it was first starred: unstarring and restarring keep it.
        if starred && a.starred_at.is_none() {
            a.starred_at = Some(Utc::now());
        }
        put(conn, item, &a).await?;
    }
    Ok(a)
}

/// Rates a thing for the current user: 0 (not rated) to 10.
pub async fn set_rating(conn: &mut PgConnection, item: Item, rating: i16) -> anyhow::Result<Annotation> {
    if !(0..=10).contains(&rating) {
        return Err(EditRejected::Invalid("a rating is 0 to 10".to_string()).into());
    }
    let mut a = get(conn, item).await?;
    if a.rating != rating {
        a.rating = rating;
        a.rated_at = Some(Utc::now());
        put(conn, item, &a).await?;
    }
    Ok(a)
}

/// A play of a track by the current user (the client calls it after a real listen): one
/// more play of the track, and of its albums and its primary artists, as Navidrome counts;
/// and a line in the play log (`plays`).
#[cfg(test)]
pub async fn played(conn: &mut PgConnection, track_id: Uuid) -> anyhow::Result<Annotation> {
    played_at(conn, track_id, Utc::now(), None).await
}

/// `played`, at a time (a play kept on a client while it was offline, sent later), and
/// with the client's `key` for it (written as `client:<key>`): a play sent again with the
/// same key counts once.
pub async fn played_at(conn: &mut PgConnection, track_id: Uuid, at: DateTime<Utc>, key: Option<Uuid>) -> anyhow::Result<Annotation> {
    let mut track = get(conn, Item::Track(track_id)).await?;
    // Only something the user can see (the play log's policy says so too).
    if !sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM tracks WHERE id = $1) AS "e!""#, track_id).fetch_one(&mut *conn).await? {
        return Err(EditRejected::NotFound("track").into());
    }
    // In the play log, with its time. A key it has already: counted before.
    let logged = sqlx::query!(
        "INSERT INTO plays (track_id, played_at, imported_key) VALUES ($1, $2, $3) ON CONFLICT (imported_key) DO NOTHING",
        track_id,
        at,
        key.map(|k| format!("client:{k}")),
    )
    .execute(&mut *conn)
    .await?;
    if logged.rows_affected() == 0 {
        return Ok(track);
    }
    // (An earlier play sent late doesn't move the last play back.)
    let last = |a: &Annotation| a.last_played_at.max(Some(at));
    track.play_count = track.play_count.saturating_add(1);
    track.last_played_at = last(&track);
    put(conn, Item::Track(track_id), &track).await?;

    let albums = sqlx::query_scalar!("SELECT album_id FROM album_tracks WHERE track_id = $1", track_id)
        .fetch_all(&mut *conn)
        .await?;
    let artists = sqlx::query_scalar!(
        "SELECT artist_id FROM track_credits WHERE track_id = $1 AND role = 'primary'",
        track_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let items = albums.into_iter().map(Item::Album).chain(artists.into_iter().map(Item::Artist));
    for item in items {
        let mut a = get(conn, item).await?;
        a.play_count = a.play_count.saturating_add(1);
        a.last_played_at = last(&a);
        // Hidden ones (e.g. an artist only visible through other tracks) are skipped.
        if let Err(err) = put(conn, item, &a).await
            && !matches!(err.downcast_ref::<EditRejected>(), Some(EditRejected::NotFound(_))) {
                return Err(err);
            }
    }
    Ok(track)
}

// ---- Merges ----

/// What a merge did to one user's annotations, to undo it (see `combine_into`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CombinedAnnotation {
    pub user_id: Uuid,
    /// The duplicate's annotation (it stays on the duplicate, which redirects).
    pub dupe: Annotation,
    /// The kept one's before the merge (None: there was none).
    pub kept_before: Option<Annotation>,
    /// What the merge wrote on the kept one.
    pub merged: Annotation,
}

/// Merging `from` into `into`: every user's annotation of `from` is combined into their
/// annotation of `into` (see `Annotation::combined`). `from`'s own rows stay (it's a
/// redirect now). As the system: it's every user's. Returns what to undo.
pub async fn combine_into(conn: &mut PgConnection, from: Item, into: Item) -> anyhow::Result<Vec<CombinedAnnotation>> {
    let dupes = rows_of(conn, from).await?;
    let mut done = Vec::new();
    for (user_id, dupe) in dupes {
        let kept_before = rows_of(conn, into).await?.into_iter().find(|(u, _)| *u == user_id).map(|(_, a)| a);
        let merged = kept_before.clone().unwrap_or_default().combined(&dupe);
        put_for(conn, user_id, into, &merged).await?;
        done.push(CombinedAnnotation { user_id, dupe, kept_before, merged });
    }
    Ok(done)
}

/// Undoing `combine_into`: each user's annotation of the kept thing loses the duplicate's
/// plays. Its star and rating go back to what they were only if they're still what the
/// merge set (so a change made since is kept). A row the merge created that's back to
/// nothing is removed. As the system.
pub async fn split(conn: &mut PgConnection, into: Item, combined: &[CombinedAnnotation]) -> anyhow::Result<()> {
    let current = rows_of(conn, into).await?;
    for c in combined {
        let Some((_, now)) = current.iter().find(|(u, _)| *u == c.user_id) else {
            continue;
        };
        let before = c.kept_before.clone().unwrap_or_default();
        let mut a = now.clone();
        a.play_count = (a.play_count - c.dupe.play_count).max(0);
        if a.last_played_at == c.merged.last_played_at {
            a.last_played_at = before.last_played_at;
        }
        if (a.rating, a.rated_at) == (c.merged.rating, c.merged.rated_at) {
            (a.rating, a.rated_at) = (before.rating, before.rated_at);
        }
        if (a.starred, a.starred_at) == (c.merged.starred, c.merged.starred_at) {
            (a.starred, a.starred_at) = (before.starred, before.starred_at);
        }
        if c.kept_before.is_none() && a == Annotation::default() {
            sqlx::query!(
                "DELETE FROM annotations WHERE user_id = $1 AND (track_id = $2 OR album_id = $2 OR artist_id = $2)",
                c.user_id,
                into.id()
            )
            .execute(&mut *conn)
            .await?;
        } else {
            put_for(conn, c.user_id, into, &a).await?;
        }
    }
    Ok(())
}

/// Every user's annotation of a thing. As the system.
async fn rows_of(conn: &mut PgConnection, item: Item) -> sqlx::Result<Vec<(Uuid, Annotation)>> {
    let rows = sqlx::query!(
        r#"
        SELECT user_id, play_count, last_played_at, rating, rated_at, starred, starred_at
        FROM annotations WHERE (track_id = $1 OR album_id = $1 OR artist_id = $1)
        ORDER BY user_id
        "#,
        item.id()
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let a = Annotation {
                play_count: r.play_count,
                last_played_at: r.last_played_at,
                rating: r.rating,
                rated_at: r.rated_at,
                starred: r.starred,
                starred_at: r.starred_at,
            };
            (r.user_id, a)
        })
        .collect())
}

/// Writes a user's annotation (not necessarily the current user's). As the system: used by
/// merges and by the Navidrome import.
pub async fn put_for(conn: &mut PgConnection, user_id: Uuid, item: Item, a: &Annotation) -> sqlx::Result<()> {
    let (track_id, album_id, artist_id) = item.columns();
    sqlx::query!(
        r#"
        INSERT INTO annotations (user_id, track_id, album_id, artist_id, play_count,
                                 last_played_at, rating, rated_at, starred, starred_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        ON CONFLICT (user_id, (COALESCE(track_id, album_id, artist_id))) DO UPDATE SET
            play_count = EXCLUDED.play_count, last_played_at = EXCLUDED.last_played_at,
            rating = EXCLUDED.rating, rated_at = EXCLUDED.rated_at,
            starred = EXCLUDED.starred, starred_at = EXCLUDED.starred_at
        "#,
        user_id,
        track_id,
        album_id,
        artist_id,
        a.play_count,
        a.last_played_at,
        a.rating,
        a.rated_at,
        a.starred,
        a.starred_at,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// A user's annotation of a thing, as the system (the Navidrome import).
pub async fn get_for(conn: &mut PgConnection, user_id: Uuid, item: Item) -> sqlx::Result<Annotation> {
    Ok(rows_of(conn, item)
        .await?
        .into_iter()
        .find(|(u, _)| *u == user_id)
        .map(|(_, a)| a)
        .unwrap_or_default())
}
