//! Credits of one artist, in bulk, on many tracks and on the albums they're on: credited
//! under another of its names (an alias, or its own name), or moved to another artist (a
//! name that was really someone else's). Optionally only the credits under one of its
//! names ("those credited as “Lil X”"). Each track and album whose credits change gets a
//! revision, as by hand; the rest are left as they are. Other artists' credits are never
//! touched, and no artist or alias is changed itself.
//!
//! The API runs it once to preview (in a transaction it doesn't keep: the exact list of
//! what would change) and again to apply.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use super::revisions::{self, EditRejected, Source};
use crate::model::{AlbumData, Credit};

#[derive(Debug, Serialize, PartialEq)]
pub struct Named {
    pub id: Uuid,
    pub title: String,
}

/// What changed (or would).
#[derive(Debug, Default, Serialize, PartialEq)]
pub struct CreditedAs {
    pub tracks: Vec<Named>,
    pub albums: Vec<Named>,
    /// Tracks crediting the artist that way already.
    pub already: usize,
    /// Tracks not crediting the artist (under the name asked about) at all: left alone,
    /// and their albums aren't looked at.
    pub not_credited: usize,
    /// Tracks not there (merged away since, or not seen).
    pub not_found: usize,
}

/// Which of the artist's credits: under any name, its own, or one alias.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Only {
    All,
    OwnName,
    Alias(Uuid),
}

impl Only {
    fn takes(self, credit: &Credit) -> bool {
        match self {
            Only::All => true,
            Only::OwnName => credit.alias_id.is_none(),
            Only::Alias(alias) => credit.alias_id == Some(alias),
        }
    }
}

/// Credited to `artist`, as `alias` (None: its own name).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub artist: Uuid,
    pub alias: Option<Uuid>,
}

/// `from`'s credits (those `only` takes) on `tracks` (the shown version of each) and on
/// the albums they're on become `to`'s. A credit whose new artist is credited in that
/// role already just goes (once is enough).
pub async fn credit_as(conn: &mut PgConnection, tracks: &[Uuid], from: Uuid, only: Only, to: Target, reason: &str) -> anyhow::Result<CreditedAs> {
    let from_name = artist_name(conn, from).await?;
    if let Only::Alias(alias) = only {
        alias_name(conn, from, alias, &from_name).await?;
    }
    let to_name = if to.artist == from { from_name.clone() } else { artist_name(conn, to.artist).await? };
    let as_name = match to.alias {
        Some(alias) => alias_name(conn, to.artist, alias, &to_name).await?,
        None => to_name.clone(),
    };
    let reason = if reason.trim().is_empty() {
        let which = match only {
            Only::All => String::new(),
            Only::OwnName => " (under its own name)".to_string(),
            Only::Alias(alias) => format!(" (as \"{}\")", alias_name(conn, from, alias, &from_name).await?),
        };
        if to.artist == from {
            format!("{from_name}{which} credited as \"{as_name}\"")
        } else if to.alias.is_some() {
            format!("{from_name}{which} credited to {to_name} instead, as \"{as_name}\"")
        } else {
            format!("{from_name}{which} credited to {to_name} instead")
        }
    } else {
        reason.trim().to_string()
    };
    let mut out = CreditedAs::default();
    let mut albums: Vec<Uuid> = Vec::new();
    for &track in tracks {
        let Some(row) = sqlx::query!("SELECT title, primary_variant_id FROM tracks WHERE id = $1 AND merged_into IS NULL", track)
            .fetch_optional(&mut *conn)
            .await?
        else {
            out.not_found += 1;
            continue;
        };
        let Some(variant) = row.primary_variant_id else { continue };
        let Some((_, mut meta)) = revisions::latest_internal(conn, track, variant).await? else { continue };
        let (taken, changed) = recredit(&mut meta.credits, from, only, to);
        if !taken {
            out.not_credited += 1;
            continue;
        }
        if changed {
            revisions::write_internal(conn, track, variant, None, &meta, Source::Edit, &reason, false).await?;
            out.tracks.push(Named { id: track, title: row.title });
        } else {
            out.already += 1;
        }
        let on = sqlx::query_scalar!(
            "SELECT at.album_id FROM album_tracks at JOIN albums a ON a.id = at.album_id AND a.merged_into IS NULL WHERE at.track_id = $1",
            track
        )
        .fetch_all(&mut *conn)
        .await?;
        for album in on {
            if !albums.contains(&album) {
                albums.push(album);
            }
        }
    }
    for album in albums {
        let Some(latest) = sqlx::query!(
            "SELECT r.data, a.title FROM album_revisions r JOIN albums a ON a.id = r.album_id WHERE r.album_id = $1 ORDER BY r.id DESC LIMIT 1",
            album
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            continue;
        };
        let mut data: AlbumData = serde_json::from_value(latest.data)?;
        if recredit(&mut data.credits, from, only, to).1 {
            revisions::write_album(conn, album, None, &data, Source::Edit, &reason).await?;
            out.albums.push(Named { id: album, title: latest.title });
        }
    }
    Ok(out)
}

/// `from`'s credits that `only` takes become `to`'s. Returns whether any was taken, and
/// whether anything changed.
fn recredit(credits: &mut Vec<Credit>, from: Uuid, only: Only, to: Target) -> (bool, bool) {
    let (mut taken, mut changed) = (false, false);
    let mut n = 0;
    while n < credits.len() {
        let c = &credits[n];
        if c.artist_id != from || !only.takes(c) {
            n += 1;
            continue;
        }
        taken = true;
        if (c.artist_id, c.alias_id) == (to.artist, to.alias) {
            n += 1;
            continue;
        }
        changed = true;
        let role = c.role.clone();
        if to.artist != from && credits.iter().any(|x| x.artist_id == to.artist && x.role == role) {
            credits.remove(n);
        } else {
            credits[n] = Credit { artist_id: to.artist, role, alias_id: to.alias };
            n += 1;
        }
    }
    (taken, changed)
}

async fn artist_name(conn: &mut PgConnection, artist: Uuid) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar!("SELECT name FROM artists WHERE id = $1 AND merged_into IS NULL", artist)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::NotFound("artist"))?)
}

/// The alias's name, if it's one of the artist's.
async fn alias_name(conn: &mut PgConnection, artist: Uuid, alias: Uuid, artist_name: &str) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar!("SELECT alias FROM artist_aliases WHERE id = $1 AND artist_id = $2 AND merged_into IS NULL", alias, artist)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(EditRejected::Invalid(format!("that name isn't one of {artist_name}'s")))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit(artist: u128, role: &str, alias: Option<u128>) -> Credit {
        Credit { artist_id: Uuid::from_u128(artist), role: role.into(), alias_id: alias.map(Uuid::from_u128) }
    }

    #[test]
    fn credits_move_by_name() {
        let to = Target { artist: Uuid::from_u128(2), alias: None };
        // Only those as alias 10 move; the producer credit (own name) stays.
        let mut credits = vec![credit(1, "primary", Some(10)), credit(1, "producer", None)];
        assert_eq!(recredit(&mut credits, Uuid::from_u128(1), Only::Alias(Uuid::from_u128(10)), to), (true, true));
        assert_eq!(credits, [credit(2, "primary", None), credit(1, "producer", None)]);
        // The other artist credited in that role already: the credit just goes.
        let mut credits = vec![credit(1, "primary", None), credit(2, "primary", None)];
        assert_eq!(recredit(&mut credits, Uuid::from_u128(1), Only::All, to), (true, true));
        assert_eq!(credits, [credit(2, "primary", None)]);
        // Not credited under that name: nothing taken.
        let mut credits = vec![credit(1, "primary", None)];
        assert_eq!(recredit(&mut credits, Uuid::from_u128(1), Only::Alias(Uuid::from_u128(10)), to), (false, false));
    }
}
