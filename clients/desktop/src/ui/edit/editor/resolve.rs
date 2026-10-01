//! What the editor's line means: its parts (`parse`) turned into changes (`model::Op`),
//! each saying what it does, or why it can't; the choices listed under the line for the
//! part being typed; and, for a line of plain words, what can be changed by that name.
//!
//! Artists, albums and tracks are found in the library (the best match, as listed), or
//! taken from what was chosen in the list (`Names::pin`: the same text is then always that
//! one, even with others named alike), or new ones chosen there ("New artist “…”"), made
//! when the edit is applied.

use std::collections::HashMap;

use uuid::Uuid;

use super::model::{Numbers, Op};
use super::parse::{self, Part, K};
use crate::account::Vocab;
use crate::catalog::query::loosely_in;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::details::AliasInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    Artist,
    Album,
    Track,
}

/// What the line's names stand for: choices made in the list, and new artists and albums.
#[derive(Debug, Default)]
pub struct Names {
    pins: Vec<(PinKind, String, Uuid)>,
    /// Stand-in id → name.
    pub new_artists: Vec<(Uuid, String)>,
    /// Stand-in id → (title, kind).
    pub new_albums: Vec<(Uuid, String, String)>,
    next: u128,
    /// Changes whenever something here does.
    pub version: u64,
}

impl Names {
    /// `text` (as put in the line) is `id` from now on.
    pub fn pin(&mut self, kind: PinKind, text: &str, id: Uuid) {
        self.pins.push((kind, text.trim().to_lowercase(), id));
        self.version += 1;
    }

    fn pinned(&self, kind: PinKind, text: &str) -> Option<Uuid> {
        let text = text.trim().to_lowercase();
        self.pins.iter().rev().find(|(k, t, _)| *k == kind && *t == text).map(|p| p.2)
    }

    /// A new artist called `name` (the same one for the same name).
    pub fn new_artist(&mut self, name: &str) -> Uuid {
        let name = name.trim();
        if let Some((id, _)) = self.new_artists.iter().find(|(_, n)| n == name) {
            return *id;
        }
        let id = self.stand_in();
        self.new_artists.push((id, name.to_string()));
        id
    }

    /// A new album called `title` (the same one for the same title).
    pub fn new_album(&mut self, title: &str) -> Uuid {
        let title = title.trim();
        if let Some((id, _, _)) = self.new_albums.iter().find(|(_, t, _)| t == title) {
            return *id;
        }
        let id = self.stand_in();
        self.new_albums.push((id, title.to_string(), "album".into()));
        id
    }

    pub fn set_album_kind(&mut self, album: Uuid, kind: &str) {
        if let Some(a) = self.new_albums.iter_mut().find(|a| a.0 == album)
            && a.2 != kind
        {
            a.2 = kind.to_string();
            self.version += 1;
        }
    }

    pub fn is_new(&self, id: Uuid) -> bool {
        self.new_artists.iter().any(|a| a.0 == id) || self.new_albums.iter().any(|a| a.0 == id)
    }

    /// Names for the new ones' stand-ins (for the history's words).
    pub fn stand_ins(&self) -> HashMap<Uuid, String> {
        self.new_artists.iter().map(|(id, n)| (*id, n.clone())).chain(self.new_albums.iter().map(|(id, t, _)| (*id, t.clone()))).collect()
    }

    fn stand_in(&mut self) -> Uuid {
        self.next += 1;
        self.version += 1;
        // Not an id the server makes (those are time-ordered, v7).
        Uuid::from_u128(0xACE7_0000_0000_4000_8000_0000_0000_0000 | self.next)
    }
}

/// What the resolving needs to know.
pub struct Ctx<'a> {
    pub catalog: &'a Catalog,
    pub vocab: &'a Vocab,
    /// How many tracks the edit is for.
    pub targets: usize,
    /// The album the list is of (an album's page): numbers go on it.
    pub context_album: Option<Uuid>,
    /// The album staged last, else: numbers go on it.
    pub staged_album: Option<Uuid>,
    pub names: &'a Names,
    pub may_relate: bool,
}

/// One thing the line does: the parts it was typed as, and what it means.
#[derive(Debug, Clone)]
pub struct Unit {
    pub text: String,
    pub result: Result<Resolved, String>,
}

#[derive(Debug, Clone, Default)]
pub struct Resolved {
    /// What it does, in words.
    pub label: String,
    pub ops: Vec<Op>,
    /// Allowed, but worth a look (a title for many tracks…).
    pub warning: Option<String>,
    /// A new album's kind, set when it's staged.
    pub album_kind: Option<(Uuid, String)>,
}

/// The line's parts as units (a part with the `role:`, `as:`, `kind:` after it), each
/// resolved. `aliases`: an artist's aliases (None while they load). Also says whether
/// something was still loading (then it's worth asking again).
pub fn resolve(line: &str, ctx: &Ctx, aliases: &mut dyn FnMut(Uuid) -> Option<Vec<AliasInfo>>) -> (Vec<Unit>, bool) {
    let parsed = parse::parse(line);
    let mut units = Vec::new();
    let mut waiting = false;
    let mut i = 0;
    while i < parsed.parts.len() {
        let main = &parsed.parts[i];
        let mut j = i + 1;
        while j < parsed.parts.len() && matches!(parsed.parts[j].key, K::Role | K::As | K::Kind) {
            j += 1;
        }
        let mods = &parsed.parts[i + 1..j];
        let start = line[..main.value_start].rfind(|c: char| c.is_whitespace()).map_or(0, |w| w + 1);
        let end = parsed.parts[j - 1].end;
        let text = line[start..end].trim().to_string();
        // A line album for `disc:` / `track:` after it.
        let line_album = parsed.parts[..i].iter().rev().find(|p| p.key == K::Album && !p.minus).and_then(|p| album(&p.value, ctx).ok()).map(|a| a.0);
        let result = unit(main, mods, ctx, line_album, aliases, &mut waiting);
        units.push(Unit { text, result });
        i = j;
    }
    (units, waiting)
}

fn unit(
    main: &Part,
    mods: &[Part],
    ctx: &Ctx,
    line_album: Option<Uuid>,
    aliases: &mut dyn FnMut(Uuid) -> Option<Vec<AliasInfo>>,
    waiting: &mut bool,
) -> Result<Resolved, String> {
    let value = main.value.trim();
    let none = |v: &str| v.is_empty() || v.eq_ignore_ascii_case("none");
    let many = ctx.targets > 1;
    let done = |label: String, ops: Vec<Op>| Ok(Resolved { label, ops, ..Default::default() });
    // `role:`, `as:` and `kind:` belong after something; `#` only after some.
    let modifier = |k: K| mods.iter().find(|p| p.key == k);
    let hash = mods.iter().rev().chain(std::iter::once(main)).find_map(|p| p.hash.clone());
    let only = |allowed: &[K]| -> Result<(), String> {
        match mods.iter().find(|p| !allowed.contains(&p.key)) {
            Some(p) => Err(format!("{}: doesn't go after {}:", p.key.name(), main.key.name())),
            None => Ok(()),
        }
    };
    match main.key {
        K::Title => {
            only(&[])?;
            if main.minus || value.is_empty() {
                return Err("title: type it after the colon (a track needs one)".into());
            }
            let warning = many.then(|| format!("every one of the {} tracks gets this title", ctx.targets));
            Ok(Resolved { label: format!("title → “{value}”"), ops: vec![Op::Title(value.into())], warning, ..Default::default() })
        }
        K::Date => {
            only(&[])?;
            if main.minus || none(value) {
                done("release date → none".into(), vec![Op::Date(None)])
            } else {
                done(format!("release date → {value}"), vec![Op::Date(Some(value.into()))])
            }
        }
        K::Comment => {
            only(&[])?;
            if main.minus || none(value) {
                done("comment → none".into(), vec![Op::Comment(None)])
            } else {
                done(format!("comment → “{value}”"), vec![Op::Comment(Some(value.into()))])
            }
        }
        K::Status => {
            only(&[])?;
            match find_in(&ctx.vocab.statuses, value) {
                Some(s) => done(format!("status → {s}"), vec![Op::Status(s)]),
                None if value.is_empty() => Err("status: choose one".into()),
                None => Err(format!("no status like “{value}”")),
            }
        }
        K::Genre => {
            only(&[])?;
            match (main.minus, main.colon) {
                (true, false) => done("genres → none".into(), vec![Op::NoGenres]),
                (true, true) if value.is_empty() => Err("-genre: which one (or -genre alone: all of them)".into()),
                (true, true) => done(format!("genres: − {value}"), vec![Op::NoGenre(value.into())]),
                _ if none(value) => done("genres → none".into(), vec![Op::NoGenres]),
                _ => {
                    let genres: Vec<&str> = value.split(',').map(str::trim).filter(|g| !g.is_empty()).collect();
                    done(format!("genres: + {}", genres.join(", ")), genres.iter().map(|g| Op::Genre(g.to_string())).collect())
                }
            }
        }
        K::Artist | K::Featured => {
            only(&[K::Role, K::As])?;
            if main.minus && !main.colon {
                if main.key == K::Featured {
                    return Err("-feat alone isn't anything: -artist takes every artist off".into());
                }
                return done("artists → none".into(), vec![Op::NoCredits]);
            }
            if value.is_empty() {
                return Err(format!("{}: type a name", main.key.name()));
            }
            let (id, name) = artist(value, ctx)?;
            let role = match (modifier(K::Role), main.key) {
                (Some(p), _) => Some(find_in(&ctx.vocab.credit_roles, &p.value).ok_or_else(|| format!("no role like “{}”", p.value))?),
                (None, K::Featured) => Some("featured".to_string()),
                _ => None,
            };
            if main.minus {
                if modifier(K::As).is_some() || hash.is_some() {
                    return Err("taking an artist off needs no as: or #".into());
                }
                let label = match &role {
                    Some(r) => format!("artists: − {name} ({r})"),
                    None => format!("artists: − {name}"),
                };
                return done(label, vec![Op::Uncredit { artist: id, role }]);
            }
            let alias = match modifier(K::As) {
                None => None,
                Some(p) if none(&p.value) || p.value.trim().eq_ignore_ascii_case(&name) || p.value.trim().eq_ignore_ascii_case("own") => Some(None),
                Some(p) => {
                    if ctx.names.is_new(id) {
                        return Err(format!("a new artist has no other names yet (as: {})", p.value));
                    }
                    match aliases(id) {
                        None => {
                            *waiting = true;
                            return Err(format!("looking up {name}'s names…"));
                        }
                        Some(list) => {
                            let names: Vec<String> = list.iter().map(|a| a.name.clone()).collect();
                            let found = find_in(&names, &p.value).ok_or_else(|| format!("{name} has no name like “{}”", p.value))?;
                            Some(list.iter().find(|a| a.name == found).map(|a| a.id))
                        }
                    }
                }
            };
            let at = match &hash {
                Some(h) if h.is_empty() => None,
                Some(h) => match h.parse::<usize>() {
                    Ok(n) if n >= 1 => Some(n - 1),
                    _ => return Err(format!("#{h}: an artist's place is a number from 1")),
                },
                None => None,
            };
            let mut label = format!("artists: + {name}");
            if ctx.names.is_new(id) {
                label.push_str(" (new)");
            }
            match &role {
                Some(r) => label.push_str(&format!(" as {r}")),
                None => label.push_str(" (primary if it has none, else featured)"),
            }
            if let Some(Some(alias)) = alias {
                let shown = aliases(id).and_then(|l| l.into_iter().find(|a| a.id == alias)).map(|a| a.name).unwrap_or_default();
                label.push_str(&format!(", credited as “{shown}”"));
            }
            if let Some(at) = at {
                label.push_str(&format!(", #{}", at + 1));
            }
            done(label, vec![Op::Credit { artist: id, role, alias, at }])
        }
        K::Album => {
            only(&[K::Kind])?;
            if main.minus && !main.colon {
                return done("albums → none".into(), vec![Op::NoAlbums]);
            }
            if value.is_empty() {
                return Err("album: type its title".into());
            }
            let (id, title) = album(value, ctx)?;
            if main.minus {
                return done(format!("albums: − “{title}”"), vec![Op::NoAlbum(id)]);
            }
            let new = ctx.names.is_new(id);
            let kind = match modifier(K::Kind) {
                Some(_) if !new => return Err("kind: only for a new album (an album's own kind is changed on its page)".into()),
                Some(p) => Some(find_in(&ctx.vocab.album_kinds, &p.value).ok_or_else(|| format!("no album kind like “{}”", p.value))?),
                None => None,
            };
            let numbers = match hash.as_deref().map(parse::numbers) {
                None => Numbers::Keep,
                Some(None) => return Err("#: a track number (#5), disc and track (#2-5), or in order from (#1+)".into()),
                Some(Some((_, Some(from), true))) => Numbers::InOrder(from),
                Some(Some((disc, number, _))) => {
                    if many && number.is_some() {
                        return Err(format!("#{}: one number for {} tracks; #{}+ numbers them in order", hash.unwrap_or_default(), ctx.targets, number.unwrap_or(1)));
                    }
                    Numbers::At { disc, number }
                }
            };
            let mut label = if new {
                format!("albums: + “{title}” (new {})", kind.clone().unwrap_or_else(|| new_kind(ctx, id)))
            } else {
                format!("albums: + “{title}”")
            };
            match numbers {
                Numbers::InOrder(from) => label.push_str(&format!(", numbered in order from {from}")),
                Numbers::At { disc: Some(d), number: Some(n) } => label.push_str(&format!(", disc {d} track {n}")),
                Numbers::At { disc: None, number: Some(n) } => label.push_str(&format!(", track {n}")),
                _ => {}
            }
            Ok(Resolved { label, ops: vec![Op::Album { album: id, numbers }], warning: None, album_kind: kind.map(|k| (id, k)) })
        }
        K::Disc | K::Number => {
            only(&[])?;
            let Some(on) = line_album.or(ctx.context_album).or(ctx.staged_album) else {
                return Err(format!("{}: on which album? (album:… first, or on an album's page)", main.key.name()));
            };
            let title = album_title(ctx, on);
            let (digits, in_order) = match value.strip_suffix('+') {
                Some(d) => (d, true),
                None => (value, false),
            };
            let n: i16 = digits.trim().parse().ok().filter(|n| *n > 0).ok_or_else(|| format!("{}: “{value}” isn't a number", main.key.name()))?;
            match (main.key, in_order) {
                (K::Disc, false) => done(format!("disc on “{title}” → {n}"), vec![Op::Album { album: on, numbers: Numbers::At { disc: Some(n), number: None } }]),
                (K::Disc, true) => Err("disc: one number (track:N+ numbers in order)".into()),
                (_, true) => done(format!("track numbers on “{title}”: in order from {n}"), vec![Op::Album { album: on, numbers: Numbers::InOrder(n) }]),
                (_, false) => {
                    let warning = many.then(|| format!("every one of the {} tracks gets number {n} (track:{n}+ numbers them in order)", ctx.targets));
                    Ok(Resolved {
                        label: format!("track number on “{title}” → {n}"),
                        ops: vec![Op::Album { album: on, numbers: Numbers::At { disc: None, number: Some(n) } }],
                        warning,
                        ..Default::default()
                    })
                }
            }
        }
        K::Related => {
            only(&[K::Kind])?;
            if !ctx.may_relate {
                return Err("related tracks need the edit_relations permission".into());
            }
            if main.minus && !main.colon {
                return done("related → none".into(), vec![Op::NoRelations]);
            }
            if value.is_empty() {
                return Err("related: type the other track's title".into());
            }
            let (id, title) = track(value, ctx)?;
            if main.minus {
                return done(format!("related: − “{title}”"), vec![Op::Unrelate(id)]);
            }
            let kind = match modifier(K::Kind) {
                Some(p) => relation_kind(ctx.vocab, &p.value).ok_or_else(|| format!("no relation like “{}”", p.value))?,
                None => ctx.vocab.relation_kinds.first().map(|k| k.kind.clone()).ok_or("no relation kinds")?,
            };
            let warning = many.then(|| format!("every one of the {} tracks gets this relation; usually it's one track's", ctx.targets));
            Ok(Resolved { label: format!("related: {}", relation_sentence(ctx.vocab, &kind, &title)), ops: vec![Op::Relate { track: id, kind }], warning, ..Default::default() })
        }
        K::Group => {
            only(&[])?;
            if main.minus {
                return Err("taking tracks out of a grouping is on the grouping's page".into());
            }
            let lower = value.to_lowercase();
            let found = ctx
                .catalog
                .groupings
                .iter()
                .find(|g| g.name.to_lowercase() == lower)
                .or_else(|| ctx.catalog.groupings.iter().find(|g| !value.is_empty() && loosely_in(value, &g.name)));
            match found {
                Some(g) => done(format!("grouping: + “{}” ({})", g.name, g.kind), vec![Op::Group(g.id)]),
                None if value.is_empty() => Err("group: type its name".into()),
                None => Err(format!("no grouping like “{value}”")),
            }
        }
        K::Why => {
            only(&[])?;
            done(format!("why: “{value}”"), vec![Op::Why(value.into())])
        }
        K::Along => {
            only(&[])?;
            match value.to_lowercase().as_str() {
                "on" | "yes" | "" => done("albums it's alone on: change along".into(), vec![Op::Along(true)]),
                "off" | "no" => done("albums it's alone on: left as they are".into(), vec![Op::Along(false)]),
                _ => Err("along: on or off".into()),
            }
        }
        K::Role | K::As | K::Kind => Err(format!("{}: goes after what it's about (artist:… role:…)", main.key.name())),
        K::Other(k) => Err(format!("{}: not something to change here", k.name())),
    }
}

/// The new album's kind as it is now.
fn new_kind(ctx: &Ctx, id: Uuid) -> String {
    ctx.names.new_albums.iter().find(|a| a.0 == id).map(|a| a.2.clone()).unwrap_or_else(|| "album".into())
}

/// An artist by name: chosen, new, or the library's best match.
fn artist(value: &str, ctx: &Ctx) -> Result<(Uuid, String), String> {
    if let Some(id) = ctx.names.pinned(PinKind::Artist, value) {
        let name = ctx.catalog.artist(id).map(|a| a.name.clone()).or_else(|| ctx.names.new_artists.iter().find(|a| a.0 == id).map(|a| a.1.clone()));
        return Ok((id, name.unwrap_or_else(|| value.to_string())));
    }
    if let Some((id, name)) = ctx.names.new_artists.iter().find(|a| a.1.eq_ignore_ascii_case(value.trim())) {
        return Ok((*id, name.clone()));
    }
    let hit = Search::default().run_part(ctx.catalog, value, SearchPart::Artists, 1).first().map(|h| h.0);
    match hit {
        Some(i) => {
            let a = &ctx.catalog.artists[i as usize];
            Ok((a.id, a.label()))
        }
        None => Err(format!("no artist like “{value}” (a new one: choose “New artist” in the list)")),
    }
}

fn album(value: &str, ctx: &Ctx) -> Result<(Uuid, String), String> {
    if let Some(id) = ctx.names.pinned(PinKind::Album, value) {
        return Ok((id, album_title(ctx, id)));
    }
    if let Some((id, title, _)) = ctx.names.new_albums.iter().find(|a| a.1.eq_ignore_ascii_case(value.trim())) {
        return Ok((*id, title.clone()));
    }
    let hit = Search::default().run_part(ctx.catalog, value, SearchPart::Albums, 1).first().map(|h| h.0);
    match hit {
        Some(i) => {
            let a = &ctx.catalog.albums[i as usize];
            Ok((a.id, a.title.clone()))
        }
        None => Err(format!("no album like “{value}” (a new one: choose “New album” in the list)")),
    }
}

fn track(value: &str, ctx: &Ctx) -> Result<(Uuid, String), String> {
    if let Some(id) = ctx.names.pinned(PinKind::Track, value) {
        let title = ctx.catalog.track_index(id).map(|i| ctx.catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| value.to_string());
        return Ok((id, title));
    }
    let hit = Search::default().run_part(ctx.catalog, value, SearchPart::Tracks, 1).first().map(|h| h.0);
    match hit {
        Some(i) => {
            let t = &ctx.catalog.tracks[i as usize];
            Ok((t.id, t.title.clone()))
        }
        None => Err(format!("no track like “{value}”")),
    }
}

fn album_title(ctx: &Ctx, id: Uuid) -> String {
    ctx.catalog
        .album(id)
        .map(|a| a.title.clone())
        .or_else(|| ctx.names.new_albums.iter().find(|a| a.0 == id).map(|a| a.1.clone()))
        .unwrap_or_else(|| "the album".into())
}

/// One of `list` like `typed`: the same (any case), one it starts, then one with its
/// letters in order.
pub fn find_in(list: &[String], typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    let lower = typed.to_lowercase();
    list.iter()
        .find(|s| s.to_lowercase() == lower)
        .or_else(|| list.iter().find(|s| s.to_lowercase().starts_with(&lower)))
        .or_else(|| list.iter().find(|s| loosely_in(typed, s)))
        .cloned()
}

/// A relation kind by its name ("remix_of", "remix") or words of its sentence.
fn relation_kind(vocab: &Vocab, typed: &str) -> Option<String> {
    let names: Vec<String> = vocab.relation_kinds.iter().map(|k| k.kind.clone()).collect();
    find_in(&names, &typed.replace(' ', "_")).or_else(|| vocab.relation_kinds.iter().find(|k| loosely_in(typed, &k.template)).map(|k| k.kind.clone()))
}

/// "This track is a remix of “X”" (or the other way round), for a kind.
pub fn relation_sentence(vocab: &Vocab, kind: &str, other: &str) -> String {
    match vocab.relation_kinds.iter().find(|k| k.kind == kind) {
        Some(k) => crate::ui::edit::capitalized(&k.template.replace("%A", "this track").replace("%B", &format!("“{other}”"))),
        None => format!("{} “{other}”", kind.replace('_', " ")),
    }
}

/// A choice listed under the line, for the part being typed.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub label: String,
    pub sub: String,
    /// What goes in the line for the value.
    pub text: String,
    /// What the text stands for from then on.
    pub pick: Option<Pick>,
    /// The next part to type after it, on Tab (`role:`, `as:`, `kind:`, `#`).
    pub next: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub enum Pick {
    Pin(PinKind, Uuid),
    NewArtist,
    NewAlbum,
}

/// Which library search the part being typed needs, if any.
pub fn search_for(part: &Part) -> Option<SearchPart> {
    match part.key {
        K::Artist | K::Featured => Some(SearchPart::Artists),
        K::Album => Some(SearchPart::Albums),
        K::Related => Some(SearchPart::Tracks),
        _ => None,
    }
}

/// The choices for the last part of the line. `owner`: the part it goes with (`role:`
/// after `artist:`…); `hits`: the library's matches for its value (see `search_for`).
pub fn candidates(
    part: &Part,
    owner: Option<&Part>,
    ctx: &Ctx,
    hits: &[u32],
    aliases: &mut dyn FnMut(Uuid) -> Option<Vec<AliasInfo>>,
) -> Vec<Candidate> {
    let value = part.value.trim();
    let simple = |list: &[String], label: &dyn Fn(&str) -> String| -> Vec<Candidate> {
        list.iter()
            .filter(|s| value.is_empty() || s.to_lowercase().starts_with(&value.to_lowercase()) || loosely_in(value, s))
            .map(|s| Candidate { label: label(s), sub: String::new(), text: s.clone(), pick: None, next: None })
            .collect()
    };
    let mut out = Vec::new();
    match part.key {
        K::Artist | K::Featured => {
            for &i in hits {
                let (name, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Artists, i);
                let a = &ctx.catalog.artists[i as usize];
                out.push(Candidate { label: name.clone(), sub, text: a.label(), pick: Some(Pick::Pin(PinKind::Artist, a.id)), next: (!part.minus).then_some("role:") });
            }
            if !value.is_empty() && !part.minus {
                out.push(Candidate { label: format!("New artist “{value}”"), sub: "made when it's applied".into(), text: value.to_string(), pick: Some(Pick::NewArtist), next: Some("role:") });
            }
        }
        K::Album => {
            for &i in hits {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Albums, i);
                let a = &ctx.catalog.albums[i as usize];
                out.push(Candidate { label: title, sub, text: a.title.clone(), pick: Some(Pick::Pin(PinKind::Album, a.id)), next: (!part.minus).then_some("#") });
            }
            if !value.is_empty() && !part.minus {
                out.push(Candidate { label: format!("New album “{value}”"), sub: "made when it's applied".into(), text: value.to_string(), pick: Some(Pick::NewAlbum), next: Some("kind:") });
            }
        }
        K::Related => {
            for &i in hits {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Tracks, i);
                let t = &ctx.catalog.tracks[i as usize];
                out.push(Candidate { label: title, sub, text: t.title.clone(), pick: Some(Pick::Pin(PinKind::Track, t.id)), next: (!part.minus).then_some("kind:") });
            }
        }
        K::Status => out = simple(&ctx.vocab.statuses, &|s| crate::ui::edit::capitalized(s)),
        K::Role => {
            out = simple(&ctx.vocab.credit_roles, &|s| crate::ui::edit::capitalized(s));
            for c in &mut out {
                c.next = Some("as:");
            }
        }
        K::As => {
            let artist = owner.filter(|o| matches!(o.key, K::Artist | K::Featured)).and_then(|o| artist(&o.value, ctx).ok());
            if let Some((id, name)) = artist {
                let mut names = vec![name.clone()];
                if !ctx.names.is_new(id) {
                    names.extend(aliases(id).unwrap_or_default().into_iter().map(|a| a.name));
                }
                out = simple(&names, &|s| if s == name { format!("{s} (its own name)") } else { s.to_string() });
                for c in &mut out {
                    c.next = Some("#");
                }
            }
        }
        K::Kind => match owner.map(|o| o.key) {
            Some(K::Album) => out = simple(&ctx.vocab.album_kinds, &|s| format!("New {s}")),
            Some(K::Related) => {
                let other = owner.and_then(|o| track(&o.value, ctx).ok()).map(|t| t.1).unwrap_or_else(|| "…".into());
                out = ctx
                    .vocab
                    .relation_kinds
                    .iter()
                    .filter(|k| value.is_empty() || loosely_in(value, &k.kind.replace('_', " ")) || loosely_in(value, &k.template))
                    .map(|k| Candidate { label: relation_sentence(ctx.vocab, &k.kind, &other), sub: String::new(), text: k.kind.clone(), pick: None, next: None })
                    .collect();
            }
            _ => {}
        },
        K::Group => {
            let names: Vec<String> = ctx.catalog.groupings.iter().map(|g| g.name.clone()).collect();
            out = simple(&names, &|s| s.to_string());
            for c in &mut out {
                if let Some(g) = ctx.catalog.groupings.iter().find(|g| g.name == c.text) {
                    c.sub = g.kind.clone();
                }
            }
        }
        K::Along => out = simple(&["on".to_string(), "off".to_string()], &|s| if s == "on" { "on: albums it's alone on change along".into() } else { "off: only the tracks".into() }),
        _ => {}
    }
    out.truncate(10);
    out
}

/// For plain words: the things to change by that name, each with what it puts in the line.
pub fn actions(words: &str, ctx: &Ctx) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = vec![
        ("Title…".into(), "title:".into()),
        ("Artists: add…".into(), "artist:".into()),
        ("Artists: add as featured…".into(), "feat:".into()),
        ("Artists: replace theirs with…".into(), "-artist artist:".into()),
        ("Artists: take one off…".into(), "-artist:".into()),
        ("Album: on it as well…".into(), "album:".into()),
        ("Album: on it only (replacing theirs)…".into(), "-album album:".into()),
        ("Album: take it off one…".into(), "-album:".into()),
        ("Release date…".into(), "date:".into()),
        ("Release date: none".into(), "date:none".into()),
        ("Genres: add…".into(), "genre:".into()),
        ("Genres: none".into(), "-genre".into()),
        ("Comment…".into(), "comment:".into()),
        ("Disc number…".into(), "disc:".into()),
        ("Track numbers in order from…".into(), "track:1+".into()),
        ("Why (kept with the change)…".into(), "why:".into()),
        ("Albums it's alone on: leave them".into(), "along:off".into()),
    ];
    for status in &ctx.vocab.statuses {
        all.push((format!("Status: {status}"), format!("status:{status}")));
    }
    if ctx.may_relate {
        all.push(("Related track…".into(), "related:".into()));
    }
    for g in &ctx.catalog.groupings {
        all.push((format!("Grouping: into “{}” ({})", g.name, g.kind), format!("group:{}", parse::quoted(&g.name))));
    }
    all.into_iter().filter(|(label, _)| loosely_in(words, label)).take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::RelationKind;
    use crate::catalog::model;

    const ANN: &str = "00000000-0000-7000-8000-0000000000a1";
    const ANN_UK: &str = "00000000-0000-7000-8000-0000000000a3";
    const BO: &str = "00000000-0000-7000-8000-0000000000a2";
    const LP: &str = "00000000-0000-7000-8000-0000000000b1";

    fn id(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": 8,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate"],
                       "rows": [["00000000-0000-7000-8000-000000000001", "Other Song", "Bo", [[BO, "primary", null]], [], 1000,
                                 "00000000-0000-7000-8000-0000000000f1", null, null, null, null, "released", [], "", false, "flac"]]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [[LP, "Long Player", "album", "Ann", [], null, null, null, null, "", 1, 1]]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[ANN, "Ann", "", [], 1, 1], [ANN_UK, "Ann", "UK", [], 1, 0], [BO, "Bo", "", [], 1, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"],
                          "rows": [["00000000-0000-7000-8000-0000000000c1", "collective", "The Crew", 0, 0, []]]},
            "playlists": []
        });
        Catalog::build(model::parse(body.to_string().as_bytes()).unwrap())
    }

    fn vocab() -> Vocab {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        Vocab {
            statuses: strings(&["released", "unreleased", "demo"]),
            credit_roles: strings(&["primary", "featured", "producer"]),
            album_kinds: strings(&["album", "ep", "single"]),
            relation_kinds: vec![RelationKind { kind: "remix_of".into(), template: "%A is a remix of %B".into() }],
            ..Default::default()
        }
    }

    /// The line's units (ops, or the error), for `targets` tracks.
    fn run(line: &str, names: &Names, targets: usize) -> Vec<Result<(Vec<Op>, Option<String>), String>> {
        let (catalog, vocab) = (catalog(), vocab());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets, context_album: None, staged_album: None, names, may_relate: true };
        let mut no_aliases = |_: Uuid| Some(Vec::new());
        resolve(line, &ctx, &mut no_aliases).0.into_iter().map(|u| u.result.map(|r| (r.ops, r.warning))).collect()
    }

    fn ops(line: &str) -> Vec<Op> {
        run(line, &Names::default(), 1).into_iter().flat_map(|u| u.unwrap().0).collect()
    }

    #[test]
    fn fields_are_found_by_their_starts() {
        assert_eq!(
            ops("status:dem date:2020 genre:rap, trap"),
            [Op::Status("demo".into()), Op::Date(Some("2020".into())), Op::Genre("rap".into()), Op::Genre("trap".into())]
        );
        assert_eq!(ops("date:none -genre"), [Op::Date(None), Op::NoGenres]);
        assert_eq!(ops("group:crew"), [Op::Group(id("00000000-0000-7000-8000-0000000000c1"))]);
        assert_eq!(ops("along:off why:fixing tags"), [Op::Along(false), Op::Why("fixing tags".into())]);
    }

    #[test]
    fn artists_with_role_place_and_replacing() {
        assert_eq!(ops("artist:Bo role:prod #1"), [Op::Credit { artist: id(BO), role: Some("producer".into()), alias: None, at: Some(0) }]);
        assert_eq!(ops("feat:Bo"), [Op::Credit { artist: id(BO), role: Some("featured".into()), alias: None, at: None }]);
        assert_eq!(ops("-artist artist:Bo"), [Op::NoCredits, Op::Credit { artist: id(BO), role: None, alias: None, at: None }]);
        assert_eq!(ops("-artist:Bo role:producer"), [Op::Uncredit { artist: id(BO), role: Some("producer".into()) }]);
        assert_eq!(ops("artist:Bo as:Bo"), [Op::Credit { artist: id(BO), role: None, alias: Some(None), at: None }]);
    }

    #[test]
    fn chosen_and_new_names_stand_for_what_was_chosen() {
        let mut names = Names::default();
        // Two artists called Ann: the one chosen in the list, every time.
        names.pin(PinKind::Artist, "Ann", id(ANN_UK));
        assert_eq!(run("artist:ann", &names, 1)[0].clone().unwrap().0, [Op::Credit { artist: id(ANN_UK), role: None, alias: None, at: None }]);
        // A new one: its stand-in.
        let zed = names.new_artist("Zed Person");
        assert_eq!(run("artist:Zed Person", &names, 1)[0].clone().unwrap().0, [Op::Credit { artist: zed, role: None, alias: None, at: None }]);
        assert!(run("artist:Zed Person as:Zeddy", &names, 1)[0].is_err(), "a new artist has no aliases");
        // Not in the library and not chosen as new: not guessed.
        assert!(run("artist:qqqqzzzz", &Names::default(), 1)[0].is_err());
    }

    #[test]
    fn albums_and_their_numbers() {
        let lp = id(LP);
        assert_eq!(ops("album:long player #2-5"), [Op::Album { album: lp, numbers: Numbers::At { disc: Some(2), number: Some(5) } }]);
        assert_eq!(ops("-album album:long player track:1+"), [Op::NoAlbums, Op::Album { album: lp, numbers: Numbers::Keep }, Op::Album { album: lp, numbers: Numbers::InOrder(1) }]);
        // Numbers need an album.
        assert!(run("track:3", &Names::default(), 1)[0].is_err());
        // One number for many tracks: refused (#N+ numbers them in order).
        assert!(run("album:long player #3", &Names::default(), 3)[0].is_err());
        assert!(run("album:long player #3+", &Names::default(), 3)[0].is_ok());
        // A new album's kind; an existing one's isn't changed here.
        let mut names = Names::default();
        let ep = names.new_album("Fresh EP");
        let new = run("album:Fresh EP kind:ep", &names, 1);
        assert_eq!(new[0].clone().unwrap().0, [Op::Album { album: ep, numbers: Numbers::Keep }]);
        assert!(run("album:long player kind:ep", &names, 1)[0].is_err());
    }

    #[test]
    fn what_needs_a_look_and_what_cant_be() {
        // A title for many: allowed, with a warning.
        let units = run("title:Same", &Names::default(), 3);
        assert!(units[0].as_ref().unwrap().1.is_some());
        assert!(run("title:Same", &Names::default(), 1)[0].as_ref().unwrap().1.is_none());
        // A relation for many: a warning too.
        assert!(run("related:other song kind:remix", &Names::default(), 2)[0].as_ref().unwrap().1.is_some());
        // Things that aren't anything.
        for line in ["title:", "role:producer", "format:flac", "status:nope", "-group:crew"] {
            assert!(run(line, &Names::default(), 1)[0].is_err(), "{line}");
        }
    }

    #[test]
    fn units_keep_their_text_to_type_back() {
        let (catalog, vocab) = (catalog(), vocab());
        let names = Names::default();
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 1, context_album: None, staged_album: None, names: &names, may_relate: true };
        let (units, _) = resolve("status:demo artist:Bo role:featured #2", &ctx, &mut |_| Some(Vec::new()));
        let texts: Vec<&str> = units.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, ["status:demo", "artist:Bo role:featured #2"]);
    }
}
