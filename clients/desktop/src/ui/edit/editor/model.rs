//! The editor's model: a track's metadata (its shown version's) and facts as the server
//! keeps them, and the changes typed in the editor (operations), applied to them.
//!
//! An edit is a list of operations, not a form's fields: the same list goes on each
//! target track's own current state, which is how one edit changes one track or a
//! thousand, and how a track someone changed meanwhile gets the edit again on their
//! version. `apply` is the only place an operation means something.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::details::TrackDetail;

/// A version's metadata (the server's `TrackMetadata`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credits: Vec<Credit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credit {
    pub artist_id: Uuid,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_id: Option<Uuid>,
}

/// The track's facts (the server's `TrackFacts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub albums: Vec<Membership>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Membership {
    pub album_id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disc_number: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_number: Option<i16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub kind: String,
    pub track_id: Uuid,
}

/// A track as the editor starts from it: its shown version (id, latest revision,
/// metadata) and its facts (latest revision).
#[derive(Debug, Clone)]
pub struct Base {
    pub variant: Uuid,
    pub variant_base: Uuid,
    pub meta: Meta,
    pub facts_base: Option<Uuid>,
    pub facts: Facts,
}

impl Base {
    /// From the track's details, editing `variant` (None, or one it doesn't have: the
    /// shown one).
    pub fn for_variant(d: &TrackDetail, variant: Option<Uuid>) -> Option<Base> {
        let v = variant
            .and_then(|id| d.variants.iter().find(|v| v.id == id))
            .or_else(|| d.primary_variant_id.and_then(|id| d.variants.iter().find(|v| v.id == id)))
            .or(d.variants.first())?;
        let meta: Meta = serde_json::from_value(v.data.clone()).unwrap_or_default();
        let facts = Facts {
            status: d.status.clone(),
            albums: d.albums.iter().map(|a| Membership { album_id: a.album_id, disc_number: a.disc_number, track_number: a.track_number }).collect(),
            relations: d.relations.iter().map(|r| Relation { kind: r.kind.clone(), track_id: r.track_id }).collect(),
        };
        Some(Base { variant: v.id, variant_base: v.revision_id, meta, facts_base: d.external_revision_id, facts })
    }
}

/// Track numbers on an album, for `Op::Album`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Numbers {
    /// As they are (none on a new membership).
    Keep,
    /// These (None: as they are).
    At { disc: Option<i16>, number: Option<i16> },
    /// In the order of the targets, from this number (disc 1 where it has none).
    InOrder(i16),
}

/// One change, as typed.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Title(String),
    /// None: no date.
    Date(Option<String>),
    Comment(Option<String>),
    Status(String),
    Genre(String),
    NoGenre(String),
    NoGenres,
    /// Credits an artist. `role` None: primary if the track has none, else featured (an
    /// artist credited already in some role: nothing new). `alias`: Some(None) its own
    /// name, Some(Some(id)) an alias, None as it is (its own name if new). `at`: its place
    /// (0 first; past the end: last).
    Credit { artist: Uuid, role: Option<String>, alias: Option<Option<Uuid>>, at: Option<usize> },
    /// Takes the artist off (in one role, or all of them).
    Uncredit { artist: Uuid, role: Option<String> },
    NoCredits,
    Album { album: Uuid, numbers: Numbers },
    NoAlbum(Uuid),
    NoAlbums,
    Relate { track: Uuid, kind: String },
    Unrelate(Uuid),
    NoRelations,
    /// Into a grouping: not a revision; done after the rest is written.
    Group(Uuid),
    /// Whether albums it's alone on change along (the last one says).
    Along(bool),
    /// Albums it's alone on are made to match it, changed or not (title, date, performers).
    Sync,
    /// Why (the last one says): kept with every revision.
    Why(String),
}

/// `ops` on a track's metadata and facts. `position`: its place among the targets (for
/// numbering in order).
pub fn apply(ops: &[Op], meta: &Meta, facts: &Facts, position: usize) -> (Meta, Facts) {
    let (mut meta, mut facts) = (meta.clone(), facts.clone());
    let text = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
    for op in ops {
        match op {
            Op::Title(title) => meta.title = title.trim().to_string(),
            Op::Date(date) => meta.release_date = date.as_deref().and_then(text),
            Op::Comment(comment) => meta.comment = comment.as_deref().and_then(text),
            Op::Status(status) => facts.status = status.clone(),
            Op::Genre(genre) => {
                if !meta.genres.iter().any(|g| g.eq_ignore_ascii_case(genre.trim())) {
                    meta.genres.push(genre.trim().to_string());
                }
            }
            Op::NoGenre(genre) => meta.genres.retain(|g| !g.eq_ignore_ascii_case(genre.trim())),
            Op::NoGenres => meta.genres.clear(),
            Op::Credit { artist, role, alias, at } => credit(&mut meta.credits, *artist, role.as_deref(), *alias, *at),
            Op::Uncredit { artist, role } => meta.credits.retain(|c| !(c.artist_id == *artist && role.as_ref().is_none_or(|r| *r == c.role))),
            Op::NoCredits => meta.credits.clear(),
            Op::Album { album, numbers } => {
                let at = match facts.albums.iter().position(|m| m.album_id == *album) {
                    Some(at) => at,
                    None => {
                        facts.albums.push(Membership { album_id: *album, disc_number: None, track_number: None });
                        facts.albums.len() - 1
                    }
                };
                let m = &mut facts.albums[at];
                match numbers {
                    Numbers::Keep => {}
                    Numbers::At { disc, number } => {
                        if disc.is_some() {
                            m.disc_number = *disc;
                        }
                        if number.is_some() {
                            m.track_number = *number;
                        }
                    }
                    Numbers::InOrder(from) => {
                        m.disc_number = m.disc_number.or(Some(1));
                        m.track_number = Some(from.saturating_add(position.min(i16::MAX as usize) as i16));
                    }
                }
            }
            Op::NoAlbum(album) => facts.albums.retain(|m| m.album_id != *album),
            Op::NoAlbums => facts.albums.clear(),
            Op::Relate { track, kind } => {
                if !facts.relations.iter().any(|r| r.track_id == *track && r.kind == *kind) {
                    facts.relations.push(Relation { kind: kind.clone(), track_id: *track });
                }
            }
            Op::Unrelate(track) => facts.relations.retain(|r| r.track_id != *track),
            Op::NoRelations => facts.relations.clear(),
            Op::Group(_) | Op::Along(_) | Op::Sync | Op::Why(_) => {}
        }
    }
    (meta, facts)
}

fn credit(credits: &mut Vec<Credit>, artist: Uuid, role: Option<&str>, alias: Option<Option<Uuid>>, at: Option<usize>) {
    let existing = match role {
        Some(role) => credits.iter().position(|c| c.artist_id == artist && c.role == role),
        // No role given: the artist as credited already, in any role.
        None => credits.iter().position(|c| c.artist_id == artist),
    };
    let index = match existing {
        Some(i) => {
            if let Some(alias) = alias {
                credits[i].alias_id = alias;
            }
            i
        }
        None => {
            let role = role.map(str::to_string).unwrap_or_else(|| {
                if credits.iter().any(|c| c.role == "primary") { "featured".into() } else { "primary".into() }
            });
            credits.push(Credit { artist_id: artist, role, alias_id: alias.flatten() });
            credits.len() - 1
        }
    };
    if let Some(at) = at {
        let row = credits.remove(index);
        credits.insert(at.min(credits.len()), row);
    }
}

/// The groupings `ops` put tracks in.
pub fn groupings(ops: &[Op]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for op in ops {
        if let Op::Group(g) = op
            && !out.contains(g)
        {
            out.push(*g);
        }
    }
    out
}

/// Whether albums change along (on unless an `along:off` says otherwise, the last one
/// winning).
pub fn along(ops: &[Op]) -> bool {
    ops.iter().rev().find_map(|op| match op {
        Op::Along(on) => Some(*on),
        Op::Sync => Some(true),
        _ => None,
    }).unwrap_or(true)
}

/// Whether the albums are made to match (an `along:sync` not turned off after it).
pub fn sync(ops: &[Op]) -> bool {
    ops.iter().rev().find_map(|op| match op {
        Op::Sync => Some(true),
        Op::Along(false) => Some(false),
        _ => None,
    }).unwrap_or(false)
}

/// The reason given (the last `why:`), or empty.
pub fn why(ops: &[Op]) -> String {
    ops.iter().rev().find_map(|op| if let Op::Why(w) = op { Some(w.trim().to_string()) } else { None }).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn c(n: u128, role: &str) -> Credit {
        Credit { artist_id: id(n), role: role.into(), alias_id: None }
    }

    fn facts() -> Facts {
        Facts { status: "released".into(), albums: vec![], relations: vec![] }
    }

    #[test]
    fn credits_take_their_role_and_place() {
        let meta = Meta { title: "Song".into(), ..Default::default() };
        let add = |artist, role: Option<&str>, at| Op::Credit { artist: id(artist), role: role.map(str::to_string), alias: None, at };
        // The first is primary, the next featured; one already credited isn't twice.
        let (m, _) = apply(&[add(1, None, None), add(2, None, None), add(1, None, None)], &meta, &facts(), 0);
        assert_eq!(m.credits, [c(1, "primary"), c(2, "featured")]);
        // The same artist in another role: a second credit. In the same role: still one.
        let (m, _) = apply(&[add(1, None, None), add(1, Some("producer"), None), add(1, Some("producer"), None)], &meta, &facts(), 0);
        assert_eq!(m.credits, [c(1, "primary"), c(1, "producer")]);
        // A place: first; past the end: last.
        let (m, _) = apply(&[add(1, None, None), add(2, None, Some(0)), add(3, None, Some(9))], &meta, &facts(), 0);
        assert_eq!(m.credits, [c(2, "featured"), c(1, "primary"), c(3, "featured")]);
        // Moving and renaming one that's there.
        let alias = Op::Credit { artist: id(1), role: None, alias: Some(Some(id(70))), at: Some(1) };
        let (m, _) = apply(&[add(1, None, None), add(2, None, None), alias], &meta, &facts(), 0);
        assert_eq!(m.credits[1], Credit { artist_id: id(1), role: "primary".into(), alias_id: Some(id(70)) });
        // Off: one role, or every role.
        let ops = [add(1, None, None), add(1, Some("producer"), None), Op::Uncredit { artist: id(1), role: Some("producer".into()) }];
        assert_eq!(apply(&ops, &meta, &facts(), 0).0.credits, [c(1, "primary")]);
        let ops = [add(1, None, None), add(1, Some("producer"), None), Op::Uncredit { artist: id(1), role: None }];
        assert!(apply(&ops, &meta, &facts(), 0).0.credits.is_empty());
        // Replacing all: clear, then add.
        let start = Meta { credits: vec![c(5, "primary")], ..meta.clone() };
        assert_eq!(apply(&[Op::NoCredits, add(1, None, None)], &start, &facts(), 0).0.credits, [c(1, "primary")]);
    }

    #[test]
    fn albums_numbers_and_order() {
        let meta = Meta::default();
        let ops = [Op::Album { album: id(8), numbers: Numbers::InOrder(1) }];
        let (_, f) = apply(&ops, &meta, &facts(), 2);
        assert_eq!(f.albums, [Membership { album_id: id(8), disc_number: Some(1), track_number: Some(3) }]);
        // On it already: only the numbers given change.
        let on = Facts { albums: vec![Membership { album_id: id(8), disc_number: Some(2), track_number: Some(4) }], ..facts() };
        let ops = [Op::Album { album: id(8), numbers: Numbers::At { disc: None, number: Some(7) } }];
        assert_eq!(apply(&ops, &meta, &on, 0).1.albums[0], Membership { album_id: id(8), disc_number: Some(2), track_number: Some(7) });
        // Only on another one: clear, then add.
        let ops = [Op::NoAlbums, Op::Album { album: id(9), numbers: Numbers::Keep }];
        assert_eq!(apply(&ops, &meta, &on, 0).1.albums, [Membership { album_id: id(9), disc_number: None, track_number: None }]);
    }

    #[test]
    fn fields_genres_relations_and_settings() {
        let meta = Meta { title: "Old".into(), genres: vec!["Rap".into()], release_date: Some("2019".into()), ..Default::default() };
        let ops = [
            Op::Title("  New ".into()),
            Op::Date(Some(" ".into())),
            Op::Genre("rap".into()),
            Op::Genre("trap".into()),
            Op::NoGenre("RAP".into()),
            Op::Relate { track: id(4), kind: "remix_of".into() },
            Op::Relate { track: id(4), kind: "remix_of".into() },
            Op::Along(false),
            Op::Why("a".into()),
            Op::Why(" b ".into()),
        ];
        let (m, f) = apply(&ops, &meta, &facts(), 0);
        assert_eq!((m.title.as_str(), m.release_date.as_deref(), m.genres.clone()), ("New", None, vec!["trap".to_string()]));
        assert_eq!(f.relations.len(), 1);
        assert!(!along(&ops));
        assert_eq!(why(&ops), "b");
        assert!(along(&[]));
        // Made to match: along, until turned off.
        assert!(sync(&[Op::Sync]) && along(&[Op::Along(false), Op::Sync]));
        assert!(!sync(&[Op::Sync, Op::Along(false)]) && !sync(&[]));
        assert!(sync(&[Op::Sync, Op::Along(true)]), "on doesn't undo making them match");
    }
}
