//! What the editor's line means: its parts (`parse`) turned into changes (`model::Op`),
//! each saying what it does, or why it can't; the choices listed under the line for the
//! part being typed; and, for a line of plain words, what can be changed by that name.
//!
//! Artists, albums and tracks are found in the library (the best match, as listed), or
//! taken from what was chosen in the list (`Names::pin`: the same text is then always that
//! one, even with others named alike), or new ones chosen there ("New artist “…”"), made
//! when the edit is applied.
//!
//! Some parts ask for something first (`Want`): the image in the clipboard or a file's
//! (uploaded, then staged), or the form correcting a source.

use std::collections::HashMap;

use uuid::Uuid;

use super::model::{Image, Numbers, Op};
use super::parse::{self, Part, K};
use super::title_parse::{Credited, TitlesParsed};
use crate::account::Vocab;
use crate::catalog::query::loosely_in;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::details::AliasInfo;
use crate::title_credits::{Fixes, NameTable, Setting};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    Artist,
    Album,
    Track,
    Grouping,
}

/// What the line's names stand for: choices made in the list, and new artists, albums and
/// groupings.
#[derive(Debug, Default)]
pub struct Names {
    pins: Vec<(PinKind, String, Uuid)>,
    /// Stand-in id → name.
    pub new_artists: Vec<(Uuid, String)>,
    /// Stand-in id → (title, kind).
    pub new_albums: Vec<(Uuid, String, String)>,
    /// Stand-in id → (name, kind).
    pub new_groupings: Vec<(Uuid, String, String)>,
    /// Aliases credited by the staged changes: id → name (for the preview's words).
    pub aliases: Vec<(Uuid, String)>,
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

    /// `text` is no longer the one chosen (it's found as before).
    pub fn unpin(&mut self, kind: PinKind, text: &str) {
        let text = text.trim().to_lowercase();
        self.pins.retain(|(k, t, _)| !(*k == kind && *t == text));
        self.version += 1;
    }

    pub fn pinned(&self, kind: PinKind, text: &str) -> Option<Uuid> {
        self.pinned_any(&[kind], text).map(|p| p.1)
    }

    /// The last of `kinds` chosen for `text`.
    fn pinned_any(&self, kinds: &[PinKind], text: &str) -> Option<(PinKind, Uuid)> {
        let text = text.trim().to_lowercase();
        self.pins.iter().rev().find(|(k, t, _)| kinds.contains(k) && *t == text).map(|p| (p.0, p.2))
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

    /// An alias the staged changes credit, named.
    pub fn alias(&mut self, id: Uuid, name: &str) {
        if !self.aliases.iter().any(|a| a.0 == id) {
            self.aliases.push((id, name.to_string()));
            self.version += 1;
        }
    }

    /// A new grouping called `name` (the same one for the same name); a collective until
    /// its kind is said.
    pub fn new_grouping(&mut self, name: &str) -> Uuid {
        let name = name.trim();
        if let Some((id, _, _)) = self.new_groupings.iter().find(|(_, n, _)| n == name) {
            return *id;
        }
        let id = self.stand_in();
        self.new_groupings.push((id, name.to_string(), "collective".into()));
        id
    }

    /// A new album's or grouping's kind.
    pub fn set_kind(&mut self, id: Uuid, kind: &str) {
        let new = self.new_albums.iter_mut().chain(self.new_groupings.iter_mut()).find(|a| a.0 == id);
        if let Some(a) = new
            && a.2 != kind
        {
            a.2 = kind.to_string();
            self.version += 1;
        }
    }

    pub fn is_new(&self, id: Uuid) -> bool {
        self.new_artists.iter().any(|a| a.0 == id) || self.new_albums.iter().any(|a| a.0 == id) || self.new_groupings.iter().any(|a| a.0 == id)
    }

    /// Names for the new ones' stand-ins (for the history's words).
    pub fn stand_ins(&self) -> HashMap<Uuid, String> {
        self.new_artists
            .iter()
            .map(|(id, n)| (*id, n.clone()))
            .chain(self.new_albums.iter().map(|(id, t, _)| (*id, t.clone())))
            .chain(self.new_groupings.iter().map(|(id, n, _)| (*id, n.clone())))
            .collect()
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
    /// `edit_sources`: sources, and where the file came from.
    pub may_source: bool,
    /// For `parse:`: each target's title as the staged changes leave it (None: still
    /// loading, or not asked for), and every artist's names.
    pub titles: Option<&'a [(Uuid, String)]>,
    pub artist_names: Option<&'a NameTable>,
}

/// What resolving looks up in the library's details (None: still loading).
pub trait Lookups {
    /// An artist's other names.
    fn aliases(&mut self, artist: Uuid) -> Option<Vec<AliasInfo>>;
    /// A track's or album's covers, in order: (id, image hash).
    fn covers(&mut self, kind: PinKind, id: Uuid) -> Option<Vec<(Uuid, String)>>;
}

/// What a part asks for before it's a change.
#[derive(Debug, Clone, PartialEq)]
pub enum Want {
    /// The image in the clipboard, as a new cover (`at`: its place; None: last).
    Paste { at: Option<usize> },
    /// An image chosen in the file dialog.
    File { at: Option<usize> },
    /// The form correcting the source in this place (one track).
    Correct(usize),
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
    /// A new album's or grouping's kind, set when it's staged.
    pub new_kind: Option<(Uuid, String)>,
    /// What it asks for first (it's staged once that's there).
    pub want: Option<Want>,
}

/// The line's parts as units (a part with the `role:`, `as:`, `kind:` after it), each
/// resolved. Also says whether something was still loading (then it's worth asking again).
pub fn resolve(line: &str, ctx: &Ctx, look: &mut dyn Lookups) -> (Vec<Unit>, bool) {
    let parsed = parse::parse(line);
    let mut units = Vec::new();
    let mut waiting = false;
    let mut before = 0;
    for command in parsed.commands() {
        let (main, mods) = (&command[0], &command[1..]);
        let start = line[..main.value_start].rfind(|c: char| c.is_whitespace() || c == '|').map_or(0, |w| w + 1);
        let end = command[command.len() - 1].end;
        let text = line[start..end].trim().to_string();
        // A line album for `disc:` / `track:` after it.
        let line_album = parsed.parts[..before].iter().rev().find(|p| p.key == K::Album && !p.minus).and_then(|p| album(&p.value, ctx).ok()).map(|a| a.0);
        before += command.len();
        // `parse:` reads what's typed after it itself (its own quotes and words).
        let result = match main.key {
            K::Parse => parse_titles(&line[main.value_start..end], ctx, look, &mut waiting),
            _ => unit(main, mods, ctx, line_album, look, &mut waiting),
        };
        units.push(Unit { text, result });
    }
    (units, waiting)
}

fn unit(
    main: &Part,
    mods: &[Part],
    ctx: &Ctx,
    line_album: Option<Uuid>,
    look: &mut dyn Lookups,
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
            Some(p) => Err(format!("{}: doesn't go after {}:", p.name(), main.key.name())),
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
                    match look.aliases(id) {
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
                let shown = look.aliases(id).and_then(|l| l.into_iter().find(|a| a.id == alias)).map(|a| a.name).unwrap_or_default();
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
            Ok(Resolved { label, ops: vec![Op::Album { album: id, numbers }], new_kind: kind.map(|k| (id, k)), ..Default::default() })
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
            only(&[K::Kind])?;
            if main.minus && !main.colon {
                return Err("-group alone isn't anything: -group:name takes it out of that one".into());
            }
            if value.is_empty() {
                return Err(format!("{}group: type its name", if main.minus { "-" } else { "" }));
            }
            let (id, name, kind, new) = grouping(value, ctx)?;
            if main.minus {
                if new {
                    return Err(format!("“{name}” isn't made yet: nothing is in it"));
                }
                return done(format!("grouping: − “{name}” ({kind}), and a single it's alone on"), vec![Op::Ungroup(id)]);
            }
            let kind = match modifier(K::Kind) {
                Some(_) if !new => return Err("kind: only for a new grouping (a grouping's own kind is changed on its page)".into()),
                Some(p) if valid_grouping_kind(p.value.trim()) => p.value.trim().to_string(),
                Some(p) => return Err(format!("“{}” isn't a kind of grouping (lowercase letters: collective, era, project…)", p.value)),
                None => kind,
            };
            let label = if new { format!("grouping: + “{name}” (new {kind})") } else { format!("grouping: + “{name}” ({kind})") };
            Ok(Resolved { label: format!("{label}, and a single it's alone on"), ops: vec![Op::Group(id)], new_kind: new.then_some((id, kind)), ..Default::default() })
        }
        K::Flag => {
            only(&[])?;
            if main.minus && !main.colon {
                return Err("-flag alone isn't anything: -flag:name settles that kind of flag on it".into());
            }
            if value.is_empty() {
                return Err(format!("{}flag: type its name", if main.minus { "-" } else { "" }));
            }
            let typed = value.split_whitespace().collect::<Vec<_>>().join(" ");
            let same = ctx.vocab.review_kinds.iter().find(|k| k.name.eq_ignore_ascii_case(&typed) || k.kind.eq_ignore_ascii_case(&typed));
            if main.minus {
                let k = same.ok_or_else(|| format!("no kind of flag named “{typed}”"))?;
                return done(format!("flag: settles its “{}” flags", k.name), vec![Op::Unflag { kind: k.kind.clone(), name: k.name.clone() }]);
            }
            match same {
                Some(k) if !k.by_hand() => Err(format!("“{}” flags are raised by the server, not by hand", k.name)),
                Some(k) => done(format!("flag: + “{}”", k.name), vec![Op::Flag { kind: k.kind.clone(), name: k.name.clone(), new: false }]),
                None => done(format!("flag: + “{typed}” (a new kind of flag, listed on Review)"), vec![Op::Flag { kind: typed.clone(), name: typed, new: true }]),
            }
        }
        K::Single => {
            only(&[])?;
            if main.minus || !value.is_empty() {
                return Err("single: alone (a single of its own for each track); nothing after it".into());
            }
            done("a single of its own, for each (named after it as it'll be; its primary artists, date and covers)".into(), vec![Op::Single])
        }
        K::Suffix => {
            only(&[])?;
            let v = main.value.trim();
            if main.minus {
                return match v {
                    "" => Err("-suffix: from what on to take off: -suffix:\\| or -suffix:( or -suffix:\"| Some Tape\" (a | alone separates changes: \\| or quotes)".into()),
                    _ => done(format!("title: from the last “{v}” to the end taken off, where it has one (suffix: alone puts it back)"), vec![Op::CutSuffix(v.into())]),
                };
            }
            match v {
                "" => done("title: what -suffix: took off, put back".into(), vec![Op::AddSuffix(None)]),
                _ => done(format!("title: + “{v}” at the end, where it doesn't end with it"), vec![Op::AddSuffix(Some(v.into()))]),
            }
        }
        K::Why => {
            only(&[])?;
            done(format!("why: “{value}”"), vec![Op::Why(value.into())])
        }
        K::Along => {
            only(&[])?;
            match value.to_lowercase().as_str() {
                // (Left out, they follow what changes; typed, they're made to match.)
                "on" | "yes" | "" => done("albums it's alone on: made to match it (title, date, performers), changed or not".into(), vec![Op::Sync]),
                "off" | "no" => done("albums it's alone on: left as they are".into(), vec![Op::Along(false)]),
                _ => Err("along: on (made to match) or off".into()),
            }
        }
        K::Cover => {
            only(&[])?;
            let warning = many.then(|| format!("each of the {} tracks gets this cover", ctx.targets));
            if main.minus {
                return match (main.colon, main.hashes.as_slice()) {
                    (false, _) => done("covers → none (to the trash)".into(), vec![Op::NoCovers]),
                    (true, [h]) if value.is_empty() => {
                        let n = place(h, "cover")?;
                        done(format!("cover #{} → the trash", n + 1), vec![Op::NoCover(n)])
                    }
                    _ => Err("-cover:#N puts the Nth in the trash; -cover alone, all of them".into()),
                };
            }
            let lower = value.to_lowercase();
            match lower.as_str() {
                "" => match main.hashes.as_slice() {
                    [from, to] => {
                        let (from, to) = (place(from, "cover")?, place(to, "cover")?);
                        done(format!("cover #{}{}", from + 1, goes(Some(to))), vec![Op::MoveCover { from, to }])
                    }
                    _ => Err("cover: paste, file, or a track or album to copy one of its covers from (cover:#3 #1 makes the third the main one)".into()),
                },
                "paste" | "clipboard" | "file" | "browse" => {
                    let at = match main.hashes.as_slice() {
                        [] => None,
                        [h] => Some(place(h, "cover")?),
                        _ => return Err("one # after it: where it goes".into()),
                    };
                    let (what, want) = match lower.as_str() {
                        "paste" | "clipboard" => ("the image in the clipboard", Want::Paste { at }),
                        _ => ("an image from a file (choose it next)", Want::File { at }),
                    };
                    Ok(Resolved { label: format!("cover: + {what}{}", goes(at)), warning, want: Some(want), ..Default::default() })
                }
                _ => {
                    let name = if lower.starts_with("from:") { value[5..].trim() } else { value };
                    let (kind, id, title) = cover_owner(name, ctx)?;
                    let (which, at) = match main.hashes.as_slice() {
                        [] => (0, None),
                        [which] => (place(which, "cover")?, None),
                        [which, at] => (place(which, "cover")?, Some(place(at, "cover")?)),
                        _ => return Err("cover:… #which #where".into()),
                    };
                    let whose = match kind {
                        PinKind::Album => format!("the album “{title}”"),
                        _ => format!("the track “{title}”"),
                    };
                    let Some(covers) = look.covers(kind, id) else {
                        *waiting = true;
                        return Err(format!("looking up {whose}'s covers…"));
                    };
                    let Some((cover, sha)) = covers.get(which).cloned() else {
                        return Err(match covers.len() {
                            0 => format!("{whose} has no covers of its own"),
                            n => format!("{whose} has {n} covers"),
                        });
                    };
                    let which = if covers.len() > 1 { format!(" {}", which + 1) } else { String::new() };
                    Ok(Resolved {
                        label: format!("cover: + {whose}'s cover{which}{}", goes(at)),
                        ops: vec![Op::Cover { image: Image::CopyOf(cover), sha, at }],
                        warning,
                        ..Default::default()
                    })
                }
            }
        }
        K::Source => {
            only(&[K::Field])?;
            if !ctx.may_source {
                return Err("sources need the edit_sources permission".into());
            }
            if main.minus {
                return match (main.colon, main.hashes.as_slice()) {
                    (false, _) => done("sources → none (to the trash)".into(), vec![Op::NoSources]),
                    (true, [h]) if value.is_empty() => {
                        let n = place(h, "source")?;
                        done(format!("source #{} → the trash", n + 1), vec![Op::NoSource(n)])
                    }
                    _ => Err("-source:#N puts the Nth in the trash; -source alone, all of them".into()),
                };
            }
            // `source:=…`: its only source from now on, and where its file came from.
            let (replace, value) = match value.strip_prefix('=') {
                Some(rest) => (true, rest.trim()),
                None => (false, value),
            };
            if replace && value.is_empty() {
                return Err("source:= and a link (or custom and where it's from): its only source, the file from there too".into());
            }
            if value.is_empty() {
                return match main.hashes.as_slice() {
                    [h] if !many => {
                        let n = place(h, "source")?;
                        Ok(Resolved { label: format!("source #{}: correct it (its form opens)", n + 1), want: Some(Want::Correct(n)), ..Default::default() })
                    }
                    [_] => Err("correcting a source is one track's".into()),
                    _ => Err("source: a link, or custom and where it's from (custom vk, sent by a friend)".into()),
                };
            }
            let fields: Vec<(&str, &str)> = mods.iter().map(|p| (p.name(), p.value.trim())).collect();
            let typed = crate::ui::edit::sources::typed(value, &fields, "track", &ctx.vocab.source_shapes)?;
            let (platform, name, data, shown) = (typed.platform, typed.name, typed.data, typed.shown);
            let id = {
                use std::hash::{Hash, Hasher};
                let mut h = std::hash::DefaultHasher::new();
                (&platform, data.to_string()).hash(&mut h);
                Uuid::from_u128(0xACE7_5000_0000_4000_8000_0000_0000_0000 | h.finish() as u128)
            };
            let source = Op::Source { id, platform, name, data };
            if replace {
                let label = format!("sources → only {shown}, and the file came from it (the others to the trash)");
                return done(label, vec![Op::NoSources, source, Op::Origin(0)]);
            }
            let warning = many.then(|| format!("every one of the {} tracks gets this source; usually it's one track's", ctx.targets));
            Ok(Resolved { label: format!("source: + {shown}"), ops: vec![source], warning, ..Default::default() })
        }
        K::Origin => {
            only(&[])?;
            if !ctx.may_source {
                return Err("where a file came from needs the edit_sources permission".into());
            }
            if many {
                return Err("where a file came from is one track's".into());
            }
            match main.hashes.as_slice() {
                [h] if value.is_empty() && !main.minus => {
                    let n = place(h, "source")?;
                    done(format!("the file came from source #{}", n + 1), vec![Op::Origin(n)])
                }
                _ => Err("origin:#N: the file came from its Nth source".into()),
            }
        }
        K::Role | K::As | K::Kind => Err(format!("{}: goes after what it's about (artist:… role:…)", main.key.name())),
        K::Parse => Err("parse: on its own".into()),
        K::Field => Err(format!("{}: goes after source: and its platform (source:distrokid album_uuid:…)", main.name())),
        K::Split | K::Whole | K::Keep | K::Word => Err(format!("{}: goes after parse: (parse:feat prod {}:…)", main.key.name(), main.key.name())),
        K::Other(k) => Err(format!("{}: not something to change here", k.name())),
    }
}

/// `parse:` and what goes with it (`raw`, as typed): every target's title parsed with the
/// rules, and each name found matched to an artist exactly (see `title_parse`).
fn parse_titles(raw: &str, ctx: &Ctx, look: &mut dyn Lookups, waiting: &mut bool) -> Result<Resolved, String> {
    let setting = Setting::parse(raw, &|typed| find_in(&ctx.vocab.credit_roles, typed)).map_err(|e| format!("parse: {e}"))?;
    let (Some(titles), Some(table)) = (ctx.titles, ctx.artist_names) else {
        *waiting = true;
        return Err("parse: the tracks are still loading…".into());
    };
    let mut looking: Vec<String> = Vec::new();
    let parsed = TitlesParsed::new(&setting, titles.to_vec(), Fixes::default(), table, &mut |name| Some(credited(name, ctx, look, &mut looking))).ok_or("parse: still looking up names…")?;
    if !looking.is_empty() {
        *waiting = true;
        return Err(format!("parse: looking up the names of {}…", looking.join(", ")));
    }
    if parsed.changing() == 0 {
        return Err(format!("parse: {} finds nothing in {}", parsed.setting, if titles.len() == 1 { "its title" } else { "the titles" }));
    }
    let (label, warning) = parse_label(&parsed);
    Ok(Resolved { label, ops: vec![Op::ParseTitle(std::sync::Arc::new(parsed))], warning, ..Default::default() })
}

/// What a name found in a title credits: the artist chosen for it (`Names::pin`), a new one
/// staged by that name, else the artists that have it as their name or an alias, exactly
/// (ignoring case). Names whose aliases are still being looked up go in `looking` (each is
/// asked for at once, then it's tried again).
pub fn credited(name: &str, ctx: &Ctx, look: &mut dyn Lookups, looking: &mut Vec<String>) -> Credited {
    let name = name.trim();
    let mut artist = |id: Uuid| -> Credited {
        if ctx.catalog.artist(id).is_some_and(|a| a.name.eq_ignore_ascii_case(name)) {
            return Credited::Artist { id, alias: None };
        }
        match look.aliases(id) {
            Some(list) => Credited::Artist { id, alias: list.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.id) },
            None => {
                looking.push(name.to_string());
                Credited::Artist { id, alias: None }
            }
        }
    };
    if let Some(id) = ctx.names.pinned(PinKind::Artist, name) {
        return if ctx.names.is_new(id) { Credited::New(Some(id)) } else { artist(id) };
    }
    if let Some((id, _)) = ctx.names.new_artists.iter().find(|a| a.1.eq_ignore_ascii_case(name)) {
        return Credited::New(Some(*id));
    }
    match ctx.artist_names.map(|t| t.get(name)).unwrap_or_default() {
        [] => Credited::New(None),
        [id] => artist(*id),
        ids => Credited::Undecided(ids.to_vec()),
    }
}

/// What a staged `parse:` says it does, and what's worth a look.
pub fn parse_label(parsed: &TitlesParsed) -> (String, Option<String>) {
    let new = parsed.names.iter().filter(|n| matches!(n.credited, Credited::New(_))).count();
    let undecided: Vec<String> = parsed.undecided().iter().map(|n| format!("“{}”", n.text)).collect();
    let mut label = format!("parse: {}", parsed.setting);
    label.push_str(&match parsed.titles.as_slice() {
        [(_, title)] => {
            let o = &parsed.outcomes[title];
            let names: Vec<String> = o.credits.iter().map(|c| format!("{} ({})", parsed.names[c.name].text, c.role)).collect();
            format!(" → “{}”, {}", o.title, if names.is_empty() { "no credits".into() } else { format!("+ {}", names.join(", ")) })
        }
        titles => format!(" → {} of {} tracks, {} names", parsed.changing(), titles.len(), parsed.names.len()),
    });
    if new > 0 {
        label.push_str(&format!(" ({new} new artist{})", if new == 1 { "" } else { "s" }));
    }
    let by_hand = parsed.fixes.not_names.len() + parsed.fixes.split.len() + parsed.fixes.whole.len() + parsed.skip.len();
    if by_hand > 0 {
        label.push_str(&format!(", {by_hand} corrected by hand"));
    }
    let mut warnings = Vec::new();
    if !undecided.is_empty() {
        warnings.push(format!("more than one artist is named {}: choose which in the check (or leave its tracks out) before applying", undecided.join(", ")));
    }
    match parsed.titles.as_slice() {
        [(_, title)] => warnings.extend(parsed.outcomes[title].warnings.iter().map(|w| w.text())),
        _ => match parsed.to_check() {
            0 => {}
            1 => warnings.push("1 track has something to look at".into()),
            n => warnings.push(format!("{n} tracks have something to look at")),
        },
    }
    (label, (!warnings.is_empty()).then(|| warnings.join("; ")))
}

/// A grouping by name: chosen, new, or the library's (the same name, then one like it).
/// (id, name, kind, whether it's new.)
fn grouping(value: &str, ctx: &Ctx) -> Result<(Uuid, String, String, bool), String> {
    let known = |id: Uuid| ctx.catalog.groupings.iter().find(|g| g.id == id).map(|g| (g.id, g.name.clone(), g.kind.clone(), false));
    let new = |id: Uuid| ctx.names.new_groupings.iter().find(|g| g.0 == id).map(|g| (g.0, g.1.clone(), g.2.clone(), true));
    if let Some(id) = ctx.names.pinned(PinKind::Grouping, value) {
        return known(id).or_else(|| new(id)).ok_or_else(|| format!("no grouping like “{value}”"));
    }
    if let Some(g) = ctx.names.new_groupings.iter().find(|g| g.1.eq_ignore_ascii_case(value.trim())) {
        return Ok((g.0, g.1.clone(), g.2.clone(), true));
    }
    let lower = value.to_lowercase();
    let found = ctx.catalog.groupings.iter().find(|g| g.name.to_lowercase() == lower).or_else(|| ctx.catalog.groupings.iter().find(|g| loosely_in(value, &g.name)));
    found.and_then(|g| known(g.id)).ok_or_else(|| format!("no grouping like “{value}” (a new one: choose “New grouping” in the list)"))
}

/// A kind of grouping is lowercase letters (and `_`), as the server says.
fn valid_grouping_kind(kind: &str) -> bool {
    kind.starts_with(|c: char| c.is_ascii_lowercase()) && kind.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

/// `#N` as a place from 0.
fn place(hash: &str, what: &str) -> Result<usize, String> {
    hash.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1).ok_or_else(|| format!("#{hash}: a {what}'s place is a number from 1"))
}

/// Where a cover goes, in words (None: last).
fn goes(at: Option<usize>) -> String {
    match at {
        None => String::new(),
        Some(0) => " as the main one".into(),
        Some(n) => format!(" as #{}", n + 1),
    }
}

/// The track or album to copy a cover from: the one chosen, else the best album, then
/// track, by that name.
fn cover_owner(value: &str, ctx: &Ctx) -> Result<(PinKind, Uuid, String), String> {
    let title = |kind: PinKind, id: Uuid| match kind {
        PinKind::Album => album_title(ctx, id),
        _ => ctx.catalog.track_index(id).map(|i| ctx.catalog.tracks[i as usize].title.clone()).unwrap_or_else(|| value.to_string()),
    };
    if let Some((kind, id)) = ctx.names.pinned_any(&[PinKind::Album, PinKind::Track], value) {
        return Ok((kind, id, title(kind, id)));
    }
    if let Some(i) = Search::default().run_part(ctx.catalog, value, SearchPart::Albums, 1).first().map(|h| h.0) {
        let a = &ctx.catalog.albums[i as usize];
        return Ok((PinKind::Album, a.id, a.title.clone()));
    }
    match Search::default().run_part(ctx.catalog, value, SearchPart::Tracks, 1).first().map(|h| h.0) {
        Some(i) => {
            let t = &ctx.catalog.tracks[i as usize];
            Ok((PinKind::Track, t.id, t.title.clone()))
        }
        None => Err(format!("no album or track like “{value}” to copy a cover from")),
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
    /// The next part to type after it, on Tab (`role:`, `as:`, `kind:`, `#`, a source's
    /// first needed field).
    pub next: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Pick {
    Pin(PinKind, Uuid),
    NewArtist,
    NewAlbum,
    NewGrouping,
}

/// Which library search the part being typed needs, if any (a cover's: albums, and
/// tracks too, see `cover_value`).
pub fn search_for(part: &Part) -> Option<SearchPart> {
    match part.key {
        K::Artist | K::Featured => Some(SearchPart::Artists),
        K::Album => Some(SearchPart::Albums),
        K::Related => Some(SearchPart::Tracks),
        K::Cover if cover_value(part).is_some() => Some(SearchPart::Albums),
        _ => None,
    }
}

/// A cover's value when it names a track or album to copy from (not `paste`, `file`, a
/// `#`…).
pub fn cover_value(part: &Part) -> Option<String> {
    let value = part.value.trim();
    let value = if value.to_lowercase().starts_with("from:") { value[5..].trim() } else { value };
    let word = ["paste", "clipboard", "file", "browse"].iter().any(|w| w.starts_with(&value.to_lowercase()));
    (part.key == K::Cover && !part.minus && !value.is_empty() && !word && part.hash.is_none()).then(|| value.to_string())
}

/// The choices for the last part of the line. `group`: for a modifier, the part it goes
/// with (`role:` after `artist:`…) and the modifiers between (a source's fields given
/// already); `hits`: the library's matches for its value (see `search_for`); `track_hits`:
/// tracks' too, for a cover.
pub fn candidates(part: &Part, group: &[Part], ctx: &Ctx, hits: &[u32], track_hits: &[u32], look: &mut dyn Lookups) -> Vec<Candidate> {
    let owner = group.first();
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
                out.push(Candidate { label: name.clone(), sub, text: a.label(), pick: Some(Pick::Pin(PinKind::Artist, a.id)), next: (!part.minus).then(|| "role:".into()) });
            }
            if !value.is_empty() && !part.minus {
                out.push(Candidate { label: format!("New artist “{value}”"), sub: "made when it's applied".into(), text: value.to_string(), pick: Some(Pick::NewArtist), next: Some("role:".into()) });
            }
        }
        K::Album => {
            for &i in hits {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Albums, i);
                let a = &ctx.catalog.albums[i as usize];
                out.push(Candidate { label: title, sub, text: a.title.clone(), pick: Some(Pick::Pin(PinKind::Album, a.id)), next: (!part.minus).then(|| "#".into()) });
            }
            if !value.is_empty() && !part.minus {
                out.push(Candidate { label: format!("New album “{value}”"), sub: "made when it's applied".into(), text: value.to_string(), pick: Some(Pick::NewAlbum), next: Some("kind:".into()) });
            }
        }
        K::Related => {
            for &i in hits {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Tracks, i);
                let t = &ctx.catalog.tracks[i as usize];
                out.push(Candidate { label: title, sub, text: t.title.clone(), pick: Some(Pick::Pin(PinKind::Track, t.id)), next: (!part.minus).then(|| "kind:".into()) });
            }
        }
        K::Status => out = simple(&ctx.vocab.statuses, &|s| crate::ui::edit::capitalized(s)),
        K::Parse => {
            // The rules not typed yet, for the word being typed.
            let mut words: Vec<&str> = value.split_whitespace().collect();
            let typing = if value.is_empty() || part.value.ends_with(char::is_whitespace) { "" } else { words.pop().unwrap_or("") };
            // (`loose` only after `prefix`.)
            let fits = |r: &str| r != "loose" || words.last().is_some_and(|w| w.to_lowercase().starts_with("prefix"));
            for (rule, what) in crate::title_credits::RULES.iter().filter(|(r, _)| !words.contains(r) && fits(r) && r.starts_with(&typing.to_lowercase())) {
                let text = words.iter().copied().chain([*rule]).collect::<Vec<_>>().join(" ");
                out.push(Candidate { label: rule.to_string(), sub: what.to_string(), text, pick: None, next: None });
            }
        }
        K::Whole => out = simple(&["on".to_string(), "off".to_string()], &|s| format!("whole:{s} (names that are an artist's kept whole)")),
        K::Role => {
            out = simple(&ctx.vocab.credit_roles, &|s| crate::ui::edit::capitalized(s));
            for c in &mut out {
                c.next = Some("as:".into());
            }
        }
        K::As => {
            let artist = owner.filter(|o| matches!(o.key, K::Artist | K::Featured)).and_then(|o| artist(&o.value, ctx).ok());
            if let Some((id, name)) = artist {
                let mut names = vec![name.clone()];
                if !ctx.names.is_new(id) {
                    names.extend(look.aliases(id).unwrap_or_default().into_iter().map(|a| a.name));
                }
                out = simple(&names, &|s| if s == name { format!("{s} (its own name)") } else { s.to_string() });
                for c in &mut out {
                    c.next = Some("#".into());
                }
            }
        }
        K::Kind => match owner.map(|o| o.key) {
            Some(K::Album) => out = simple(&ctx.vocab.album_kinds, &|s| format!("New {s}")),
            Some(K::Group) => {
                let kinds: Vec<String> = crate::ui::views::grouping::KINDS.iter().map(|k| k.to_string()).collect();
                out = simple(&kinds, &|s| format!("New {s}"));
            }
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
        K::Flag => {
            let typed = value.split_whitespace().collect::<Vec<_>>().join(" ");
            for k in ctx.vocab.review_kinds.iter().filter(|k| (part.minus || k.by_hand()) && (typed.is_empty() || k.name.to_lowercase().starts_with(&typed.to_lowercase()) || loosely_in(&typed, &k.name))) {
                let sub = if k.custom { "made by people" } else if k.automatic { "the server's" } else { "" };
                out.push(Candidate { label: k.name.clone(), sub: sub.into(), text: k.name.clone(), pick: None, next: None });
            }
            if !typed.is_empty() && !part.minus && !ctx.vocab.review_kinds.iter().any(|k| k.name.eq_ignore_ascii_case(&typed)) {
                out.push(Candidate { label: format!("New flag “{typed}”"), sub: "a kind of flag, made when it's applied".into(), text: typed.clone(), pick: None, next: None });
            }
        }
        K::Group => {
            for g in ctx.catalog.groupings.iter().filter(|g| value.is_empty() || g.name.to_lowercase().starts_with(&value.to_lowercase()) || loosely_in(value, &g.name)) {
                out.push(Candidate { label: g.name.clone(), sub: g.kind.clone(), text: g.name.clone(), pick: Some(Pick::Pin(PinKind::Grouping, g.id)), next: None });
            }
            if !value.is_empty() && !part.minus && !ctx.catalog.groupings.iter().any(|g| g.name.eq_ignore_ascii_case(value)) {
                out.push(Candidate { label: format!("New grouping “{value}”"), sub: "made when it's applied".into(), text: value.to_string(), pick: Some(Pick::NewGrouping), next: Some("kind:".into()) });
            }
        }
        K::Cover if !part.minus && part.hash.is_none() => {
            let words = [("paste", "the image in the clipboard (or Ctrl+V anywhere here)"), ("file", "choose an image file")];
            for (word, sub) in words {
                if value.is_empty() || word.starts_with(&value.to_lowercase()) {
                    out.push(Candidate { label: word.into(), sub: sub.into(), text: word.into(), pick: None, next: Some("#".into()) });
                }
            }
            for &i in hits.iter().take(5) {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Albums, i);
                let a = &ctx.catalog.albums[i as usize];
                out.push(Candidate { label: format!("its cover: {title}"), sub, text: a.title.clone(), pick: Some(Pick::Pin(PinKind::Album, a.id)), next: Some("#".into()) });
            }
            for &i in track_hits.iter().take(5) {
                let (title, sub) = crate::ui::edit::inline_search::describe(ctx.catalog, SearchPart::Tracks, i);
                let t = &ctx.catalog.tracks[i as usize];
                out.push(Candidate { label: format!("its cover: {title}"), sub, text: t.title.clone(), pick: Some(Pick::Pin(PinKind::Track, t.id)), next: Some("#".into()) });
            }
        }
        K::Source if !part.minus && part.hash.is_none() && ctx.may_source => {
            let (equals, rest) = match value.strip_prefix('=') {
                Some(rest) => ("=", rest.trim()),
                None => ("", value),
            };
            for c in crate::ui::edit::sources::platform_choices(rest, equals, "track", &ctx.vocab.source_shapes) {
                out.push(Candidate { label: c.label, sub: c.sub, text: c.text, pick: None, next: c.next });
            }
        }
        // A source's field: what it takes, and on Tab the next field (needed ones first).
        K::Field => {
            let Some(owner) = owner.filter(|o| o.key == K::Source) else { return out };
            let given: Vec<&str> = group.iter().skip(1).chain([part]).map(|p| p.name()).collect();
            for c in crate::ui::edit::sources::field_choices(&owner.value, part.name(), value, &given, "track", &ctx.vocab.source_shapes) {
                out.push(Candidate { label: c.label, sub: c.sub, text: c.text, pick: None, next: c.next });
            }
        }
        K::Along => {
            let label = |s: &str| match s {
                "on" => "on: albums it's alone on made to match it, changed or not".to_string(),
                _ => "off: only the tracks".to_string(),
            };
            out = simple(&["on".to_string(), "off".to_string()], &label);
        }
        _ => {}
    }
    // (Every rule of `parse:`: there are a dozen, and the last ones were cut off.)
    if part.key != K::Parse {
        out.truncate(10);
    }
    out
}

/// For plain words: the things to change by that name, each with what it puts in the line.
pub fn actions(words: &str, ctx: &Ctx) -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = vec![
        ("Title…".into(), "title:".into()),
        ("Title: its credits into credits (parse feat, prod…)".into(), "parse:".into()),
        ("Title: take a suffix off the end…".into(), "-suffix:".into()),
        ("Title: add a suffix (alone: put back what was taken off)…".into(), "suffix:".into()),
        ("Artists: add…".into(), "artist:".into()),
        ("Artists: add as featured…".into(), "feat:".into()),
        ("Artists: replace theirs with…".into(), "-artist | artist:".into()),
        ("Artists: take one off…".into(), "-artist:".into()),
        ("Album: on it as well…".into(), "album:".into()),
        ("Album: on it only (replacing theirs)…".into(), "-album | album:".into()),
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
        ("Albums it's alone on: make them match it".into(), "along:on".into()),
        ("Single: a single of its own, for each".into(), "single:".into()),
        ("Grouping: into one (or a new one)…".into(), "group:".into()),
        ("Grouping: out of one…".into(), "-group:".into()),
    ];
    for status in &ctx.vocab.statuses {
        all.push((format!("Status: {status}"), format!("status:{status}")));
    }
    if ctx.may_relate {
        all.push(("Related track…".into(), "related:".into()));
    }
    all.extend([
        ("Cover: paste the copied image".to_string(), "cover:paste".to_string()),
        ("Cover: an image file…".into(), "cover:file".into()),
        ("Cover: copy another album's or track's…".into(), "cover:".into()),
        ("Covers: none (to the trash)".into(), "-cover".into()),
    ]);
    if ctx.may_source {
        all.extend([
            ("Source: add a link…".to_string(), "source:".to_string()),
            ("Source: custom (where it's from)…".into(), "source:custom ".into()),
            ("Sources: replace them all with one, the file from there too…".into(), "source:=".into()),
            ("Source: the file came from… (origin)".into(), "origin:#".into()),
        ]);
    }
    for g in &ctx.catalog.groupings {
        all.push((format!("Grouping: into “{}” ({})", g.name, g.kind), format!("group:{}", parse::quoted(&g.name))));
    }
    all.into_iter().filter(|(label, _)| loosely_in(words, label)).take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{RelationKind, ReviewKind};
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
            review_kinds: vec![
                ReviewKind { kind: "fix_later".into(), name: "Fix later".into(), automatic: false, custom: false },
                ReviewKind { kind: "missing_cover".into(), name: "No cover".into(), automatic: true, custom: false },
                ReviewKind { kind: "missing_sc".into(), name: "Missing SoundCloud metadata".into(), automatic: false, custom: true },
            ],
            ..Default::default()
        }
    }

    /// The line's units (ops, or the error), for `targets` tracks.
    fn run(line: &str, names: &Names, targets: usize) -> Vec<Result<(Vec<Op>, Option<String>), String>> {
        let (catalog, vocab) = (catalog(), vocab());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets, context_album: None, staged_album: None, names, may_relate: true, may_source: true, titles: None, artist_names: None };
        resolve(line, &ctx, &mut Known).0.into_iter().map(|u| u.result.map(|r| (r.ops, r.warning))).collect()
    }

    /// No aliases; Long Player's covers: two.
    struct Known;

    impl Lookups for Known {
        fn aliases(&mut self, _: Uuid) -> Option<Vec<AliasInfo>> {
            Some(Vec::new())
        }

        fn covers(&mut self, _: PinKind, id: Uuid) -> Option<Vec<(Uuid, String)>> {
            Some(if id == self::id(LP) { vec![(Uuid::from_u128(1), "1".repeat(64)), (Uuid::from_u128(2), "2".repeat(64))] } else { Vec::new() })
        }
    }

    fn ops(line: &str) -> Vec<Op> {
        run(line, &Names::default(), 1).into_iter().flat_map(|u| u.unwrap().0).collect()
    }

    /// Bo's alias "Bobo" (looked up), and the two Anns.
    struct Aliases;

    impl Lookups for Aliases {
        fn aliases(&mut self, artist: Uuid) -> Option<Vec<AliasInfo>> {
            Some(if artist == id(BO) { vec![AliasInfo { id: Uuid::from_u128(0xb0b0), name: "Bobo".into() }] } else { Vec::new() })
        }

        fn covers(&mut self, _: PinKind, _: Uuid) -> Option<Vec<(Uuid, String)>> {
            Some(Vec::new())
        }
    }

    /// `parse:` on tracks with `titles`: (label, the staged parse, the warning), or why not.
    fn parse_line(line: &str, names: &Names, titles: &[&str]) -> Result<(String, std::sync::Arc<TitlesParsed>, Option<String>), String> {
        let (catalog, vocab) = (catalog(), vocab());
        let titles: Vec<(Uuid, String)> = titles.iter().enumerate().map(|(i, t)| (Uuid::from_u128(1000 + i as u128), t.to_string())).collect();
        let table = NameTable::new([(id(ANN), "Ann"), (id(ANN_UK), "Ann"), (id(BO), "Bo"), (id(BO), "Bobo")]);
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: titles.len(), context_album: None, staged_album: None, names, may_relate: true, may_source: true, titles: Some(&titles), artist_names: Some(&table) };
        let (mut units, _) = resolve(line, &ctx, &mut Aliases);
        assert_eq!(units.len(), 1, "{line}");
        let r = units.remove(0).result?;
        match r.ops.as_slice() {
            [Op::ParseTitle(p)] => Ok((r.label, p.clone(), r.warning)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn suffixes() {
        assert_eq!(ops("-suffix:\\|"), [Op::CutSuffix("|".into())]);
        assert_eq!(ops("-suffix:\"|\""), [Op::CutSuffix("|".into())]);
        assert_eq!(ops("-suffix:\"| Some Tape\""), [Op::CutSuffix("| Some Tape".into())]);
        assert_eq!(ops("suffix:"), [Op::AddSuffix(None)]);
        assert_eq!(ops("suffix:\\| Some Tape"), [Op::AddSuffix(Some("| Some Tape".into()))]);
        assert_eq!(ops("-suffix:("), [Op::CutSuffix("(".into())]);
        assert!(run("-suffix:", &Names::default(), 1)[0].as_ref().unwrap_err().contains("from what on"));
    }

    #[test]
    fn parse_titles_names_matched_exactly() {
        let (label, p, warning) = parse_line("parse:feat prod", &Names::default(), &["Song (feat. Bo)", "Other [prod. Cy]", "Plain"]).unwrap();
        assert_eq!(label, "parse: prod feat → 2 of 3 tracks, 2 names (1 new artist)");
        assert_eq!(warning, None);
        let credited: Vec<(&str, &Credited)> = p.names.iter().map(|n| (n.text.as_str(), &n.credited)).collect();
        assert_eq!(credited, [("Bo", &Credited::Artist { id: id(BO), alias: None }), ("Cy", &Credited::New(None))]);
        // An alias: credited under it.
        let (_, p, _) = parse_line("parse:feat", &Names::default(), &["Song (feat. BOBO)"]).unwrap();
        assert_eq!(p.names[0].credited, Credited::Artist { id: id(BO), alias: Some(Uuid::from_u128(0xb0b0)) });
        // Not "artist like": "Bob" is new, not Bo.
        let (_, p, _) = parse_line("parse:feat", &Names::default(), &["Song (feat. Bob)"]).unwrap();
        assert_eq!(p.names[0].credited, Credited::New(None));
    }

    #[test]
    fn parse_titles_one_track_says_it_all() {
        let (label, _, warning) = parse_line("parse:feat prod", &Names::default(), &["Song (feat. Bo) [prod. Bo]"]).unwrap();
        assert_eq!(label, "parse: prod feat → “Song”, + Bo (producer), Bo (featured)");
        assert_eq!(warning, None);
        let (_, _, warning) = parse_line("parse:feat", &Names::default(), &["Song (feat. Bo]"]).unwrap();
        assert!(warning.unwrap().contains("bracket"));
    }

    #[test]
    fn parse_titles_two_anns_are_chosen_not_guessed() {
        let (_, p, warning) = parse_line("parse:feat", &Names::default(), &["Song (feat. ann)", "Other"]).unwrap();
        assert_eq!(p.names[0].credited, Credited::Undecided(vec![id(ANN), id(ANN_UK)]));
        assert!(warning.unwrap().contains("more than one artist is named “ann”"));
        // One chosen in the list for that name: that one.
        let mut names = Names::default();
        names.pin(PinKind::Artist, "Ann", id(ANN_UK));
        let (_, p, warning) = parse_line("parse:feat", &names, &["Song (feat. ann)"]).unwrap();
        assert_eq!(p.names[0].credited, Credited::Artist { id: id(ANN_UK), alias: None });
        assert_eq!(warning, None);
        // A new artist staged by that name already: the same one.
        let mut names = Names::default();
        let new = names.new_artist("Cy");
        let (_, p, _) = parse_line("parse:feat", &names, &["Song (feat. CY)"]).unwrap();
        assert_eq!(p.names[0].credited, Credited::New(Some(new)));
    }

    #[test]
    fn parse_titles_overrides_and_refusals() {
        let (_, p, _) = parse_line("parse:feat split:-x keep:tape -cover", &Names::default(), &["Song (feat. Bo x Cy)"]).unwrap();
        assert_eq!(p.setting, "feat split:-x keep:tape -cover");
        assert_eq!(p.names[0].text, "Bo x Cy");
        let (_, p, _) = parse_line("parse:feat word:\"beat by\"=prod", &Names::default(), &["Song beat by Bo"]).unwrap();
        assert_eq!(p.names[0].text, "Bo");
        for (line, why) in [
            ("parse:nope", "parse: no rule like “nope”"),
            ("parse:feat role:producer", "parse: no rule like “role:producer”"),
            ("parse:prod", "parse: prod finds nothing in the titles"),
        ] {
            let err = parse_line(line, &Names::default(), &["Song (feat. Bo)", "Plain"]).unwrap_err();
            assert!(err.contains(why), "{line}: {err}");
        }
        assert!(run("split:-x", &Names::default(), 1)[0].as_ref().unwrap_err().contains("goes after parse:"));
        // Not loaded yet.
        assert!(run("parse:feat", &Names::default(), 1)[0].as_ref().unwrap_err().contains("still loading"));
    }

    #[test]
    fn flags_by_hand() {
        let flag = |kind: &str, name: &str, new: bool| Op::Flag { kind: kind.into(), name: name.into(), new };
        assert_eq!(ops("flag:missing   soundcloud METADATA"), [flag("missing_sc", "Missing SoundCloud metadata", false)]);
        assert_eq!(ops("flag:fix later"), [flag("fix_later", "Fix later", false)]);
        // A name no kind has: a new kind, named as typed.
        assert_eq!(ops("flag:Ask Bo"), [flag("Ask Bo", "Ask Bo", true)]);
        // The server's kinds are settled by hand, not raised.
        assert!(run("flag:no cover", &Names::default(), 1)[0].as_ref().unwrap_err().contains("raised by the server"));
        assert_eq!(ops("-flag:No cover"), [Op::Unflag { kind: "missing_cover".into(), name: "No cover".into() }]);
        assert!(run("-flag:nothing like it", &Names::default(), 1)[0].is_err());
    }

    #[test]
    fn lines_staged_for_you_resolve_to_what_they_name() {
        // As the "Didn't import" page stages them (`version_of`): names pinned, quoted
        // where needed.
        let song = id("00000000-0000-7000-8000-000000000001");
        let mut names = Names::default();
        names.pin(PinKind::Track, "Other Song", song);
        names.pin(PinKind::Artist, "Ann", id(ANN_UK));
        let line = |l: &str| run(l, &names, 1).into_iter().flat_map(|u| u.unwrap().0).collect::<Vec<Op>>();
        assert_eq!(line("title:\"Other: Song (Remix)\""), [Op::Title("Other: Song (Remix)".into())]);
        assert_eq!(line("related:Other Song kind:remix_of"), [Op::Relate { track: song, kind: "remix_of".into() }]);
        assert_eq!(line("artist:Ann role:primary"), [Op::Credit { artist: id(ANN_UK), role: Some("primary".into()), alias: None, at: None }]);
        assert_eq!(line("-artist:Ann"), [Op::Uncredit { artist: id(ANN_UK), role: None }]);
    }

    #[test]
    fn fields_are_found_by_their_starts() {
        assert_eq!(
            ops("status:dem | date:2020 | genre:rap, trap"),
            [Op::Status("demo".into()), Op::Date(Some("2020".into())), Op::Genre("rap".into()), Op::Genre("trap".into())]
        );
        assert_eq!(ops("date:none | -genre"), [Op::Date(None), Op::NoGenres]);
        assert_eq!(ops("group:crew | -group:the crew | single:"), [Op::Group(id("00000000-0000-7000-8000-0000000000c1")), Op::Ungroup(id("00000000-0000-7000-8000-0000000000c1")), Op::Single]);
        assert_eq!(ops("along:off | why:fixing tags"), [Op::Along(false), Op::Why("fixing tags".into())]);
        assert_eq!(ops("along:on"), [Op::Sync]);
        assert!(run("along:sync", &Names::default(), 1)[0].is_err());
    }

    #[test]
    fn artists_with_role_place_and_replacing() {
        assert_eq!(ops("artist:Bo role:prod #1"), [Op::Credit { artist: id(BO), role: Some("producer".into()), alias: None, at: Some(0) }]);
        assert_eq!(ops("feat:Bo"), [Op::Credit { artist: id(BO), role: Some("featured".into()), alias: None, at: None }]);
        assert_eq!(ops("-artist | artist:Bo"), [Op::NoCredits, Op::Credit { artist: id(BO), role: None, alias: None, at: None }]);
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
        assert_eq!(ops("-album | album:long player | track:1+"), [Op::NoAlbums, Op::Album { album: lp, numbers: Numbers::Keep }, Op::Album { album: lp, numbers: Numbers::InOrder(1) }]);
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
        for line in ["title:", "role:producer", "format:flac", "status:nope", "-group", "group:nobody like this", "single:yes"] {
            assert!(run(line, &Names::default(), 1)[0].is_err(), "{line}");
        }
    }

    #[test]
    fn covers_copied_moved_and_trashed() {
        let copy = |n: u128, at| Op::Cover { image: Image::CopyOf(Uuid::from_u128(n)), sha: n.to_string().repeat(64), at };
        assert_eq!(ops("cover:long player"), [copy(1, None)]);
        assert_eq!(ops("cover:from:Long Player #2 #1"), [copy(2, Some(0))]);
        assert_eq!(ops("cover:#3 #1 | -cover:#2 | -cover"), [Op::MoveCover { from: 2, to: 0 }, Op::NoCover(1), Op::NoCovers]);
        assert!(run("cover:long player #3", &Names::default(), 1)[0].is_err(), "it has two");
        assert!(run("cover:other song", &Names::default(), 1)[0].is_err(), "no covers of its own");
        // Pasted, and from a file: asked for first.
        let (catalog, vocab, names) = (catalog(), vocab(), Names::default());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 2, context_album: None, staged_album: None, names: &names, may_relate: true, may_source: true, titles: None, artist_names: None };
        let (units, _) = resolve("cover:paste #1 | cover:file", &ctx, &mut Known);
        let r = units[0].result.as_ref().unwrap();
        assert_eq!((r.want.clone(), r.ops.is_empty(), r.warning.is_some()), (Some(Want::Paste { at: Some(0) }), true, true));
        assert_eq!(units[1].result.as_ref().unwrap().want, Some(Want::File { at: None }));
    }

    #[test]
    fn sources_from_links_custom_ones_and_the_origin() {
        let mut vocab = vocab();
        let field = |name: &str, kind: &str, required| crate::account::ShapeField { name: name.into(), kind: kind.into(), required };
        vocab.source_shapes = vec![
            crate::account::SourceShape { platform: "netease".into(), kind: "track".into(), id_field: Some("song_id".into()), fields: vec![field("song_id", "int", true)] },
            crate::account::SourceShape {
                platform: "custom".into(),
                kind: "track".into(),
                id_field: None,
                fields: vec![field("label", "str", true), field("description", "str", false), field("url", "str", false)],
            },
        ];
        let (catalog, names) = (catalog(), Names::default());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 1, context_album: None, staged_album: None, names: &names, may_relate: true, may_source: true, titles: None, artist_names: None };
        let (units, _) = resolve("source:https://music.163.com/#/song?id=123 | source:custom vk, sent by a friend | origin:#2 | -source:#1", &ctx, &mut Known);
        let ops: Vec<Op> = units.iter().flat_map(|u| u.result.clone().unwrap().ops).collect();
        let [Op::Source { platform, name, data, .. }, Op::Source { data: custom, .. }, Op::Origin(1), Op::NoSource(0)] = ops.as_slice() else { panic!("{ops:?}") };
        assert_eq!((platform.as_str(), name.as_str(), data.clone()), ("netease", "123", serde_json::json!({ "song_id": 123 })));
        assert_eq!(custom, &serde_json::json!({ "label": "vk", "description": "sent by a friend" }));
        assert_eq!(resolve("source:#1", &ctx, &mut Known).0[0].result.as_ref().unwrap().want, Some(Want::Correct(0)));
        // Replacing them all, for many tracks too: every one off, this one on, the file from it.
        let many = Ctx { targets: 3, ..ctx };
        let (units, _) = resolve("source:= custom google drive, a shared folder", &many, &mut Known);
        let r = units[0].result.as_ref().unwrap();
        assert!(r.warning.is_none());
        let [Op::NoSources, Op::Source { data, .. }, Op::Origin(0)] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "label": "google drive", "description": "a shared folder" }));
        let ctx = Ctx { targets: 1, ..many };
        assert!(resolve("source:=", &ctx, &mut Known).0[0].result.is_err());
        // Not a link; and without the permission.
        assert!(resolve("source:somewhere", &ctx, &mut Known).0[0].result.is_err());
        let ctx = Ctx { may_source: false, ..ctx };
        assert!(resolve("origin:#1", &ctx, &mut Known).0[0].result.is_err());
    }

    #[test]
    fn sources_typed_field_by_field() {
        let mut vocab = vocab();
        let field = |name: &str, kind: &str, required| crate::account::ShapeField { name: name.into(), kind: kind.into(), required };
        vocab.source_shapes = vec![
            crate::account::SourceShape { platform: "distrokid".into(), kind: "track".into(), id_field: None, fields: vec![field("album_uuid", "str", true), field("index", "int", false)] },
            crate::account::SourceShape {
                platform: "soundcloud".into(),
                kind: "track".into(),
                id_field: Some("song_id".into()),
                fields: vec![field("song_id", "int", false), field("song_permalinks", "strs", false)],
            },
            crate::account::SourceShape {
                platform: "custom".into(),
                kind: "track".into(),
                id_field: None,
                fields: vec![field("label", "str", true), field("description", "str", false), field("url", "str", false)],
            },
        ];
        let (catalog, names) = (catalog(), Names::default());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 1, context_album: None, staged_album: None, names: &names, may_relate: true, may_source: true, titles: None, artist_names: None };
        let one = |line: &str| resolve(line, &ctx, &mut Known).0.remove(0).result;

        let r = one("source:distrokid album_uuid:0b358691-d0db-4fa4-b494-1cdf92f034dc index:1").unwrap();
        let [Op::Source { platform, name, data, .. }] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "album_uuid": "0b358691-d0db-4fa4-b494-1cdf92f034dc", "index": 1 }));
        assert_eq!((platform.as_str(), name.as_str()), ("distrokid", "album_uuid:0b358691-d0db-4fa4-b494-1cdf92f034dc index:1"));
        assert_eq!(r.label, "source: + distrokid album_uuid:0b358691-d0db-4fa4-b494-1cdf92f034dc index:1");
        // Its only source, the file from it; any order, any case.
        let r = one("source:= DistroKid index:2 album_uuid:x").unwrap();
        let [Op::NoSources, Op::Source { data, .. }, Op::Origin(0)] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "album_uuid": "x", "index": 2 }));
        // A list: its field once per item; a value runs until the next field.
        let r = one("source:soundcloud song_permalinks:a/b song_permalinks:a/c song_id:5").unwrap();
        let [Op::Source { name, data, .. }] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!((name.as_str(), data.clone()), ("5", serde_json::json!({ "song_id": 5, "song_permalinks": ["a/b", "a/c"] })));
        let r = one("source:custom label:vk description:sent by a friend url:https://vk.com/x").unwrap();
        let [Op::Source { data, .. }] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "label": "vk", "description": "sent by a friend", "url": "https://vk.com/x" }));
        // Missing, twice, not a number, not a field.
        for line in ["source:distrokid", "source:distrokid index:1", "source:distrokid album_uuid:x album_uuid:y", "source:distrokid album_uuid:x index:one", "source:distrokid uuid:x"] {
            assert!(one(line).is_err(), "{line}");
        }
        assert!(one("source:distrokid index:1").unwrap_err().contains("album_uuid:… (needed)"));

        // A link or a custom label, and fields after it.
        let r = one("source:https://drive.google.com/x description:sent by a friend").unwrap();
        let [Op::Source { data, .. }] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "label": "drive.google.com", "url": "https://drive.google.com/x", "description": "sent by a friend" }));
        let r = one("source:custom vk url:https://vk.com/x").unwrap();
        let [Op::Source { data, .. }] = r.ops.as_slice() else { panic!("{:?}", r.ops) };
        assert_eq!(data, &serde_json::json!({ "label": "vk", "url": "https://vk.com/x" }));
        assert!(one("source:distrokid 0b358691 index:1").is_err(), "a platform's value is its name alone");

        // The choices, as `artist:` → `role:` → `as:`: Tab adds the next field, needed first.
        let choices = |line: &str| {
            let parsed = parse::parse(line);
            let last = parsed.parts.last().unwrap();
            candidates(last, parsed.last_goes_with(), &ctx, &[], &[], &mut Known).into_iter().map(|c| (c.label, c.next)).collect::<Vec<_>>()
        };
        let next = |s: &str| Some(s.to_string());
        assert_eq!(choices("source:distr"), [("distrokid".to_string(), next("album_uuid:"))]);
        assert_eq!(choices("source:=s"), [("=soundcloud".to_string(), next("song_id:"))]);
        assert_eq!(choices("source:cus"), [("custom".to_string(), None)]);
        assert_eq!(choices("source:distrokid album_uuid:"), [("album_uuid:…".to_string(), None)]);
        assert_eq!(choices("source:distrokid album_uuid:0b358691"), [("album_uuid:0b358691".to_string(), next("index:"))]);
        assert_eq!(choices("source:distrokid album_uuid:0b358691 index:3"), [("index:3".to_string(), None)]);
        assert_eq!(choices("source:distrokid index:3"), [("index:3".to_string(), next("album_uuid:"))]);
        assert_eq!(choices("source:distrokid uuid:x"), [("album_uuid:… (needed)".to_string(), None), ("index:…".to_string(), None)]);
    }

    #[test]
    fn new_groupings_and_their_kind() {
        let mut names = Names::default();
        let early = names.new_grouping("Early Days");
        let units = run("group:early days kind:era", &names, 2);
        assert_eq!(units[0].clone().unwrap().0, [Op::Group(early)]);
        // An existing one's kind isn't changed here; a kind is lowercase letters.
        assert!(run("group:the crew kind:era", &names, 1)[0].is_err());
        assert!(run("group:early days kind:Big Era", &names, 1)[0].is_err());
        // Nothing is in a new one yet.
        assert!(run("-group:early days", &names, 1)[0].is_err());
        let (catalog, vocab) = (catalog(), vocab());
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 1, context_album: None, staged_album: None, names: &names, may_relate: true, may_source: true, titles: None, artist_names: None };
        let (units, _) = resolve("group:early days kind:project", &ctx, &mut Known);
        assert_eq!(units[0].result.as_ref().unwrap().new_kind, Some((early, "project".to_string())));
    }

    #[test]
    fn units_keep_their_text_to_type_back() {
        let (catalog, vocab) = (catalog(), vocab());
        let names = Names::default();
        let ctx = Ctx { catalog: &catalog, vocab: &vocab, targets: 1, context_album: None, staged_album: None, names: &names, may_relate: true, may_source: true, titles: None, artist_names: None };
        let (units, _) = resolve("status:demo | artist:Bo role:featured #2", &ctx, &mut Known);
        let texts: Vec<&str> = units.iter().map(|u| u.text.as_str()).collect();
        assert_eq!(texts, ["status:demo", "artist:Bo role:featured #2"]);
    }
}
