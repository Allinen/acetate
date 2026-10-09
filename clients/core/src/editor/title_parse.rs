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
//! alias); a name chosen for it (`Names::pin`: in the check, or the list of an `artist:`) →
//! that one; none → a new artist, made when the chip is staged; more than one → not
//! decided, and the tracks with it are refused (and applying waits) until one is chosen.
//!
//! It keeps what it was made from (the setting, the titles, the corrections by hand), so
//! the check can correct it and parse again (`again`).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use uuid::Uuid;

use crate::title_credits::{self, Fixes, NameTable, Setting, Warning};

/// What a name found credits.
#[derive(Debug, Clone, PartialEq)]
pub enum Credited {
    /// An artist; `alias`: the name is one of its aliases (credited under it).
    Artist { id: Uuid, alias: Option<Uuid> },
    /// A new artist (its stand-in, once staged).
    New(Option<Uuid>),
    /// More than one artist has the name: one has to be chosen.
    Undecided(Vec<Uuid>),
    /// Each track's own primary artists (`prod. himself`; the `self` rule).
    Own,
}

/// A name found in the titles (the same name in any case is one).
#[derive(Debug, Clone, PartialEq)]
pub struct Name {
    /// As written first.
    pub text: String,
    pub credited: Credited,
    /// How many of the targets have it.
    pub tracks: usize,
    /// Kept whole (an artist's name with a separator in it, or by hand).
    pub whole: bool,
    /// What looks wrong about it (the parser's warning), if anything.
    pub why: Option<&'static str>,
    /// The roles it's credited in.
    pub roles: Vec<String>,
    /// The text it was first found in, before splitting ("Ann, Bo & Cy").
    pub part: String,
}

/// A credit a title gets.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Its name, in `TitlesParsed::names`.
    pub name: usize,
    pub role: String,
}

/// What one title becomes.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub title: String,
    pub credits: Vec<Found>,
    pub warnings: Vec<Warning>,
    /// Why a track with it can't be changed (the title would be empty).
    pub problem: Option<String>,
}

impl Outcome {
    pub fn changes(&self, before: &str) -> bool {
        self.title != before || !self.credits.is_empty()
    }
}

/// `parse:` staged: what each title becomes (see the module's notes).
#[derive(Clone)]
pub struct TitlesParsed {
    /// The rules and overrides, and as they read back (`feat prod [producer]`).
    pub rules: Setting,
    pub setting: String,
    /// The targets' titles it was made for (as the edit left them before it).
    pub titles: Vec<(Uuid, String)>,
    /// Corrections by hand (the check).
    pub fixes: Fixes,
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
    /// Every title of `titles` parsed with `setting` and `fixes`; `credited` says what a
    /// name credits (None: still looking it up, then so is this).
    pub fn new(setting: &Setting, titles: Vec<(Uuid, String)>, fixes: Fixes, table: &NameTable, credited: &mut dyn FnMut(&str) -> Option<Credited>) -> Option<TitlesParsed> {
        let mut outcomes: HashMap<String, Outcome> = HashMap::new();
        let mut names: Vec<Name> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for (_, title) in &titles {
            if outcomes.contains_key(title) {
                continue;
            }
            let parsed = title_credits::parse(title, setting, table, &fixes);
            let mut credits = Vec::new();
            for c in &parsed.credits {
                let i = *index.entry(c.name.to_lowercase()).or_insert_with(|| {
                    let credited = if c.own { Credited::Own } else { Credited::New(None) };
                    names.push(Name { text: c.name.clone(), credited, tracks: 0, whole: c.whole, why: None, roles: Vec::new(), part: c.part.clone() });
                    names.len() - 1
                });
                if !names[i].roles.contains(&c.role) {
                    names[i].roles.push(c.role.clone());
                }
                credits.push(Found { name: i, role: c.role.clone() });
            }
            for w in &parsed.warnings {
                if let Warning::Name { name, why } = w
                    && let Some(&i) = index.get(&name.to_lowercase())
                {
                    names[i].why = Some(why);
                }
            }
            let problem = parsed.title.trim().is_empty().then(|| "its title would be empty".to_string());
            outcomes.insert(title.clone(), Outcome { title: parsed.title, credits, warnings: parsed.warnings, problem });
        }
        for (_, title) in &titles {
            let mut seen: Vec<usize> = outcomes[title].credits.iter().map(|c| c.name).collect();
            seen.sort();
            seen.dedup();
            for i in seen {
                names[i].tracks += 1;
            }
        }
        for name in names.iter_mut().filter(|n| n.credited != Credited::Own) {
            name.credited = credited(&name.text)?;
        }
        let mut parsed = TitlesParsed { rules: setting.clone(), setting: setting.to_text(), titles, fixes, outcomes, names, skip: Vec::new(), key: 0 };
        parsed.seal();
        Some(parsed)
    }

    /// Parsed again (the corrections changed, a name was chosen): the same titles and
    /// tracks left alone.
    pub fn again(&self, table: &NameTable, credited: &mut dyn FnMut(&str) -> Option<Credited>) -> Option<TitlesParsed> {
        let mut again = TitlesParsed::new(&self.rules, self.titles.clone(), self.fixes.clone(), table, credited)?;
        again.skip = self.skip.clone();
        again.seal();
        Some(again)
    }

    /// The new artists it credits made (`new_artist`: a name's stand-in).
    pub fn make_new(&mut self, new_artist: &mut dyn FnMut(&str) -> Uuid) {
        for n in &mut self.names {
            if n.credited == Credited::New(None) {
                n.credited = Credited::New(Some(new_artist(&n.text)));
            }
        }
        self.seal();
    }

    /// The key again, after a change.
    pub fn seal(&mut self) {
        let mut outcomes: Vec<(&String, &Outcome)> = self.outcomes.iter().collect();
        outcomes.sort_by_key(|o| o.0);
        let mut h = std::hash::DefaultHasher::new();
        format!("{}{outcomes:?}{:?}{:?}{:?}", self.setting, self.names, self.skip, self.fixes).hash(&mut h);
        self.key = h.finish();
    }

    /// What `track`'s title (as it is when this applies) becomes: None, left alone; Err,
    /// why it can't be. `primary`: its primary artists then (who `himself` is).
    pub fn result(&self, track: Uuid, title: &str, primary: &[Uuid]) -> Result<Option<Becomes>, String> {
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
        for c in &o.credits {
            let name = &self.names[c.name];
            match &name.credited {
                Credited::Artist { id, alias } => credits.push((*id, c.role.clone(), *alias)),
                Credited::New(Some(id)) => credits.push((*id, c.role.clone(), None)),
                Credited::New(None) => return Err(format!("“{}” isn't made yet (stage parse: again)", name.text)),
                Credited::Undecided(_) => undecided.push(format!("“{}”", name.text)),
                Credited::Own if primary.is_empty() => return Err(format!("“{}” is its own artist, and it has no primary artist", name.text)),
                Credited::Own => credits.extend(primary.iter().map(|id| (*id, c.role.clone(), None))),
            }
        }
        if !undecided.is_empty() {
            return Err(format!("{}: more than one artist has that name; choose which first", undecided.join(", ")));
        }
        Ok(Some((o.title.clone(), credits)))
    }

    /// How many of the titles it changes.
    pub fn changing(&self) -> usize {
        self.titles.iter().filter(|(t, title)| !self.skip.contains(t) && self.outcomes.get(title).is_some_and(|o| o.changes(title))).count()
    }

    /// How many of the titles have something to look at (warnings, a problem).
    pub fn to_check(&self) -> usize {
        self.titles.iter().filter(|(_, title)| self.outcomes.get(title).is_some_and(|o| o.problem.is_some() || !o.warnings.is_empty())).count()
    }

    /// The names still to choose an artist for, on tracks it doesn't leave alone.
    pub fn undecided(&self) -> Vec<&Name> {
        let mut out: Vec<usize> = Vec::new();
        for (t, title) in &self.titles {
            if self.skip.contains(t) {
                continue;
            }
            for c in self.outcomes.get(title).map(|o| o.credits.as_slice()).unwrap_or_default() {
                if matches!(self.names[c.name].credited, Credited::Undecided(_)) && !out.contains(&c.name) {
                    out.push(c.name);
                }
            }
        }
        out.into_iter().map(|i| &self.names[i]).collect()
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
        let mut p = TitlesParsed::new(&setting("feat prod"), t.clone(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert_eq!(p.outcomes.len(), 3, "the same title once");
        assert_eq!(p.changing(), 3);
        assert_eq!(p.names.iter().map(|n| (n.text.as_str(), n.tracks)).collect::<Vec<_>>(), [("Ann", 2), ("Cy", 1)]);
        assert_eq!(p.result(t[0].0, "Song (feat. Ann)", &[]), Ok(Some(("Song".into(), vec![(id(1), "featured".into(), None)]))));
        // A new artist has to be made first (staging does it).
        assert!(p.result(t[1].0, "Other [prod. Cy]", &[]).is_err());
        p.names[1].credited = Credited::New(Some(id(9)));
        p.seal();
        assert_eq!(p.result(t[1].0, "Other [prod. Cy]", &[]), Ok(Some(("Other".into(), vec![(id(9), "producer".into(), None)]))));
        // Nothing found: the same title, no credits.
        assert_eq!(p.result(t[2].0, "Plain", &[]), Ok(Some(("Plain".into(), vec![]))));
    }

    #[test]
    fn refused_when_it_cant_say() {
        let t = titles(&["Song (feat. Bo)", "(feat. Ann)"]);
        let p = TitlesParsed::new(&setting("feat"), t.clone(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert!(p.result(t[0].0, "Song (feat. Bo)", &[]).unwrap_err().contains("more than one artist"));
        assert!(p.result(t[1].0, "(feat. Ann)", &[]).unwrap_err().contains("empty"));
        assert!(p.result(t[0].0, "Song (feat. Bo) [v2]", &[]).unwrap_err().contains("changed since"));
        let mut skipped = p.clone();
        skipped.skip.push(t[0].0);
        skipped.seal();
        assert_eq!(skipped.result(t[0].0, "Song (feat. Bo)", &[]), Ok(None));
        assert_ne!(skipped, p, "a change makes another key");
    }

    #[test]
    fn parsed_again_with_corrections() {
        let t = titles(&["Cool song! [bong]", "Song (feat. Bo)"]);
        let p = TitlesParsed::new(&setting("feat [prod]"), t.clone(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert_eq!(p.undecided().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Bo"]);
        let mut fixed = p.clone();
        fixed.fixes.not_names.push("bong".into());
        fixed.skip.push(t[1].0);
        let again = fixed.again(&NameTable::default(), &mut credited).unwrap();
        assert_eq!(again.outcomes["Cool song! [bong]"].title, "Cool song! [bong]");
        assert_eq!(again.names.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["Bo"]);
        assert!(again.undecided().is_empty(), "its track is left alone");
        assert_eq!(again.changing(), 0);
    }

    #[test]
    fn himself_is_each_tracks_own_artist() {
        let t = titles(&["Song | PROD. BY HIMSELF", "Other (self-prod.)", "Third (prod. me)"]);
        let p = TitlesParsed::new(&setting("prod self"), t.clone(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert!(p.names.iter().all(|n| n.credited == Credited::Own), "{:?}", p.names);
        assert_eq!(p.result(t[0].0, &t[0].1, &[id(7)]), Ok(Some(("Song".into(), vec![(id(7), "producer".into(), None)]))));
        assert_eq!(p.result(t[1].0, &t[1].1, &[id(8), id(9)]), Ok(Some(("Other".into(), vec![(id(8), "producer".into(), None), (id(9), "producer".into(), None)]))));
        assert!(p.result(t[2].0, &t[2].1, &[]).unwrap_err().contains("no primary artist"));
        // Without the rule, it's a name like any (and the check says what it may mean).
        let p = TitlesParsed::new(&setting("prod"), t[..1].to_vec(), Fixes::default(), &NameTable::default(), &mut credited).unwrap();
        assert_eq!((p.names[0].credited.clone(), p.names[0].why), (Credited::New(None), Some("means the track's own artist? (the self rule)")));
    }

    #[test]
    fn still_looking_up_is_nothing_yet() {
        let t = titles(&["Song (feat. Ann)"]);
        assert!(TitlesParsed::new(&setting("feat"), t.clone(), Fixes::default(), &NameTable::default(), &mut |_| None).is_none());
    }
}
