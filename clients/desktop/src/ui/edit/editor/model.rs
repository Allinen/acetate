//! The editor's model: a track's metadata (its shown version's) and facts as the server
//! keeps them, and the changes typed in the editor (operations), applied to them.
//!
//! An edit is a list of operations, not a form's fields: the same list goes on each
//! target track's own current state, which is how one edit changes one track or a
//! thousand, and how a track someone changed meanwhile gets the edit again on their
//! version. `apply` (metadata and facts) and `attach` (covers and sources, which aren't
//! revisions) are the only places an operation means something.

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

/// A track's covers and sources, and which source its file came from: not revisions, but
/// changed with the rest of it (the server's `CoversEdit`, `SourcesEdit`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attached {
    /// In order: the first is the main one.
    pub covers: Vec<Pic>,
    pub sources: Vec<Src>,
    /// The source its file came from.
    pub origin: Option<Uuid>,
    /// The groupings it's in (from the library: its details don't say).
    pub groups: Vec<Uuid>,
}

/// A cover, and what it'll be made from.
#[derive(Debug, Clone, PartialEq)]
pub struct Pic {
    pub image: Image,
    /// Its image's SHA-256 (to draw it, and to know the same image again).
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    /// A cover the track has (its id).
    Own(Uuid),
    /// A copy of another track's or album's cover (its id).
    CopyOf(Uuid),
    /// An image uploaded for this edit (`POST /api/covers/staged`): its hash says which.
    Staged,
}

/// A source.
#[derive(Debug, Clone, PartialEq)]
pub struct Src {
    /// Its id; a new one's stand-in (the server says which source it became).
    pub id: Uuid,
    pub new: bool,
    pub platform: String,
    /// What tells it apart: its id on the platform, or a custom one's label.
    pub name: String,
    /// Its newest data (corrected: as corrected).
    pub data: serde_json::Value,
    pub corrected: bool,
}

impl Src {
    /// "netease 123", "custom “vk”".
    pub fn label(&self) -> String {
        match (self.platform.as_str(), self.name.as_str()) {
            (p, "") => p.to_string(),
            ("custom", name) => format!("custom “{name}”"),
            (p, name) => format!("{p} {name}"),
        }
    }
}

/// A track as the editor starts from it: its shown version (id, latest revision,
/// metadata), its facts (latest revision), and its covers and sources.
#[derive(Debug, Clone)]
pub struct Base {
    pub variant: Uuid,
    pub variant_base: Uuid,
    pub meta: Meta,
    pub facts_base: Option<Uuid>,
    pub facts: Facts,
    pub attached: Attached,
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
        let attached = Attached {
            covers: d.covers.iter().map(|c| Pic { image: Image::Own(c.id), sha: c.blob_sha256.clone() }).collect(),
            sources: d
                .sources
                .iter()
                .map(|s| {
                    let data = s.snapshots.first().map(|x| x.data.clone()).unwrap_or_default();
                    let name = s.external_id.clone().or_else(|| data.get("label").and_then(|l| l.as_str()).map(str::to_string)).unwrap_or_default();
                    Src { id: s.id, new: false, platform: s.platform.clone(), name, data, corrected: false }
                })
                .collect(),
            origin: d.file.origin.as_ref().map(|o| o.source_id),
            // (The library's: the engine fills them in.)
            groups: Vec::new(),
        };
        Some(Base { variant: v.id, variant_base: v.revision_id, meta, facts_base: d.external_revision_id, facts, attached })
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
    /// Into a grouping (or a new one's stand-in): not a revision.
    Group(Uuid),
    /// Out of a grouping.
    Ungroup(Uuid),
    /// A single of its own, made after the rest is written (from the track as it is then).
    Single,
    /// Whether albums it's alone on change along (the last one says).
    Along(bool),
    /// Albums it's alone on are made to match it, changed or not (title, date, performers):
    /// `along:on` typed (left out, they follow only what changes).
    Sync,
    /// Why (the last one says): kept with every revision.
    Why(String),
    /// A cover: a new image, or a copy of another cover. `at`: its place (0: the main one;
    /// None: last). A track with that image already: it's moved there (if `at` says).
    Cover { image: Image, sha: String, at: Option<usize> },
    /// A cover moved (places from 0).
    MoveCover { from: usize, to: usize },
    /// A cover to the trash (its place).
    NoCover(usize),
    NoCovers,
    /// A source added: `id` its stand-in, `name` what tells it apart (see `Src`). One with
    /// the same platform id there already gets the data, as a correction (the server
    /// makes it a snapshot of that one).
    Source { id: Uuid, platform: String, name: String, data: serde_json::Value },
    /// A new snapshot of a source it has.
    CorrectSource { id: Uuid, data: serde_json::Value },
    /// A source to the trash (its place among them).
    NoSource(usize),
    NoSources,
    /// The file came from the source in this place.
    Origin(usize),
    /// The metadata changes go to a new version named this, starting from nothing (only
    /// what they say; e.g. what was imported from SoundCloud). On a draft: the version it
    /// has, renamed (the last one says).
    Version(String),
    /// An account on a platform given to an artist (or a new one's stand-in) as its source:
    /// `label` says it ("soundcloud someone"), `data` is in the platform's artist shape. Not
    /// a revision; sent once, with the edit.
    Account { artist: Uuid, platform: String, label: String, data: serde_json::Value },
    /// Flagged by hand: `kind` ("fix_later", one people made), or (`new`) a new kind of
    /// flag named `name`, made with the edit. Not a revision.
    Flag { kind: String, name: String, new: bool },
    /// Its open flags of `kind` settled.
    Unflag { kind: String, name: String },
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
            Op::Group(_) | Op::Ungroup(_) | Op::Single | Op::Along(_) | Op::Sync | Op::Why(_) | Op::Version(_) | Op::Account { .. } | Op::Flag { .. } | Op::Unflag { .. } => {}
            Op::Cover { .. } | Op::MoveCover { .. } | Op::NoCover(_) | Op::NoCovers => {}
            Op::Source { .. } | Op::CorrectSource { .. } | Op::NoSource(_) | Op::NoSources | Op::Origin(_) => {}
        }
    }
    (meta, facts)
}

/// `ops` on a track's covers and sources.
pub fn attach(ops: &[Op], attached: &Attached) -> Attached {
    let mut a = attached.clone();
    let moved = |list: &mut Vec<Pic>, from: usize, to: usize| {
        if from < list.len() {
            let pic = list.remove(from);
            list.insert(to.min(list.len()), pic);
        }
    };
    for op in ops {
        match op {
            Op::Cover { image, sha, at } => match a.covers.iter().position(|p| p.sha == *sha) {
                Some(there) => {
                    if let Some(at) = at {
                        moved(&mut a.covers, there, *at);
                    }
                }
                None => {
                    let pic = Pic { image: image.clone(), sha: sha.clone() };
                    let at = at.unwrap_or(a.covers.len()).min(a.covers.len());
                    a.covers.insert(at, pic);
                }
            },
            Op::MoveCover { from, to } => moved(&mut a.covers, *from, *to),
            Op::NoCover(at) => {
                if *at < a.covers.len() {
                    a.covers.remove(*at);
                }
            }
            Op::NoCovers => a.covers.clear(),
            Op::Source { id, platform, name, data } => {
                // The same source: the same platform id, or (custom ones, without an id)
                // the same data, as the server matches them.
                let same = |s: &Src| s.platform == *platform && if platform == "custom" { s.data == *data } else { !name.is_empty() && s.name == *name };
                if let Some(s) = a.sources.iter_mut().find(|s| same(s)) {
                    s.data = data.clone();
                    s.corrected = !s.new;
                } else if let Some(had) = attached.sources.iter().find(|s| same(s)) {
                    // One it had, taken off before (replacing them all): it stays.
                    a.sources.push(Src { data: data.clone(), corrected: had.data != *data, ..had.clone() });
                } else {
                    a.sources.push(Src { id: *id, new: true, platform: platform.clone(), name: name.clone(), data: data.clone(), corrected: false });
                }
            }
            Op::CorrectSource { id, data } => {
                if let Some(s) = a.sources.iter_mut().find(|s| s.id == *id) {
                    s.data = data.clone();
                    s.corrected = !s.new;
                    if s.platform == "custom" {
                        s.name = data.get("label").and_then(|l| l.as_str()).unwrap_or_default().to_string();
                    }
                }
            }
            Op::NoSource(at) => {
                if *at < a.sources.len() {
                    a.sources.remove(*at);
                }
            }
            Op::NoSources => a.sources.clear(),
            Op::Origin(at) => {
                if let Some(s) = a.sources.get(*at) {
                    a.origin = Some(s.id);
                }
            }
            Op::Group(g) => {
                if !a.groups.contains(g) {
                    a.groups.push(*g);
                }
            }
            Op::Ungroup(g) => a.groups.retain(|x| x != g),
            _ => {}
        }
    }
    a
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

/// Whether each track gets a single of its own.
pub fn single(ops: &[Op]) -> bool {
    ops.contains(&Op::Single)
}

/// The new version the metadata goes to (`Op::Version`, the last one).
pub fn version(ops: &[Op]) -> Option<String> {
    ops.iter().rev().find_map(|op| if let Op::Version(label) = op { Some(label.clone()) } else { None })
}

/// The flags raised and settled by hand: (kinds or new kinds' names raised, kinds settled),
/// each once (the last one for a kind says which).
pub fn flags(ops: &[Op]) -> (Vec<String>, Vec<String>) {
    let (mut add, mut settle): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for op in ops {
        match op {
            Op::Flag { kind, .. } => {
                settle.retain(|k| k != kind);
                if !add.contains(kind) {
                    add.push(kind.clone());
                }
            }
            Op::Unflag { kind, .. } => {
                add.retain(|k| k != kind);
                if !settle.contains(kind) {
                    settle.push(kind.clone());
                }
            }
            _ => {}
        }
    }
    (add, settle)
}

/// The new kinds of flag the changes make (their names).
pub fn new_flag_kinds(ops: &[Op]) -> Vec<String> {
    let (add, _) = flags(ops);
    ops.iter().filter_map(|op| if let Op::Flag { kind, new: true, .. } = op { Some(kind.clone()) } else { None }).filter(|k| add.contains(k)).fold(Vec::new(), |mut out, k| {
        if !out.contains(&k) {
            out.push(k);
        }
        out
    })
}

/// The accounts given to artists: (artist, platform, data), each once.
pub fn accounts(ops: &[Op]) -> Vec<(Uuid, String, serde_json::Value)> {
    let mut out: Vec<(Uuid, String, serde_json::Value)> = Vec::new();
    for op in ops {
        if let Op::Account { artist, platform, data, .. } = op
            && !out.iter().any(|(a, p, d)| a == artist && p == platform && d == data)
        {
            out.push((*artist, platform.clone(), data.clone()));
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

/// Whether the albums are made to match (an `along:on` not turned off after it).
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

    fn pic(n: u128) -> Pic {
        Pic { image: Image::Own(id(n)), sha: format!("{n:064x}") }
    }

    #[test]
    fn covers_added_moved_and_trashed() {
        let base = Attached { covers: vec![pic(1), pic(2)], ..Default::default() };
        let staged = |n: u128, at| Op::Cover { image: Image::Staged, sha: format!("{n:064x}"), at };
        // A new one as the main one; another last; one it has already: only moved.
        let a = attach(&[staged(3, Some(0)), staged(4, None), staged(2, Some(0))], &base);
        let shas: Vec<String> = a.covers.iter().map(|p| p.sha.clone()).collect();
        assert_eq!(shas, [2, 3, 1, 4].map(|n: u128| format!("{n:064x}")));
        assert_eq!(a.covers[1].image, Image::Staged);
        // Moved, then the second to the trash; past the end: nothing.
        let a = attach(&[Op::MoveCover { from: 1, to: 0 }, Op::NoCover(1), Op::NoCover(5)], &base);
        assert_eq!(a.covers, [pic(2)]);
        assert!(attach(&[Op::NoCovers], &base).covers.is_empty());
    }

    #[test]
    fn groupings_joined_and_left() {
        let base = Attached { groups: vec![id(1)], ..Default::default() };
        let a = attach(&[Op::Group(id(2)), Op::Group(id(1)), Op::Ungroup(id(1)), Op::Group(id(2))], &base);
        assert_eq!(a.groups, [id(2)]);
        assert!(single(&[Op::Title("x".into()), Op::Single]) && !single(&[]));
    }

    #[test]
    fn sources_added_corrected_trashed_and_the_origin() {
        let src = |n: u128, platform: &str, name: &str| Src { id: id(n), new: false, platform: platform.into(), name: name.into(), data: serde_json::json!({}), corrected: false };
        let base = Attached { sources: vec![src(1, "custom", "vk"), src(2, "netease", "123")], origin: Some(id(1)), ..Default::default() };
        let add = |n: u128, platform: &str, name: &str| Op::Source { id: id(n), platform: platform.into(), name: name.into(), data: serde_json::json!({ "n": n as u64 }) };
        // A new one, made the origin; the same NetEase song: a correction of the one there.
        let a = attach(&[add(9, "youtube", "abc"), Op::Origin(2), add(8, "netease", "123")], &base);
        assert_eq!(a.sources.len(), 3);
        assert!(a.sources[2].new && a.origin == Some(id(9)));
        assert!(a.sources[1].corrected && a.sources[1].data["n"] == 8);
        // Corrected by hand; the first to the trash (the origin stays what it was).
        let a = attach(&[Op::CorrectSource { id: id(1), data: serde_json::json!({ "label": "discord" }) }, Op::NoSource(0)], &base);
        assert_eq!((a.sources.len(), a.origin), (1, Some(id(1))));
        let a = attach(&[Op::CorrectSource { id: id(1), data: serde_json::json!({ "label": "discord" }) }], &base);
        assert_eq!((a.sources[0].label(), a.sources[0].corrected), ("custom “discord”".to_string(), true));
        // All of them replaced by one it has: that one stays (corrected, its data being
        // new), the file from it; the other goes.
        let a = attach(&[Op::NoSources, add(8, "netease", "123"), Op::Origin(0)], &base);
        assert_eq!(a.sources.iter().map(|s| (s.id, s.new, s.corrected)).collect::<Vec<_>>(), [(id(2), false, true)]);
        assert_eq!(a.origin, Some(id(2)));
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
    }
}
