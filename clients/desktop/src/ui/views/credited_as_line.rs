//! The commandline of "Credited as…" (`credited_as`), declared once (`SCHEMA`, the
//! language is `crate::commandline::schema`'s). A command per artist whose credits change,
//! several at once apart by `|`:
//!
//! * `artist:Name` the artist (those credited on the tracks are listed), then in any order:
//! * `only:all`, `only:own` or `only:Alias` which of its credits (all, those under its own
//!   name, those under one of its other names);
//! * `to:Name` credit them to another artist instead (`to:same`: the same one);
//! * `as:own` or `as:Alias` the name they're credited under: one of the names of the artist
//!   in `to:`, else of the artist's own. One of `to:` and `as:` is needed;
//! * `why:` kept with every change.
//!
//! `artist:Ann only:Annie as:own | artist:Bo to:Cy` credits both. The line is what the popup
//! does: its chips write into the line, and Ctrl+Enter credits the tracks so.

use std::collections::HashMap;

use uuid::Uuid;

use super::credited_as::Only;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::commandline::schema::{self, Kind, Lists, Need, Next, Node, Pins, Row, Schema, Spec, Word};

const ONLY: &[Word] = &[Word { word: "all", help: "under any of its names" }, Word { word: "own", help: "under its own name" }];
const SAME: &[Word] = &[Word { word: "same", help: "the same artist" }];
const OWN: &[Word] = &[Word { word: "own", help: "its own name" }];

const ARGS: &[Spec] = &[
    Spec::new("only", Kind::Or(ONLY, &Kind::AliasOf(&[])), "which of its credits: all, own, or one of its other names").aliases(&["under", "credits"]),
    Spec::new("to", Kind::Or(SAME, &Kind::Artist), "credit them to another artist instead").aliases(&["instead"]).need(Need::OneOf("what")),
    Spec::new("as", Kind::Or(OWN, &Kind::AliasOf(&["to"])), "the name they're credited under: own, or an other name").aliases(&["alias"]).need(Need::OneOf("what")),
    Spec::new("why", Kind::Text, "kept with every change").aliases(&["reason"]),
];

pub static SCHEMA: Schema = Schema { commands: &[Spec::new("artist", Kind::Artist, "whose credits change").aliases(&["from"]).next(Next::Args(ARGS))] };

/// What a command asks for: the artist, which of its credits, and the artist and name they
/// become (alias None: that artist's own name).
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub from: Uuid,
    pub only: Only,
    pub to: Uuid,
    pub alias: Option<Uuid>,
    pub why: String,
}

/// What the popup knows, to understand the line.
pub struct World<'a> {
    pub catalog: &'a Catalog,
    /// The artists credited on the tracks.
    pub artists: &'a [(Uuid, String)],
    /// Artists' other names, for those loaded.
    pub names: &'a HashMap<Uuid, Vec<(Uuid, String)>>,
    pub pins: &'a Pins,
}

/// The one in `list` called `typed`: exactly (ignoring case), else the only one that
/// contains it.
fn find(list: &[(Uuid, String)], typed: &str, what: &str) -> Result<Uuid, String> {
    let typed = typed.trim().to_lowercase();
    if let Some((id, _)) = list.iter().find(|(_, n)| n.to_lowercase() == typed) {
        return Ok(*id);
    }
    let mut like = list.iter().filter(|(_, n)| n.to_lowercase().contains(&typed));
    match (like.next(), like.next()) {
        (Some((id, _)), None) if !typed.is_empty() => Ok(*id),
        (Some(_), Some(_)) => Err(format!("more than one of {what} is like “{typed}”: choose in the list")),
        _ => Err(format!("none of {what} is “{typed}”")),
    }
}

/// What each part of a command means, or why it can't (None: not typed).
pub struct Parts {
    pub from: Result<(Uuid, String), String>,
    pub only: Option<Result<Only, String>>,
    /// The artist credited to: None, the same one.
    pub to: Option<Result<Option<(Uuid, String)>, String>>,
    pub alias: Option<Result<Option<Uuid>, String>>,
    pub why: String,
}

impl World<'_> {
    fn artist_name(&self, id: Uuid) -> String {
        self.artists.iter().find(|(a, _)| *a == id).map(|(_, n)| n.clone()).or_else(|| self.catalog.artist_label(id)).unwrap_or_default()
    }

    /// The artist a command is about: one chosen in the list, else one of those on the tracks.
    fn from(&self, node: &Node) -> Result<(Uuid, String), String> {
        let value = node.value.trim();
        let pinned = self.pins.get(Kind::Artist, value).filter(|id| self.artists.iter().any(|(a, _)| a == id));
        let id = match pinned {
            Some(id) => id,
            None => find(self.artists, value, "the artists on these tracks")?,
        };
        Ok((id, self.artist_name(id)))
    }

    /// The artist in the library a `to:` means: one chosen in the list, else the one with
    /// exactly that name. None: the same one.
    fn to(&self, node: &Node, from: Option<Uuid>) -> Result<Option<(Uuid, String)>, String> {
        let typed = node.value.trim();
        if node.spec.value.word(typed).is_some() {
            return Ok(None);
        }
        let found = if let Some(id) = self.pins.get(Kind::Artist, typed)
            && let Some(a) = self.catalog.artist(id)
        {
            (id, a.name.clone())
        } else {
            let named: Vec<&crate::catalog::model::Artist> = self.catalog.artists.iter().filter(|a| a.name.eq_ignore_ascii_case(typed)).collect();
            match named.as_slice() {
                [a] => (a.id, a.name.clone()),
                [] => return Err(format!("no artist called “{typed}”")),
                _ => return Err(format!("{} artists are called “{typed}”: choose one in the list", named.len())),
            }
        };
        Ok((Some(found.0) != from).then_some(found))
    }

    /// One of `artist`'s other names, as typed (one chosen in the list, if it's that
    /// artist's).
    fn alias(&self, typed: &str, artist: Uuid) -> Result<Uuid, String> {
        let name = self.artist_name(artist);
        let Some(names) = self.names.get(&artist) else { return Err(format!("{name}'s names are still loading")) };
        self.pinned_alias(typed, artist).map_or_else(|| find(names, typed, &format!("{name}'s names")), Ok)
    }

    /// One of `artist`'s other names chosen in the list as `typed` (it means that one, even
    /// when it's called like a word: an alias called “own”).
    fn pinned_alias(&self, typed: &str, artist: Uuid) -> Option<Uuid> {
        let names = self.names.get(&artist)?;
        self.pins.get(Kind::AliasOf(&[]), typed).filter(|id| names.iter().any(|(a, _)| a == id))
    }

    /// The artist whose names an `AliasOf` node's choices are: `to:`'s when it's another
    /// artist, else the command's.
    fn whose(&self, kind: Kind, command: &Node) -> Option<Uuid> {
        let from = self.from(command).ok().map(|f| f.0);
        match kind.artist_node(command) {
            Some(n) if !std::ptr::eq(n, command) => self.to(n, from).ok().map(|to| to.map(|t| t.0)).unwrap_or(None).or(from),
            _ => from,
        }
    }

    pub fn parts(&self, command: &Node) -> Parts {
        let from = self.from(command);
        let from_id = from.as_ref().ok().map(|f| f.0);
        let to = command.arg("to").map(|n| self.to(n, from_id));
        let only = command.arg("only").map(|n| {
            let from = from_id.ok_or("choose the artist first")?;
            if let Some(id) = self.pinned_alias(&n.value, from) {
                return Ok(Only::Alias(id));
            }
            match n.spec.value.word(&n.value) {
                Some("all") => Ok(Only::All),
                Some(_) => Ok(Only::OwnName),
                None => self.alias(&n.value, from).map(Only::Alias),
            }
        });
        let alias = command.arg("as").map(|n| {
            let whose = match &to {
                Some(Ok(Some((id, _)))) => *id,
                Some(Err(_)) => return Err("as: needs to: to be an artist".to_string()),
                _ => from_id.ok_or("choose the artist first")?,
            };
            let typed = n.value.trim();
            if let Some(id) = self.pinned_alias(typed, whose) {
                return Ok(Some(id));
            }
            if n.spec.value.word(typed).is_some() || typed.eq_ignore_ascii_case(&self.artist_name(whose)) {
                return Ok(None);
            }
            self.alias(typed, whose).map(Some)
        });
        let why = command.arg("why").map(|n| n.value.clone()).unwrap_or_default();
        Parts { from, only, to, alias, why }
    }

    /// What a command does, in words, or everything wrong with it.
    pub fn resolve(&self, command: &Node) -> Result<(String, Credit), String> {
        let mut wrong = schema::problems(command);
        let p = self.parts(command);
        fn why_not<T>(key: &str, r: Option<&Result<T, String>>) -> Option<String> {
            r?.as_ref().err().map(|e| format!("{key}: {e}"))
        }
        wrong.extend(why_not("artist", Some(&p.from)));
        wrong.extend(why_not("only", p.only.as_ref()));
        wrong.extend(why_not("to", p.to.as_ref()));
        wrong.extend(why_not("as", p.alias.as_ref()));
        if !wrong.is_empty() {
            return Err(wrong.join(" · "));
        }
        let (from, name) = p.from.unwrap();
        let only = p.only.map(Result::unwrap).unwrap_or(Only::All);
        let to = p.to.map(Result::unwrap).unwrap_or(None);
        let alias = p.alias.map(Result::unwrap).unwrap_or(None);
        let whose = to.as_ref().map(|t| t.0).unwrap_or(from);
        let names = |artist: Uuid, id: Uuid| self.names.get(&artist).and_then(|n| n.iter().find(|(a, _)| *a == id)).map(|(_, n)| n.clone()).unwrap_or_default();
        let which = match only {
            Only::All => format!("{name}'s credits"),
            Only::OwnName => format!("{name}'s credits under its own name"),
            Only::Alias(id) => format!("{name}'s credits as “{}”", names(from, id)),
        };
        let to_words = match &to {
            Some((_, other)) => format!(" credited to {other}"),
            None => String::new(),
        };
        let as_words = match alias {
            None => " under its own name".to_string(),
            Some(id) => format!(" as “{}”", names(whose, id)),
        };
        let credit = Credit { from, only, to: whose, alias, why: p.why };
        Ok((format!("{which}{to_words}{as_words}"), credit))
    }

    /// The artists the line names (on the tracks, and credited to), for their names to be
    /// loaded.
    pub fn named(&self, line: &schema::Line) -> Vec<Uuid> {
        let mut out = Vec::new();
        for c in &line.commands {
            let from = self.from(c).ok().map(|f| f.0);
            out.extend(from);
            if let Some(Ok(Some((id, _)))) = c.arg("to").map(|n| self.to(n, from)) {
                out.push(id);
            }
        }
        out
    }
}

impl Lists for World<'_> {
    fn choices(&self, kind: Kind, typed: &str, node: &Node, command: &Node) -> Vec<Row> {
        let lower = typed.to_lowercase();
        match kind {
            Kind::Artist if node.spec.key == "artist" => {
                self.artists.iter().filter(|(_, n)| n.to_lowercase().contains(&lower)).map(|(id, n)| Row::pin(n.clone(), "credited on these tracks", Kind::Artist, *id)).collect()
            }
            Kind::Artist if !typed.is_empty() => Search::default()
                .run_part(self.catalog, typed, SearchPart::Artists, 8)
                .into_iter()
                .map(|(i, _)| {
                    let (name, sub) = crate::catalog::search::describe(self.catalog, SearchPart::Artists, i);
                    Row::pin(name, sub, Kind::Artist, self.catalog.artists[i as usize].id)
                })
                .collect(),
            Kind::AliasOf(_) => {
                let Some(whose) = self.whose(kind, command) else { return Vec::new() };
                let sub = if node.spec.key == "only" { "credited as it, now" } else { "a name it goes by" };
                self.names.get(&whose).into_iter().flatten().filter(|(_, n)| n.to_lowercase().contains(&lower)).map(|(id, n)| Row::pin(n.clone(), sub, kind, *id)).collect()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commandline::schema::{parse, rows, take};

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// A library of two artists called Cy (so which is asked), Ann, Bo and Di.
    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": []},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"], "rows": []},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[id(1), "Ann", "", ["Annie"], 0, 0], [id(2), "Bo", "", ["Bobby"], 0, 0], [id(3), "Cy", "", [], 0, 0], [id(4), "Cy", "UK", [], 0, 0], [id(5), "Di", "", [], 0, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap())
    }

    struct Fixture {
        catalog: Catalog,
        artists: Vec<(Uuid, String)>,
        names: HashMap<Uuid, Vec<(Uuid, String)>>,
    }

    fn fixture() -> Fixture {
        let mut names = HashMap::new();
        names.insert(id(1), vec![(id(10), "Annie".to_string()), (id(11), "A. Lee".to_string())]);
        names.insert(id(2), vec![(id(20), "Bobby".to_string())]);
        names.insert(id(5), vec![]);
        Fixture { catalog: catalog(), artists: vec![(id(1), "Ann".into()), (id(2), "Bo".into())], names }
    }

    fn world<'a>(f: &'a Fixture, pins: &'a Pins) -> World<'a> {
        World { catalog: &f.catalog, artists: &f.artists, names: &f.names, pins }
    }

    fn credits(text: &str, w: &World) -> Vec<Result<Credit, String>> {
        parse(&SCHEMA, text).commands.iter().map(|c| w.resolve(c).map(|(_, c)| c)).collect()
    }

    fn labels(text: &str, w: &World) -> Vec<String> {
        rows(&SCHEMA, &parse(&SCHEMA, text), text, w).into_iter().map(|r| r.label).collect()
    }

    #[test]
    fn each_command_is_a_credit() {
        let f = fixture();
        let pins = Pins::default();
        let w = world(&f, &pins);
        let c = |from, only, to, alias| Ok(Credit { from: id(from), only, to: id(to), alias, why: String::new() });
        assert_eq!(credits("artist:ann only:annie as:own", &w), [c(1, Only::Alias(id(10)), 1, None)]);
        assert_eq!(credits("artist:Ann as:a. lee only:own", &w), [c(1, Only::OwnName, 1, Some(id(11)))], "any order");
        assert_eq!(credits("artist:Ann as:Ann", &w), [c(1, Only::All, 1, None)], "its own name, spelled out");
        assert_eq!(credits("artist:Ann to:same", &w), [c(1, Only::All, 1, None)]);
        assert_eq!(credits("artist:Ann to:di", &w), [c(1, Only::All, 5, None)], "exact name, any case");
        assert_eq!(credits("artist:Ann only:own as:Annie | artist:Bo to:Ann as:A. Lee", &w), [c(1, Only::OwnName, 1, Some(id(10))), c(2, Only::All, 1, Some(id(11)))], "several at once");
        assert_eq!(credits("artist:Ann as:own why:they were hers", &w)[0].as_ref().unwrap().why, "they were hers");
        assert!(credits("artist:Ann to:cy", &w)[0].as_ref().is_err_and(|e| e.contains("2 artists")), "two are called that: the list chooses");
        assert!(credits("artist:nobody as:own", &w)[0].is_err());
        assert!(credits("artist:Ann only:zed as:own", &w)[0].as_ref().is_err_and(|e| e.contains("Ann's names")));
    }

    #[test]
    fn what_is_missing_is_said() {
        let f = fixture();
        let pins = Pins::default();
        let w = world(&f, &pins);
        assert!(credits("artist:Ann only:own", &w)[0].as_ref().is_err_and(|e| e.contains("needs one of to:, as:")));
        // A name that no longer fits once `to:` changes: said, not dropped.
        assert!(credits("artist:Ann to:Bo as:Annie", &w)[0].as_ref().is_err_and(|e| e.contains("none of Bo's names is “annie”")));
        assert!(credits("artist:Ann to:same as:Bobby", &w)[0].as_ref().is_err_and(|e| e.contains("Ann's names")));
    }

    #[test]
    fn the_list_follows_the_line() {
        let f = fixture();
        let pins = Pins::default();
        let w = world(&f, &pins);
        assert_eq!(labels("", &w), ["artist:"]);
        assert_eq!(labels("artist:", &w), ["Ann", "Bo"]);
        // The artist is known before Enter: its names are listed at once.
        assert_eq!(labels("artist:Ann only:", &w), ["all", "own", "Annie", "A. Lee"]);
        assert_eq!(labels("artist:Ann only:own ", &w), ["to:", "as:", "why:"], "done: the keys left");
        assert_eq!(labels("artist:Ann |", &w), ["artist:"], "a bar: the commands again");
        assert_eq!(labels("artist:Ann to:Bo as:", &w), ["own", "Bobby"], "to: another: its names");
        assert_eq!(labels("artist:Ann to:same as:", &w), ["own", "Annie", "A. Lee"]);
        assert_eq!(labels("artist:Ann only:a. ", &w), ["A. Lee"], "a space inside a name: its choices go on");
        assert_eq!(labels("artist:Bo | artist:Ann as:", &w), ["own", "Annie", "A. Lee"], "each command its own artist");
    }

    #[test]
    fn tab_takes_and_leaves_a_space() {
        let f = fixture();
        let mut pins = Pins::default();
        let text = "artist:An";
        let row = rows(&SCHEMA, &parse(&SCHEMA, text), text, &world(&f, &Pins::default())).remove(0);
        assert_eq!(take(&SCHEMA, text, &row, true, &mut pins), "artist:Ann ");
        assert_eq!(pins.get(Kind::Artist, "Ann"), Some(id(1)));
    }

    #[test]
    fn an_alias_called_like_a_word_is_pinned() {
        let mut f = fixture();
        f.names.insert(id(1), vec![(id(12), "own".to_string())]);
        let unpinned = Pins::default();
        assert_eq!(credits("artist:Ann only:own as:own", &world(&f, &unpinned))[0].as_ref().unwrap().only, Only::OwnName, "typed: the word");
        let mut pins = Pins::default();
        pins.set(Kind::AliasOf(&[]), "own", id(12));
        let c = credits("artist:Ann only:own as:own", &world(&f, &pins)).remove(0).unwrap();
        assert_eq!((c.only, c.alias), (Only::Alias(id(12)), Some(id(12))), "chosen in the list: that name");
    }

    #[test]
    fn every_state_leads_somewhere_valid() {
        let f = fixture();
        let pins = Pins::default();
        let w = world(&f, &pins);
        schema::walk(&SCHEMA, &w, &|_| vec!["x".into()], 5).unwrap();
    }
}
