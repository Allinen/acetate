//! The commandline of "Credited as…" (`credited_as`): the choices the popup has as chips,
//! typed. The language is `crate::commandline`'s.
//!
//! * `artist:Name` the artist whose credits change (the artists credited on the tracks are
//!   listed),
//! * `only:all`, `only:own` or `only:Alias` which of its credits (all, those under its own
//!   name, those under one of its other names),
//! * `to:Name` credit them to another artist instead (`to:` and `to:same` the same one),
//! * `as:own` or `as:Alias` the name they're credited under,
//! * `why:` kept with every change.
//!
//! Enter sets what was typed; Ctrl+Enter credits the tracks so.

use uuid::Uuid;

use super::credited_as::Only;
use super::page_commands::{self as pc, Choice, Known, Pinned};
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::commandline::{self, Keys};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Artist,
    Only,
    To,
    As,
    Why,
}

const KEYS: &[(K, &[&str])] = &[
    (K::Artist, &["artist", "from"]),
    (K::Only, &["only", "under", "credits"]),
    (K::To, &["to", "instead"]),
    (K::As, &["as", "alias"]),
    (K::Why, &["why", "reason"]),
];

impl Keys for K {
    fn key(name: &str) -> Option<K> {
        let name = name.to_lowercase();
        KEYS.iter().find(|(_, names)| names.contains(&name.as_str())).map(|(k, _)| *k)
    }

    fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }

    fn takes(main: K, arg: K) -> bool {
        main != K::Why && main != arg
    }
}

/// One thing the line sets.
#[derive(Debug, Clone, PartialEq)]
pub enum Setting {
    Artist(Uuid),
    Only(Only),
    /// Another artist (its id and name); None: the same one.
    To(Option<(Uuid, String)>),
    /// The name they're credited under: an alias; None: the artist's own name.
    As(Option<Uuid>),
    Why(String),
}

/// What the popup knows, to understand the line.
pub struct World<'a> {
    pub catalog: &'a Catalog,
    /// The artists credited on the tracks.
    pub artists: &'a [(Uuid, String)],
    /// The artist chosen, and its other names.
    pub from: Option<Uuid>,
    pub from_names: &'a [(Uuid, String)],
    /// The artist they'd be credited to (the same, or another), its name and other names.
    pub to_name: Option<&'a str>,
    pub to_names: &'a [(Uuid, String)],
}

/// The one in `list` called `typed`: exactly (ignoring case), else the only one that
/// contains it.
fn find(list: &[(Uuid, String)], typed: &str) -> Result<Uuid, String> {
    let typed = typed.trim().to_lowercase();
    if let Some((id, _)) = list.iter().find(|(_, n)| n.to_lowercase() == typed) {
        return Ok(*id);
    }
    let mut like = list.iter().filter(|(_, n)| n.to_lowercase().contains(&typed));
    match (like.next(), like.next()) {
        (Some((id, _)), None) if !typed.is_empty() => Ok(*id),
        (Some(_), Some(_)) => Err(format!("more than one is like “{}”: choose in the list", typed)),
        _ => Err(format!("none is called “{}”", typed)),
    }
}

/// The artist in the library a `to:` means: one chosen in the list, else the one with
/// exactly that name.
fn artist_to(world: &World, known: &Known, typed: &str) -> Result<(Uuid, String), String> {
    if let Some(id) = known.pinned(Pinned::Artist, typed)
        && let Some(a) = world.catalog.artist(id)
    {
        return Ok((id, a.name.clone()));
    }
    let named: Vec<&crate::catalog::model::Artist> = world.catalog.artists.iter().filter(|a| a.name.eq_ignore_ascii_case(typed.trim())).collect();
    match named.as_slice() {
        [a] => Ok((a.id, a.name.clone())),
        [] => Err(format!("no artist called “{}”", typed.trim())),
        _ => Err(format!("{} artists are called “{}”: choose one in the list", named.len(), typed.trim())),
    }
}

/// What the line sets, part by part (in the order typed), each with its words or why not.
pub fn resolve(text: &str, world: &World, known: &Known) -> Vec<Result<(String, Setting), String>> {
    let line = commandline::parse::<K>(text);
    let mut out = Vec::new();
    let mut moved = false;
    for part in &line.parts {
        let value = part.value.trim();
        out.push(match part.key {
            K::Artist => {
                let pinned = known.pinned(Pinned::Artist, value).filter(|id| world.artists.iter().any(|(a, _)| a == id));
                pinned.ok_or(()).or_else(|()| find(world.artists, value)).map(|id| {
                    let name = world.artists.iter().find(|(a, _)| *a == id).map(|(_, n)| n.as_str()).unwrap_or("");
                    (format!("the credits of {name}"), Setting::Artist(id))
                })
            }
            K::Only if world.from.is_none() => Err("choose the artist first (artist:)".to_string()),
            K::Only => match value.to_lowercase().as_str() {
                "" => Err("only: all, own, or one of its other names".into()),
                "all" => Ok(("under any of its names".to_string(), Setting::Only(Only::All))),
                "own" | "own name" => Ok(("under its own name".to_string(), Setting::Only(Only::OwnName))),
                _ => find(world.from_names, value).map(|id| {
                    let name = world.from_names.iter().find(|(a, _)| *a == id).map(|(_, n)| n.as_str()).unwrap_or("");
                    (format!("those credited as “{name}”"), Setting::Only(Only::Alias(id)))
                }),
            },
            K::To if world.from.is_none() => Err("choose the artist first (artist:)".to_string()),
            K::To if value.is_empty() || value.eq_ignore_ascii_case("same") => {
                moved = true;
                Ok(("credited to the same artist".to_string(), Setting::To(None)))
            }
            K::To => artist_to(world, known, value).map(|(id, name)| {
                moved = true;
                if Some(id) == world.from {
                    ("credited to the same artist".to_string(), Setting::To(None))
                } else {
                    (format!("credited to {name} instead"), Setting::To(Some((id, name))))
                }
            }),
            K::As if world.from.is_none() => Err("choose the artist first (artist:)".to_string()),
            // (The other artist's names are looked up once it's set: not in the same line.)
            K::As if moved => Err("as: goes after to: has been set (Enter), as its names load first".to_string()),
            K::As => {
                let own = world.to_name.unwrap_or("");
                match value.to_lowercase().as_str() {
                    "" => Err("as: own, or one of its other names".to_string()),
                    "own" | "own name" => Ok(("as its own name".to_string(), Setting::As(None))),
                    v if v == own.to_lowercase() => Ok(("as its own name".to_string(), Setting::As(None))),
                    _ => find(world.to_names, value).map(|id| {
                        let name = world.to_names.iter().find(|(a, _)| *a == id).map(|(_, n)| n.as_str()).unwrap_or("");
                        (format!("as “{name}”"), Setting::As(Some(id)))
                    }),
                }
            }
            K::Why => Ok((format!("why: {value}"), Setting::Why(value.to_string()))),
        });
    }
    out
}

type Row = pc::Row<(), ()>;

fn row(label: impl Into<String>, sub: &str, choice: Choice<(), ()>) -> Row {
    Row { label: label.into(), sub: sub.into(), choice }
}

/// The words in `names` that start like `typed` (or contain it), as rows.
fn names_rows(names: &[(Uuid, String)], typed: &str, sub: &str, next: Option<&str>) -> Vec<Row> {
    let typed = typed.to_lowercase();
    names
        .iter()
        .filter(|(_, n)| n.to_lowercase().contains(&typed))
        .map(|(_, n)| row(n.clone(), sub, Choice::Value(n.clone())))
        .map(|mut r| {
            if let (Some(next), Choice::Value(n)) = (next, &r.choice) {
                r.choice = Choice::Raw(commandline::quoted::<K>(n), Some(next.to_string()));
            }
            r
        })
        .collect()
}

/// The choices for the value being typed.
pub fn choices(part: &commandline::Part<K>, world: &World) -> Vec<Row> {
    let value = part.value.trim();
    let typed = value.to_lowercase();
    match part.key {
        K::Artist => world
            .artists
            .iter()
            .filter(|(_, n)| n.to_lowercase().contains(&typed))
            .map(|(id, n)| row(n.clone(), "credited on these tracks", Choice::Pin(Pinned::Artist, n.clone(), *id, Some("only:".into()))))
            .collect(),
        K::Only if world.from.is_some() => {
            let mut rows: Vec<Row> = [("all", "under any of its names"), ("own", "under its own name")]
                .into_iter()
                .filter(|(w, _)| w.starts_with(&typed))
                .map(|(w, sub)| row(w, sub, Choice::Raw(w.into(), Some("as:".into()))))
                .collect();
            rows.extend(names_rows(world.from_names, value, "credited as it, now", Some("as:")));
            rows
        }
        K::To if world.from.is_some() => {
            let mut rows = Vec::new();
            if typed.is_empty() || "same".starts_with(&typed) {
                rows.push(row("same", "the same artist", Choice::Raw("same".into(), Some("as:".into()))));
            }
            if !typed.is_empty() {
                for (i, _) in Search::default().run_part(world.catalog, value, SearchPart::Artists, 8) {
                    let (name, sub) = crate::ui::edit::inline_search::describe(world.catalog, SearchPart::Artists, i);
                    let a = &world.catalog.artists[i as usize];
                    rows.push(row(name.clone(), &sub, Choice::Pin(Pinned::Artist, name, a.id, None)));
                }
            }
            rows
        }
        K::As if world.from.is_some() => {
            let mut rows: Vec<Row> = Some(("own", "its own name")).into_iter().filter(|(w, _)| w.starts_with(&typed)).map(|(w, sub)| row(w, sub, Choice::Raw(w.into(), None))).collect();
            rows.extend(names_rows(world.to_names, value, "a name it goes by", None));
            rows
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// A library of two artists called Cy (so which is asked), and Ann.
    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": []},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"], "rows": []},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[id(1), "Ann", "", ["Annie"], 0, 0], [id(3), "Cy", "", [], 0, 0], [id(4), "Cy", "UK", [], 0, 0], [id(5), "Di", "", [], 0, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap())
    }

    #[test]
    fn the_line_sets_the_choices_in_order() {
        let catalog = catalog();
        let artists = [(id(1), "Ann".to_string()), (id(2), "Bo".to_string())];
        let names = [(id(10), "Annie".to_string()), (id(11), "A. Lee".to_string())];
        let world = World { catalog: &catalog, artists: &artists, from: Some(id(1)), from_names: &names, to_name: Some("Ann"), to_names: &names };
        let known = Known::default();
        let set = |text: &str| -> Vec<Result<Setting, String>> { resolve(text, &world, &known).into_iter().map(|r| r.map(|(_, s)| s)).collect() };

        assert_eq!(set("artist:bo"), [Ok(Setting::Artist(id(2)))]);
        assert_eq!(set("only:annie as:own why:they were hers"), [Ok(Setting::Only(Only::Alias(id(10)))), Ok(Setting::As(None)), Ok(Setting::Why("they were hers".into()))]);
        assert_eq!(set("only:own"), [Ok(Setting::Only(Only::OwnName))]);
        assert_eq!(set("only:all"), [Ok(Setting::Only(Only::All))]);
        assert_eq!(set("as:a. lee"), [Ok(Setting::As(Some(id(11))))]);
        assert_eq!(set("as:Ann"), [Ok(Setting::As(None))], "its own name, spelled out");
        assert_eq!(set("to:same"), [Ok(Setting::To(None))]);
        assert!(set("only:zed")[0].is_err());
        assert!(set("artist:nobody")[0].is_err());
        assert!(set("to:Nobody")[0].is_err(), "no such artist in the library");
        assert!(set("to:same as:Annie")[1].is_err(), "its names are looked up once it is set");
        assert_eq!(set("to:di"), [Ok(Setting::To(Some((id(5), "Di".into()))))], "exact name, any case");
        assert!(set("to:cy")[0].as_ref().is_err_and(|e| e.contains("2 artists")), "two are called that: the list chooses");
    }

    #[test]
    fn nothing_but_the_artist_before_the_artist() {
        let catalog = catalog();
        let artists = [(id(1), "Ann".to_string())];
        let world = World { catalog: &catalog, artists: &artists, from: None, from_names: &[], to_name: None, to_names: &[] };
        let known = Known::default();
        for text in ["only:all", "to:same", "as:own"] {
            assert!(resolve(text, &world, &known)[0].is_err(), "{text}");
        }
        assert!(resolve("artist:ann", &world, &known)[0].is_ok());
    }
}
