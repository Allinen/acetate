//! A user's own plays, stars and ratings (see `db::annotations`).

use axum::Json;
use axum::extract::{Path, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppError, AppState};
use crate::auth::CurrentUser;
use crate::db;
use crate::db::annotations::{Annotation, Item};
use crate::db::revisions::EditRejected;

/// A changed star or rating, and what it was before this change: a client that sends a
/// change made a while ago (offline) tells from it whether it replaced one made elsewhere
/// meanwhile.
#[derive(Debug, Serialize)]
pub struct Changed {
    #[serde(flatten)]
    annotation: Annotation,
    was_starred: bool,
    was_rating: i16,
}

async fn star(state: AppState, user: CurrentUser, item: Item, starred: bool) -> Result<Json<Changed>, AppError> {
    let mut tx = state.db(&user).await?;
    let was = db::annotations::get(&mut tx, item).await?;
    let annotation = db::annotations::set_starred(&mut tx, item, starred).await?;
    tx.commit().await?;
    Ok(Json(Changed { annotation, was_starred: was.starred, was_rating: was.rating }))
}

/// `POST /api/tracks/{id}/star` (and albums, artists): star it. `DELETE`: unstar.
pub async fn star_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Track(id), true).await
}
pub async fn unstar_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Track(id), false).await
}
pub async fn star_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Album(id), true).await
}
pub async fn unstar_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Album(id), false).await
}
pub async fn star_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Artist(id), true).await
}
pub async fn unstar_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>) -> Result<Json<Changed>, AppError> {
    star(s, u, Item::Artist(id), false).await
}

#[derive(Debug, Deserialize)]
pub struct Rating {
    /// 0 (not rated) to 10: half stars.
    rating: i16,
}

async fn rate(state: AppState, user: CurrentUser, item: Item, rating: i16) -> Result<Json<Changed>, AppError> {
    let mut tx = state.db(&user).await?;
    let was = db::annotations::get(&mut tx, item).await?;
    let annotation = db::annotations::set_rating(&mut tx, item, rating).await?;
    tx.commit().await?;
    Ok(Json(Changed { annotation, was_starred: was.starred, was_rating: was.rating }))
}

/// `PUT /api/tracks/{id}/rating` (and albums, artists), JSON `{"rating": 0..10}`.
pub async fn rate_track(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Changed>, AppError> {
    rate(s, u, Item::Track(id), r.rating).await
}
pub async fn rate_album(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Changed>, AppError> {
    rate(s, u, Item::Album(id), r.rating).await
}
pub async fn rate_artist(State(s): State<AppState>, u: CurrentUser, Path(id): Path<Uuid>, Json(r): Json<Rating>) -> Result<Json<Changed>, AppError> {
    rate(s, u, Item::Artist(id), r.rating).await
}

/// A play's time and the client's key for it (see `played`).
#[derive(Debug, Deserialize)]
pub struct Played {
    played_at: Option<DateTime<Utc>>,
    key: Option<Uuid>,
}

/// How far back a play kept offline may be sent.
const PLAYED_DAYS_BACK: i64 = 90;

/// `POST /api/tracks/{id}/played`: the user listened to it (clients call this after half
/// the track or 4 minutes, whichever comes first). Counts for its albums and primary
/// artists too. Optional JSON `{"played_at", "key"}`: when (a play kept offline, sent
/// later; not in the future, at most 90 days back), and the client's id for it, so one
/// sent twice counts once.
pub async fn played(State(state): State<AppState>, user: CurrentUser, Path(id): Path<Uuid>, body: Option<Json<Played>>) -> Result<Json<Annotation>, AppError> {
    let (at, key) = body.map_or((None, None), |Json(p)| (p.played_at, p.key));
    let at = check_played_at(at, Utc::now())?;
    let mut tx = state.db(&user).await?;
    let annotation = db::annotations::played_at(&mut tx, id, at, key).await?;
    tx.commit().await?;
    Ok(Json(annotation))
}

/// When a play was: as the client says (not in the future, give or take a minute for a
/// clock a little ahead; at most `PLAYED_DAYS_BACK` days back), or now.
fn check_played_at(at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<DateTime<Utc>, EditRejected> {
    let at = at.unwrap_or(now);
    if at > now + chrono::Duration::minutes(1) {
        return Err(EditRejected::Invalid("a play can't be in the future".into()));
    }
    if at < now - chrono::Duration::days(PLAYED_DAYS_BACK) {
        return Err(EditRejected::Invalid(format!("a play can be sent at most {PLAYED_DAYS_BACK} days later")));
    }
    Ok(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_is_in_the_past_and_not_too_far_back() {
        let now = Utc::now();
        assert_eq!(check_played_at(None, now).unwrap(), now);
        let earlier = now - chrono::Duration::days(3);
        assert_eq!(check_played_at(Some(earlier), now).unwrap(), earlier);
        assert!(check_played_at(Some(now + chrono::Duration::seconds(30)), now).is_ok(), "a clock a little ahead");
        assert!(check_played_at(Some(now + chrono::Duration::hours(1)), now).is_err());
        assert!(check_played_at(Some(now - chrono::Duration::days(91)), now).is_err());
    }
}
