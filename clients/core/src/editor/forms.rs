//! Albums' and artists' own forms, with no UI: the data a form edits, the bodies the server
//! takes, loading what a form is filled from (with the revision it builds on) and saving it.
//! A save is refused (`stale`) when someone else saved the same thing meanwhile: the caller
//! offers to load theirs. Also the credits helpers (a track's or an album's artists with
//! their roles, new artists made when the form is saved) and an album's track order.

use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

use crate::api::{Api, ApiError};
use crate::details::{AlbumDetail, ArtistDetail};

#[derive(Debug, Clone, PartialEq)]
pub struct CreditRow {
    /// None: a new artist named `name`, made when the form is saved.
    pub artist_id: Option<Uuid>,
    /// The artist's name, and the alias's (for showing).
    pub name: String,
    pub role: String,
    pub alias_id: Option<Uuid>,
    pub alias: Option<String>,
}

/// Credits an artist (None: a new one): as primary if there's no primary yet, else as
/// featured (`featured`: always featured). Not twice.
pub fn add_artist(rows: &mut Vec<CreditRow>, id: Option<Uuid>, name: String, featured: bool) {
    if id.is_some() && rows.iter().any(|c| c.artist_id == id) {
        return;
    }
    let has_primary = rows.iter().any(|c| c.role == "primary");
    let role = if featured || has_primary { "featured" } else { "primary" };
    rows.push(CreditRow { artist_id: id, name, role: role.into(), alias_id: None, alias: None });
}

/// Rows from a details page's credits (a track's variant, an album): names as known.
pub fn rows_from(credits: &[crate::details::Credit]) -> Vec<CreditRow> {
    credits
        .iter()
        .map(|c| CreditRow { artist_id: Some(c.artist_id), name: c.artist_name.clone(), role: c.role.clone(), alias_id: c.alias_id, alias: c.alias.clone() })
        .collect()
}

/// Makes the new artists (rows without an id), one per name even if credited twice, and
/// fills in their ids. On an error, the ones made so far keep theirs.
pub async fn make_new_artists(api: &Api, rows: &mut [CreditRow]) -> Result<(), ApiError> {
    #[derive(serde::Deserialize)]
    struct Created {
        id: Uuid,
    }
    let mut made: Vec<(String, Uuid)> = Vec::new();
    for row in rows.iter_mut().filter(|r| r.artist_id.is_none()) {
        let id = match made.iter().find(|(n, _)| *n == row.name) {
            Some((_, id)) => *id,
            None => {
                let created: Created = api.send_json(reqwest::Method::POST, "/api/artists", Some(serde_json::json!({ "name": row.name }))).await?;
                made.push((row.name.clone(), created.id));
                created.id
            }
        };
        row.artist_id = Some(id);
    }
    Ok(())
}

/// The credits as the server takes them.
pub fn to_json(rows: &[CreditRow]) -> Vec<serde_json::Value> {
    rows.iter().map(|c| serde_json::json!({ "artist_id": c.artist_id, "role": c.role, "alias_id": c.alias_id })).collect()
}

/// After a save: rows still without an id get the one `sent` has for their name (the
/// artist was made).
pub fn take_ids(rows: &mut [CreditRow], sent: &[CreditRow]) {
    for row in rows.iter_mut().filter(|r| r.artist_id.is_none()) {
        row.artist_id = sent.iter().find(|s| s.name == row.name).and_then(|s| s.artist_id);
    }
}

/// The album's data, as the form edits it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlbumData {
    pub title: String,
    pub kind: String,
    /// As written (verbatim); empty: none.
    pub release_date: String,
    pub credits: Vec<CreditRow>,
}

/// The album as the server takes it.
pub fn album_body(data: &AlbumData, base: Uuid, reason: &str) -> serde_json::Value {
    let date = Some(data.release_date.trim()).filter(|d| !d.is_empty());
    serde_json::json!({
        "base_revision_id": base,
        "title": data.title.trim(),
        "kind": data.kind,
        "release_date": date,
        "credits": to_json(&data.credits),
        "reason": reason.trim(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliasRow {
    /// None: a new one.
    pub id: Option<Uuid>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArtistData {
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<AliasRow>,
}

/// Where an alias is credited, as the server says (only what the user may see, and
/// whether there's more).
#[derive(Debug, Clone, Deserialize)]
pub struct Uses {
    #[serde(default)]
    pub tracks: Vec<TrackUse>,
    #[serde(default)]
    pub albums: Vec<AlbumUse>,
    #[serde(default)]
    pub not_visible: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TrackUse {
    pub track_id: Uuid,
    pub title: String,
    /// The version's label, if not the track's main one.
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumUse {
    pub album_id: Uuid,
    pub title: String,
}

impl Uses {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.albums.is_empty() && !self.not_visible
    }
}

/// The artist as the server takes it.
pub fn artist_body(data: &ArtistData, base: Uuid, reason: &str) -> serde_json::Value {
    let aliases: Vec<serde_json::Value> = data
        .aliases
        .iter()
        .filter(|a| !a.name.trim().is_empty())
        .map(|a| match a.id {
            Some(id) => serde_json::json!({ "id": id, "name": a.name.trim() }),
            None => serde_json::json!({ "name": a.name.trim() }),
        })
        .collect();
    serde_json::json!({
        "base_revision_id": base,
        "name": data.name.trim(),
        "disambiguation": data.disambiguation.trim(),
        "aliases": aliases,
        "reason": reason.trim(),
    })
}

#[derive(Deserialize)]
struct Written {
    revision_id: Uuid,
}

/// An album as a form is filled: its data and the revision it builds on.
pub async fn load_album(api: &Api, id: Uuid) -> Result<(AlbumData, Uuid), ApiError> {
    let d: AlbumDetail = api.send_json(reqwest::Method::GET, &format!("/api/albums/{id}"), None).await?;
    let base = d.revision_id.ok_or_else(|| ApiError { status: None, extra_id: None, code: "no_revision".into(), message: "This album has no revision to edit.".into() })?;
    Ok((AlbumData { title: d.title, kind: d.kind, release_date: d.release_date.unwrap_or_default(), credits: rows_from(&d.credits) }, base))
}

/// Saves an album (new artists made first): the new revision, and the data as sent (the new
/// artists' ids in it).
pub async fn save_album(api: &Api, id: Uuid, base: Uuid, mut data: AlbumData, reason: &str) -> Result<(Uuid, AlbumData), ApiError> {
    if data.title.trim().is_empty() {
        return Err(ApiError { status: None, extra_id: None, code: "invalid".into(), message: "An album needs a title.".into() });
    }
    make_new_artists(api, &mut data.credits).await?;
    let written: Written = api.send_json(reqwest::Method::PUT, &format!("/api/albums/{id}"), Some(album_body(&data, base, reason))).await?;
    Ok((written.revision_id, data))
}

/// An artist as a form is filled.
pub async fn load_artist(api: &Api, id: Uuid) -> Result<(ArtistData, Uuid), ApiError> {
    let d: ArtistDetail = api.send_json(reqwest::Method::GET, &format!("/api/artists/{id}"), None).await?;
    let base = d.revision_id.ok_or_else(|| ApiError { status: None, extra_id: None, code: "no_revision".into(), message: "This artist has no revision to edit.".into() })?;
    Ok((ArtistData { name: d.name, disambiguation: d.disambiguation, aliases: d.aliases.into_iter().map(|a| AliasRow { id: Some(a.id), name: a.name }).collect() }, base))
}

pub async fn save_artist(api: &Api, id: Uuid, base: Uuid, data: &ArtistData, reason: &str) -> Result<Uuid, ApiError> {
    if data.name.trim().is_empty() {
        return Err(ApiError { status: None, extra_id: None, code: "invalid".into(), message: "An artist needs a name.".into() });
    }
    let written: Written = api.send_json(reqwest::Method::PUT, &format!("/api/artists/{id}"), Some(artist_body(data, base, reason))).await?;
    Ok(written.revision_id)
}

/// Where one of an artist's aliases is credited (it can only go when nowhere).
pub async fn alias_uses(api: &Api, artist: Uuid, alias: Uuid) -> Result<Uses, ApiError> {
    api.send_json(reqwest::Method::GET, &format!("/api/artists/{artist}/aliases/{alias}/uses"), None).await
}

/// A track's place on an album: its disc and number.
pub type Place = (Option<i16>, Option<i16>);

/// The places that change when an album's tracks are shown in a new order. `shown`: the
/// tracks in the order they are now, with their places. The places the album has are handed
/// out again in the new order (so the numbers stay what they were, with gaps and discs); when
/// they can't be (a track has no number, or two share one), the tracks are numbered 1, 2, 3…
/// as shown, on the discs they're on. Only the tracks whose place changes are returned.
pub fn plan_order(shown: &[(Uuid, Place)], new_order: &[Uuid]) -> Vec<(Uuid, Place)> {
    let disc = |p: Place| p.0.unwrap_or(1);
    let place_of: HashMap<Uuid, Place> = shown.iter().copied().collect();
    let order: Vec<Uuid> = new_order.iter().copied().filter(|id| place_of.contains_key(id)).collect();
    let mut slots: Vec<Place> = order.iter().map(|id| place_of[id]).collect();
    slots.sort_by_key(|p| (disc(*p), p.1));
    let numbered = slots.iter().all(|p| p.1.is_some()) && slots.windows(2).all(|w| w[0] != w[1] && (disc(w[0]), w[0].1) != (disc(w[1]), w[1].1));
    let places: Vec<Place> = if numbered {
        slots
    } else {
        // As shown: each disc's tracks 1, 2, 3… (the disc written the way they have it, 1 for
        // those with none when some have one).
        let any_disc = shown.iter().any(|(_, p)| p.0.is_some());
        let mut counts: HashMap<i16, i16> = HashMap::new();
        order
            .iter()
            .map(|id| {
                let d = disc(place_of[id]);
                let n = counts.entry(d).or_insert(0);
                *n += 1;
                (if any_disc { Some(d) } else { None }, Some(*n))
            })
            .collect()
    };
    order.iter().zip(places).filter(|(id, new)| place_of[*id] != *new).map(|(id, new)| (*id, new)).collect()
}

/// Sends the new places of an album's tracks, in one request.
pub async fn save_places(api: &Api, album: Uuid, changes: &[(Uuid, Place)]) -> Result<(), ApiError> {
    let tracks: Vec<serde_json::Value> = changes.iter().map(|(id, (disc, number))| serde_json::json!({ "track_id": id, "disc_number": disc, "track_number": number })).collect();
    api.send(reqwest::Method::PUT, &format!("/api/albums/{album}/track-numbers"), Some(serde_json::json!({ "tracks": tracks }))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn a_track_moved_takes_the_places_in_between() {
        // 1 2 3 4 with a gap; the last goes first: every number shifts, none is invented.
        let shown = [(id(1), (None, Some(1))), (id(2), (None, Some(2))), (id(3), (None, Some(3))), (id(4), (None, Some(5)))];
        let changes = plan_order(&shown, &[id(4), id(1), id(2), id(3)]);
        assert_eq!(changes, vec![(id(4), (None, Some(1))), (id(1), (None, Some(2))), (id(2), (None, Some(3))), (id(3), (None, Some(5)))]);
        // Nothing moved, nothing to send.
        assert!(plan_order(&shown, &[id(1), id(2), id(3), id(4)]).is_empty());
    }

    #[test]
    fn tracks_without_numbers_are_numbered_as_shown() {
        let shown = [(id(1), (None, None)), (id(2), (None, None)), (id(3), (None, None))];
        assert_eq!(plan_order(&shown, &[id(2), id(1), id(3)]), vec![(id(2), (None, Some(1))), (id(1), (None, Some(2))), (id(3), (None, Some(3)))]);
        // Two on one number: the same.
        let shown = [(id(1), (Some(1), Some(1))), (id(2), (Some(1), Some(1)))];
        assert_eq!(plan_order(&shown, &[id(2), id(1)]), vec![(id(1), (Some(1), Some(2)))]);
    }

    #[test]
    fn the_bodies_are_what_the_server_takes() {
        let data = ArtistData { name: " Ann ".into(), disambiguation: "UK".into(), aliases: vec![AliasRow { id: Some(id(7)), name: "Annie".into() }, AliasRow { id: None, name: "  ".into() }] };
        let body = artist_body(&data, id(1), " why ");
        assert_eq!(body["name"], "Ann");
        assert_eq!(body["aliases"].as_array().unwrap().len(), 1);
        assert_eq!(body["reason"], "why");
    }
}
