//! Domain types shared across modules. No IO in here.
//!
//! The `...Data`/`TrackMetadata`/`TrackFacts` types are exactly what's stored in the
//! `data` column of revisions (as JSON). They're all sparse: only fields with a value are
//! stored, and every field has a default when reading. That makes old revisions forward
//! compatible: a field added later is simply missing from them and reads as its default,
//! no data migration needed. Fields a newer version wrote are ignored by an older one.

pub mod sources;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A variant's own metadata: the data of an *internal* track revision.
///
/// The primary variant's latest one is copied onto `tracks` (title, date, genres) and
/// `track_credits`; the artist text is derived from the credits.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackMetadata {
    pub title: String,
    /// Verbatim from the source, e.g. '2019', '2019-05-12T11:11:11Z'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// In display order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
}

/// An artist credited on a track or album.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credit {
    pub artist_id: Uuid,
    /// "primary", "featured", "producer"... (the `credit_roles` table).
    pub role: String,
    /// The alias this is credited under (one of the artist's). None = the artist's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_id: Option<Uuid>,
}

/// The track's place in the world, the same whatever variant is shown: the data of an
/// *external* track revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackFacts {
    /// "released", "unreleased"... (the `track_statuses` table).
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<AlbumMembership>,
    /// Seen from this track: `{kind: "remix_of", track_id: B}` = "this is a remix of B".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

// Written by hand instead of derived, so a missing status reads as "released" (the
// column default) rather than "".
impl Default for TrackFacts {
    fn default() -> Self {
        TrackFacts {
            status: "released".to_string(),
            albums: Vec::new(),
            relations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumMembership {
    pub album_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i16>,
}

// `Eq` + `Hash` so relations can go in a HashSet (see `diff_relations`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Relation {
    /// "remix_of", "alternate"... (the `relation_kinds` table).
    pub kind: String,
    pub track_id: Uuid,
}

/// The data of an artist revision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtistData {
    pub name: String,
    /// Tells apart artists with the same name, e.g. "US rapper".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub disambiguation: String,
    /// Other names the artist appears under (case variants included).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Alias>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alias {
    /// Stable, referred to by credits. None only in edits, for a new alias; stored
    /// revisions always have it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
    pub name: String,
}

/// The data of an album revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlbumData {
    pub title: String,
    /// "album", "ep", "single"... (the `album_kinds` table).
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
}

impl Default for AlbumData {
    fn default() -> Self {
        AlbumData {
            title: String::new(),
            kind: "album".to_string(),
            release_date: None,
            credits: Vec::new(),
        }
    }
}

/// The data of a lyrics revision: one lyrics entry of a track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsData {
    pub body: String,
    /// From the artist/label, as opposed to transcribed by someone.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_official: bool,
    /// e.g. "en", "ja". Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl LyricsData {
    /// LRC-style synced lyrics (lines like "[01:23.45] text"). Derived from the body rather
    /// than stored, so it can't disagree with it.
    pub fn is_synced(&self) -> bool {
        looks_synced(&self.body)
    }
}

/// Whether lyrics have LRC-style timestamps: lines like "[01:23.45] text".
pub fn looks_synced(body: &str) -> bool {
    body.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with('[') && line[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    })
}

// ---- Validation ----
//
// Checks that need no database. Whether ids and vocabulary values (roles, statuses,
// kinds) exist is checked by `db::revisions` before anything is written. Messages are
// meant for the client.

impl TrackMetadata {
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("title can't be empty".to_string());
        }
        validate_credits(&self.credits)
    }
}

impl TrackFacts {
    /// `track_id`: the track these facts are about.
    pub fn validate(&self, track_id: Uuid) -> Result<(), String> {
        if self.status.is_empty() {
            return Err("status can't be empty".to_string());
        }
        let mut albums = HashSet::new();
        for album in &self.albums {
            if !albums.insert(album.album_id) {
                return Err(format!("album {} is listed twice", album.album_id));
            }
        }
        let mut relations = HashSet::new();
        for relation in &self.relations {
            if relation.track_id == track_id {
                return Err("a track can't be related to itself".to_string());
            }
            if !relations.insert(relation) {
                return Err(format!(
                    "relation {} {} is listed twice",
                    relation.kind, relation.track_id
                ));
            }
        }
        Ok(())
    }
}

impl ArtistData {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name can't be empty".to_string());
        }
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for alias in &self.aliases {
            if alias.name.trim().is_empty() {
                return Err("aliases can't be empty".to_string());
            }
            if alias.name == self.name {
                return Err(format!("`{}` is the artist's name, not an alias", alias.name));
            }
            // Exact duplicates only: case variants are separate aliases on purpose.
            if !names.insert(alias.name.as_str()) {
                return Err(format!("alias `{}` is listed twice", alias.name));
            }
            if let Some(id) = alias.id
                && !ids.insert(id)
            {
                return Err(format!("alias {id} is listed twice"));
            }
        }
        Ok(())
    }
}

impl LyricsData {
    pub fn validate(&self) -> Result<(), String> {
        if self.body.trim().is_empty() {
            return Err("lyrics can't be empty".to_string());
        }
        Ok(())
    }
}

impl AlbumData {
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("title can't be empty".to_string());
        }
        validate_credits(&self.credits)
    }
}

fn validate_credits(credits: &[Credit]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for credit in credits {
        if !seen.insert((credit.artist_id, credit.role.as_str())) {
            return Err(format!(
                "artist {} is credited as {} twice",
                credit.artist_id, credit.role
            ));
        }
    }
    Ok(())
}

/// What changes between two sets of relations: (added, removed).
pub fn diff_relations(old: &[Relation], new: &[Relation]) -> (Vec<Relation>, Vec<Relation>) {
    let old_set: HashSet<&Relation> = old.iter().collect();
    let new_set: HashSet<&Relation> = new.iter().collect();
    let added = new.iter().filter(|r| !old_set.contains(r)).cloned().collect();
    let removed = old.iter().filter(|r| !new_set.contains(r)).cloned().collect();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn credit(artist: u128, role: &str) -> Credit {
        Credit {
            artist_id: id(artist),
            role: role.to_string(),
            alias_id: None,
        }
    }

    #[test]
    fn only_set_fields_are_stored() {
        let meta = TrackMetadata {
            title: "Song".into(),
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&meta).unwrap(), json!({ "title": "Song" }));

        let meta = TrackMetadata {
            title: "Song".into(),
            credits: vec![credit(1, "primary")],
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&meta).unwrap(),
            json!({ "title": "Song", "credits": [{ "artist_id": id(1), "role": "primary" }] })
        );
    }

    #[test]
    fn missing_fields_read_as_defaults() {
        let meta: TrackMetadata = serde_json::from_value(json!({ "title": "Song" })).unwrap();
        assert_eq!(meta.release_date, None);
        assert!(meta.genres.is_empty());
        assert!(meta.credits.is_empty());
        // Fields this version doesn't know about (e.g. written by a newer one) are ignored.
        let meta: TrackMetadata =
            serde_json::from_value(json!({ "title": "Song", "future_field": 1 })).unwrap();
        assert_eq!(meta.title, "Song");

        let facts: TrackFacts = serde_json::from_value(json!({})).unwrap();
        assert_eq!(facts.status, "released");
        let album: AlbumData = serde_json::from_value(json!({ "title": "A" })).unwrap();
        assert_eq!(album.kind, "album");
    }

    #[test]
    fn validation() {
        let mut meta = TrackMetadata {
            title: "  ".into(),
            ..Default::default()
        };
        assert!(meta.validate().is_err());
        meta.title = "Song".into();
        meta.credits = vec![credit(1, "primary"), credit(1, "featured")];
        assert!(meta.validate().is_ok(), "same artist, different roles is fine");
        meta.credits.push(credit(1, "primary"));
        assert!(meta.validate().is_err());

        let facts = TrackFacts {
            relations: vec![Relation { kind: "remix_of".into(), track_id: id(7) }],
            ..Default::default()
        };
        assert!(facts.validate(id(8)).is_ok());
        assert!(facts.validate(id(7)).is_err(), "related to itself");

        let alias = |name: &str| Alias { id: None, name: name.into() };
        let mut artist = ArtistData {
            name: "A".into(),
            aliases: vec![alias("Alias"), alias("alias")],
            ..Default::default()
        };
        assert!(artist.validate().is_ok(), "case variants are separate aliases");
        artist.aliases.push(alias("Alias"));
        assert!(artist.validate().is_err(), "exact duplicate");
        artist.aliases = vec![alias("A")];
        assert!(artist.validate().is_err(), "the name itself");
    }

    #[test]
    fn synced_detection() {
        assert!(looks_synced("[00:12.34] hello\n[00:15.00] world"));
        assert!(!looks_synced("hello\nworld"));
        assert!(!looks_synced("[Chorus]\nla la la"));
    }

    #[test]
    fn relation_diff() {
        let r = |kind: &str, n| Relation { kind: kind.into(), track_id: id(n) };
        let old = vec![r("remix_of", 1), r("alternate", 2)];
        let new = vec![r("alternate", 2), r("live_of", 3)];
        let (added, removed) = diff_relations(&old, &new);
        assert_eq!(added, vec![r("live_of", 3)]);
        assert_eq!(removed, vec![r("remix_of", 1)]);
    }
}
