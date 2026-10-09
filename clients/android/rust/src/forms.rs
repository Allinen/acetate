//! Albums' and artists' own forms, and an album's track order: loaded with the revision they
//! build on, saved in one request, refused with code `stale` when someone saved meanwhile
//! (the screen offers to load theirs). The rules are `acetate_core::editor::forms`.

use acetate_core::editor::forms::{self, AlbumData, AliasRow, ArtistData, CreditRow};
use uuid::Uuid;

use crate::{Core, CoreError, parse_id};

#[derive(Debug, Clone, uniffi::Record)]
pub struct EditCredit {
    /// None: a new artist called `name`, made when the form is saved.
    pub artist_id: Option<String>,
    pub name: String,
    pub role: String,
    pub alias_id: Option<String>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AlbumForm {
    pub id: String,
    pub title: String,
    pub kind: String,
    /// As written; empty: none.
    pub release_date: String,
    pub credits: Vec<EditCredit>,
    /// The revision this was filled from (sent back with the save).
    pub revision: String,
    pub kinds: Vec<String>,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EditAlias {
    /// None: a new one.
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ArtistForm {
    pub id: String,
    pub name: String,
    pub disambiguation: String,
    pub aliases: Vec<EditAlias>,
    pub revision: String,
}

/// An album's track as the order screen lists it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct OrderRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub disc: Option<i16>,
    pub number: Option<i16>,
}

fn failed(code: &str, reason: &str) -> CoreError {
    CoreError::Failed { code: code.into(), reason: reason.into() }
}

fn id_of(text: &str) -> Result<Uuid, CoreError> {
    parse_id(text).ok_or_else(|| failed("invalid", "That isn't an id."))
}

fn credits_out(rows: Vec<CreditRow>) -> Vec<EditCredit> {
    rows.into_iter().map(|c| EditCredit { artist_id: c.artist_id.map(|a| a.to_string()), name: c.name, role: c.role, alias_id: c.alias_id.map(|a| a.to_string()), alias: c.alias }).collect()
}

fn credits_in(rows: Vec<EditCredit>) -> Vec<CreditRow> {
    rows.into_iter().map(|c| CreditRow { artist_id: c.artist_id.and_then(|a| parse_id(&a)), name: c.name, role: c.role, alias_id: c.alias_id.and_then(|a| parse_id(&a)), alias: c.alias }).collect()
}

#[uniffi::export(async_runtime = "tokio")]
impl Core {
    /// An album as its form is filled, from the server (the revision to build on).
    pub async fn album_form(&self, id: String) -> Result<AlbumForm, CoreError> {
        let (api, id) = (self.shared.api.clone(), id_of(&id)?);
        let (data, base) = self.runtime.spawn(async move { forms::load_album(&api, id).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        let vocab = self.shared.account.lock().unwrap().vocab.clone().unwrap_or_default();
        Ok(AlbumForm {
            id: id.to_string(),
            title: data.title,
            kind: data.kind,
            release_date: data.release_date,
            credits: credits_out(data.credits),
            revision: base.to_string(),
            kinds: vocab.album_kinds.clone(),
            roles: vocab.credit_roles.clone(),
        })
    }

    /// Saves it as one revision (with an optional reason); the new revision. Refused with
    /// code `stale` if someone saved the album since it was filled.
    pub async fn save_album(&self, form: AlbumForm, reason: String) -> Result<String, CoreError> {
        let (api, id, base) = (self.shared.api.clone(), id_of(&form.id)?, id_of(&form.revision)?);
        let data = AlbumData { title: form.title, kind: form.kind, release_date: form.release_date, credits: credits_in(form.credits) };
        let (revision, _) = self.runtime.spawn(async move { forms::save_album(&api, id, base, data, &reason).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(revision.to_string())
    }

    pub async fn artist_form(&self, id: String) -> Result<ArtistForm, CoreError> {
        let (api, id) = (self.shared.api.clone(), id_of(&id)?);
        let (data, base) = self.runtime.spawn(async move { forms::load_artist(&api, id).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(ArtistForm {
            id: id.to_string(),
            name: data.name,
            disambiguation: data.disambiguation,
            aliases: data.aliases.into_iter().map(|a| EditAlias { id: a.id.map(|i| i.to_string()), name: a.name }).collect(),
            revision: base.to_string(),
        })
    }

    pub async fn save_artist(&self, form: ArtistForm, reason: String) -> Result<String, CoreError> {
        let (api, id, base) = (self.shared.api.clone(), id_of(&form.id)?, id_of(&form.revision)?);
        let data = ArtistData {
            name: form.name,
            disambiguation: form.disambiguation,
            aliases: form.aliases.into_iter().map(|a| AliasRow { id: a.id.and_then(|i| parse_id(&i)), name: a.name }).collect(),
        };
        let revision = self.runtime.spawn(async move { forms::save_artist(&api, id, base, &data, &reason).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(revision.to_string())
    }

    /// Where one of an artist's other names is credited, in words (empty: nowhere, so it can
    /// go). A name that's credited stays until those credits are changed.
    pub async fn alias_uses(&self, artist: String, alias: String) -> Result<Vec<String>, CoreError> {
        let (api, artist, alias) = (self.shared.api.clone(), id_of(&artist)?, id_of(&alias)?);
        let uses = self.runtime.spawn(async move { forms::alias_uses(&api, artist, alias).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        let mut lines: Vec<String> = uses.tracks.iter().map(|t| match &t.variant {
            Some(v) => format!("track “{}” ({v})", t.title),
            None => format!("track “{}”", t.title),
        }).collect();
        lines.extend(uses.albums.iter().map(|a| format!("album “{}”", a.title)));
        if uses.not_visible {
            lines.push("others you can't see".into());
        }
        Ok(lines)
    }

    /// An album's tracks in its order, with the places they have.
    pub fn album_order(&self, album: String) -> Vec<OrderRow> {
        let (Some(catalog), Some(album)) = (self.shared.catalog(), parse_id(&album)) else { return vec![] };
        catalog
            .album_tracks(album)
            .iter()
            .map(|&i| {
                let t = catalog.track_at(i);
                let place = t.albums().iter().find(|m| m.0 == album).map_or((None, None), |m| (m.1, m.2));
                OrderRow { id: t.id().to_string(), title: t.title().to_string(), artist: t.artist_display().to_string(), disc: place.0, number: place.1 }
            })
            .collect()
    }

    /// Puts an album's tracks in this order: the places that change are sent, in one request.
    /// How many tracks changed place.
    pub async fn save_order(&self, album: String, order: Vec<String>) -> Result<u32, CoreError> {
        let (api, catalog, album) = (self.shared.api.clone(), self.shared.catalog().ok_or_else(|| failed("no_library", "The library isn't loaded."))?, id_of(&album)?);
        let shown: Vec<(Uuid, forms::Place)> = catalog
            .album_tracks(album)
            .iter()
            .map(|&i| {
                let t = catalog.track_at(i);
                (t.id(), t.albums().iter().find(|m| m.0 == album).map_or((None, None), |m| (m.1, m.2)))
            })
            .collect();
        let wanted: Vec<Uuid> = order.iter().filter_map(|i| parse_id(i)).collect();
        let changes = forms::plan_order(&shown, &wanted);
        if changes.is_empty() {
            return Ok(0);
        }
        let count = changes.len() as u32;
        self.runtime.spawn(async move { forms::save_places(&api, album, &changes).await }).await.map_err(|e| failed("unexpected", &e.to_string()))??;
        Ok(count)
    }

    /// Puts a playlist's entries in this order (all of them), in one request.
    pub fn reorder_playlist(&self, id: String, entries: Vec<String>) {
        let Some(id) = parse_id(&id) else { return };
        let order: Vec<Uuid> = entries.iter().filter_map(|e| parse_id(e)).collect();
        self.shared.playlists.lock().unwrap().set_order(id, order);
        self.shared.listener.changed(crate::Change::Playlists);
    }

    /// Moves a playlist's entry to this place (shown at once, one request).
    pub fn move_in_playlist_to(&self, id: String, entry: String, to: u32) {
        let (Some(id), Some(entry)) = (parse_id(&id), parse_id(&entry)) else { return };
        self.shared.playlists.lock().unwrap().move_entry_to(id, entry, to as isize);
        self.shared.listener.changed(crate::Change::Playlists);
    }
}
