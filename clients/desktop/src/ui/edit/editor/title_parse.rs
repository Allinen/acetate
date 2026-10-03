//! `parse:` in the editor: the credits in the targets' titles found with the rules typed
//! (`crate::title_credits`), and what each name found credits.
//!
//! One chip for every target, but each track gets its own result, so the change it stages
//! (`TitlesParsed`) keeps a result per title: each title the targets have (as the edit
//! leaves it before this) → the title it becomes and its credits. Applying looks up the
//! track's title as it is then: a title that isn't there changed since (another chip taken
//! back, someone else's edit), and that track is refused, not parsed again with whatever
//! the library says now.
//!
//! Names are matched to artists exactly, ignoring case (an artist's name or alias), not
//! the line's "artist like": one artist has it → that one (under the alias, if it's an
//! alias); a name chosen in the list for it (`Names::pin`) → that one; none → a new artist,
//! made when the chip is staged; more than one → not decided, and the tracks with it are
//! refused until one is chosen.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use uuid::Uuid;

use crate::title_credits::{self, NameTable, Setting, Warning};

/// What a name found credits.
#[derive(Debug, Clone, PartialEq)]
pub enum Credited {
    /// An artist; `alias`: the name is one of its aliases (credited under it).
    Artist { id: Uuid, alias: Option<Uuid> },
    /// A new artist (its stand-in, once staged).
    New(Option<Uuid>),
    /// More than one artist has the name: one has to be chosen.
    Undecided(Vec<Uuid>),
}

/// A name found in the titles (the same name in any case is one).
#[derive(Debug, Clone, PartialEq)]
pub struct Name {
    /// As written first.
    pub text: String,
    pub credited: Credited,
    /// How many of the targets have it.
    pub tracks: usize,
    /// Kept whole (an artist's name with a separator in it).
    pub whole: bool,
}

/// What one title becomes.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub title: String,
    /// (Its name, in `TitlesParsed::names`; the role.)
    pub credits: Vec<(usize, String)>,
    pub warnings: Vec<Warning>,
    /// Why a track with it can't be changed (the title would be empty).
    pub problem: Option<String>,
}

impl Outcome {
    fn changes(&self, before: &str) -> bool {
        self.title != before || !self.credits.is_empty()
    }
}

/// `parse:` staged: what each title becomes (see the module's notes).
#[derive(Clone)]
pub struct TitlesParsed {
    /// The setting, as it reads back (`feat prod [producer]`).
    pub setting: String,
    /// Each title (before this) → what it becomes.
    pub outcomes: HashMap<String, Outcome>,
    pub names: Vec<Name>,
    /// Tracks it leaves alone.
    pub skip: Vec<Uuid>,
    /// Sums up all of it: the edit is planned again when it changes (and it's what its
    /// `Debug` says, short, since that's hashed every frame).
    key: u64,
}

impl std::fmt::Debug for TitlesParsed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "TitlesParsed({}, {:x})", self.setting, self.key)
    }
}

impl PartialEq for TitlesParsed {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

/// What a track's title becomes, and the credits it gets: (artist, role, the alias it's
/// credited under if any).
pub type Becomes = (String, Vec<(Uuid, String, Option<Uuid>)>);

impl TitlesParsed {
    /// Every title of `titles` parsed with `setting`; `credited` says what a name credits
    /// (None: still looking it up, then so is this).
    pub fn new(setting: &Setting, titles: &[(Uuid, String)], table: &NameTable, credited: &mut dyn FnMut(&str) -> Option<Credited>) -> Option<TitlesParsed> {
        let mut outcomes: HashMap<String, Outcome> = HashMap::new();
        let mut names: Vec<Name> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (_, title) in titles {
            if outcomes.contains_key(title) {
                continue;
            }
            let parsed = title_credits::parse(title, setting, table);
            let credits = parsed
                .credits
                .iter()
                .map(|c| {
                    let i = *index.entry(c.name.to_lowercase()).or_insert_with(|| {
                        names.push(Name { text: c.name.clone(), credited: Credited::New(None), tracks: 0, whole: c.whole });
                        names.len() - 1
                    });
                    (i, c.role.clone())
                })
                .collect();
            let problem = parsed.title.trim().is_empty().then(|| "its title would be empty".to_string());
            outcomes.insert(title.clone(), Outcome { title: parsed.title, credits, warnings: parsed.warnings, problem });
        }
        for (_, title) in titles {
            let mut seen: Vec<usize> = outcomes[title].credits.iter().map(|c| c.0).collect();
            seen.sort();
            seen.dedup();
            for i in seen {
                names[i].tracks += 1;
            }
        }
        for name in &mut names {
            name.credited = credited(&name.text)?;
        }
        let mut parsed = TitlesParsed { setting: setting.to_text(), outcomes, names, skip: Vec::new(), key: 0 };
        parsed.seal();
        Some(parsed)
    }

    /// The key again, after a change.
    pub fn seal(&mut self) {
        let mut outcomes: Vec<(&String, &Outcome)> = self.outcomes.iter().collect();
        outcomes.sort_by_key(|o| o.0);
        let mut h = std::hash::DefaultHasher::new();
        format!("{}{outcomes:?}{:?}{:?}", self.setting, self.names, self.skip).hash(&mut h);
        self.key = h.finish();
    }

    /// What `track`'s title (as it is when this applies) becomes: None, left alone; Err,
    /// why it can't be.
    pub fn result(&self, track: Uuid, title: &str) -> Result<Option<Becomes>, String> {
        if self.skip.contains(&track) {
            return Ok(None);
        }
        let Some(o) = self.outcomes.get(title) else {
            return Err("its title changed since parse: was staged (stage it again)".into());
        };
        if let Some(problem) = &o.problem {
            return Err(problem.clone());
        }
        let mut credits = Vec::new();
        let mut undecided = Vec::new();
        for (i, role) in &o.credits {
            let name = &self.names[*i];
            match &name.credited {
                Credited::Artist { id, alias } => credits.push((*id, role.clone(), *alias)),
                Credited::New(Some(id)) => credits.push((*id, role.clone(), None)),
                Credited::New(None) => return Err(format!("“{}” isn't made yet (stage parse: again)", name.text)),
                Credited::Undecided(_) => undecided.push(format!("“{}”", name.text)),
            }
        }
        if !undecided.is_empty() {
            return Err(format!("{}: more than one artist has that name; choose which first", undecided.join(", ")));
        }
        Ok(Some((o.title.clone(), credits)))
    }

    /// How many of `titles` it changes.
    pub fn changing(&self, titles: &[(Uuid, String)]) -> usize {
        titles.iter().filter(|(t, title)| !self.skip.contains(t) && self.outcomes.get(title).is_some_and(|o| o.changes(title))).count()
    }

    /// How many of `titles` have something to look at (warnings, a problem).
    pub fn to_check(&self, titles: &[(Uuid, String)]) -> usize {
        titles.iter().filter(|(_, title)| self.outcomes.get(title).is_some_and(|o| o.problem.is_some() || !o.warnings.is_empty())).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn setting(text: &str) -> Setting {
        Setting::parse(text, &|t| ["primary", "featured", "producer"].iter().find(|r| r.starts_with(t)).map(|r| r.to_string())).unwrap()
    }

    /// Ann is artist 1, "Bo" is two artists (2, 3); anyone else is new.
    fn credited(name: &str) -> Option<Credited> {
        Some(match name.to_lowercase().as_str() {
            "ann" => Credited::Artist { id: id(1), alias: None },
            "bo" => Credited::Undecided(vec![id(2), id(3)]),
            _ => Credited::New(None),
        })
    }

    fn titles(list: &[&str]) -> Vec<(Uuid, String)> {
        list.iter().enumerate().map(|(i, t)| (id(100 + i as u128), t.to_string())).collect()
    }

    #[test]
    fn each_title_gets_its_own_result() {
        let t = titles(&["Song (feat. Ann)", "Other [prod. Cy]", "Plain", "Song (feat. Ann)"]);
        let mut p = TitlesParsed::new(&setting("feat prod"), &t, &NameTable::default(), &mut credited).unwrap();
        assert_eq!(p.outcomes.len(), 3, "the same title once");
        assert_eq!(p.changing(&t), 3);
        assert_eq!(p.names.iter().map(|n| (n.text.as_str(), n.tracks)).collect::<Vec<_>>(), [("Ann", 2), ("Cy", 1)]);
        assert_eq!(p.result(t[0].0, "Song (feat. Ann)"), Ok(Some(("Song".into(), vec![(id(1), "featured".into(), None)]))));
        // A new artist has to be made first (staging does it).
        assert!(p.result(t[1].0, "Other [prod. Cy]").is_err());
        p.names[1].credited = Credited::New(Some(id(9)));
        p.seal();
        assert_eq!(p.result(t[1].0, "Other [prod. Cy]"), Ok(Some(("Other".into(), vec![(id(9), "producer".into(), None)]))));
        // Nothing found: the same title, no credits.
        assert_eq!(p.result(t[2].0, "Plain"), Ok(Some(("Plain".into(), vec![]))));
    }

    #[test]
    fn refused_when_it_cant_say() {
        let t = titles(&["Song (feat. Bo)", "(feat. Ann)"]);
        let p = TitlesParsed::new(&setting("feat"), &t, &NameTable::default(), &mut credited).unwrap();
        assert!(p.result(t[0].0, "Song (feat. Bo)").unwrap_err().contains("more than one artist"));
        assert!(p.result(t[1].0, "(feat. Ann)").unwrap_err().contains("empty"));
        assert!(p.result(t[0].0, "Song (feat. Bo) [v2]").unwrap_err().contains("changed since"));
        let mut skipped = p.clone();
        skipped.skip.push(t[0].0);
        skipped.seal();
        assert_eq!(skipped.result(t[0].0, "Song (feat. Bo)"), Ok(None));
        assert_ne!(skipped, p, "a change makes another key");
    }

    #[test]
    fn still_looking_up_is_nothing_yet() {
        let t = titles(&["Song (feat. Ann)"]);
        assert!(TitlesParsed::new(&setting("feat"), &t, &NameTable::default(), &mut |_| None).is_none());
    }
}
