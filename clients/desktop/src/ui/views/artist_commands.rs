//! The artist page's commandline (a `ui::edit::commandline`; the language is
//! `crate::commandline`'s). It's there in listen mode too, for moving around and filtering.
//!
//! * `alias:Name` turns that name's chip on or off: live while it's typed, kept with Enter.
//!   `alias:all` goes back to all names. `og:` / `og:yes` / `og:no` sets the Original files
//!   chip. The chips are the state; the commandline only changes them, and Esc puts back
//!   what they were.
//! * `releases`, `tracks`, `featured`, `produced`, `extras` (a word, or `key:`) show that
//!   section on its own. `tracks:words` also sets its filter. `sources` and `notes` are
//!   opened and scrolled to.
//! * Plain words find these too, and `play`, plus `merge` and `collective` in edit mode.

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};

use super::artist::Section;
use crate::commandline::{self, Keys};
use crate::ui::edit::commandline::{self as cl, CommandLine};
use crate::ui::theme::PALETTE as P;
use crate::ui::{thousands, Shell};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Alias,
    Og,
    Section(Section),
    Sources,
    Notes,
}

/// Every key's names: its own first. Exact names only (no starts of them: `release:`
/// isn't anything else here).
const KEYS: &[(K, &[&str])] = &[
    (K::Alias, &["alias", "aliases"]),
    (K::Og, &["og", "original"]),
    (K::Section(Section::Releases), &["releases", "release"]),
    (K::Section(Section::Tracks), &["tracks", "track"]),
    (K::Section(Section::Featured), &["featured", "feat", "feature", "featuring"]),
    (K::Section(Section::Produced), &["produced", "producer", "prod"]),
    (K::Section(Section::Extras), &["extras"]),
    (K::Sources, &["sources"]),
    (K::Notes, &["notes"]),
];

impl Keys for K {
    fn key(name: &str) -> Option<K> {
        let name = name.to_lowercase();
        KEYS.iter().find(|(_, names)| names.contains(&name.as_str())).map(|(k, _)| *k)
    }

    fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }

    fn takes(_: K, _: K) -> bool {
        false
    }
}

/// What the page shows: the names it's credited under that are chosen (none: all of
/// them), and only original files or not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Filters {
    pub names: BTreeSet<Option<String>>,
    pub og_only: bool,
}

impl Filters {
    /// Whether a credit under this name (None: its own) is shown.
    pub fn shows(&self, name: Option<&str>) -> bool {
        self.names.is_empty() || self.names.iter().any(|n| n.as_deref() == name)
    }

    /// That name's chip, on or off.
    pub fn toggle(&mut self, name: Option<String>) {
        if !self.names.remove(&name) {
            self.names.insert(name);
        }
    }
}

/// What the page knows, for the commandline.
pub struct Here<'a> {
    /// The artist's own name.
    pub own: &'a str,
    /// The names it's credited under, with how many tracks (its own: None).
    pub names: &'a [(Option<String>, usize)],
    pub og_count: usize,
    /// The sections shown, with how many in each.
    pub sections: &'a [(Section, usize)],
    pub has_sources: bool,
    pub has_notes: bool,
    pub edit_mode: bool,
    /// "Turn into a collective…" is there (edit mode, `edit_sources`).
    pub may_collective: bool,
}

/// What the page does for a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// A section on its own (a track section's filter set to the text).
    Expand(Section, String),
    /// Opened and scrolled to.
    Reveal(Reveal),
    Merge,
    Collective,
    Play,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reveal {
    Sources,
    Notes,
}

/// What a command does.
#[derive(Debug, Clone, PartialEq)]
enum Does {
    /// A name's chip on or off.
    Name(Option<String>),
    AllNames,
    Og(bool),
    Act(Act),
}

/// A row of the list under the commandline: what it says, and what taking it does.
struct Row {
    label: String,
    sub: String,
    choice: Choice,
}

enum Choice {
    /// The value being typed becomes this.
    Value(String),
    /// The commandline becomes this (a key to go on typing after).
    Insert(String),
    Act(Act),
}

pub struct ArtistCommands {
    cl: CommandLine,
    notice: Option<String>,
}

impl Default for ArtistCommands {
    fn default() -> Self {
        ArtistCommands { cl: CommandLine::new(egui::Id::new("artist-commandline")), notice: None }
    }
}

/// How a name is shown: its own name for None.
fn shown<'a>(here: &'a Here, name: &'a Option<String>) -> &'a str {
    name.as_deref().unwrap_or(here.own)
}

/// The name typed, among those it's credited under: exactly, else the one that's the same
/// but for case (if only one is).
fn name_of(here: &Here, typed: &str) -> Option<Option<String>> {
    let typed = typed.trim();
    if let Some((n, _)) = here.names.iter().find(|(n, _)| shown(here, n) == typed) {
        return Some(n.clone());
    }
    let lower = typed.to_lowercase();
    let mut alike = here.names.iter().filter(|(n, _)| shown(here, n).to_lowercase() == lower);
    match (alike.next(), alike.next()) {
        (Some((n, _)), None) => Some(n.clone()),
        _ => None,
    }
}

/// Each command typed: its text, and what it does (or why it can't).
fn resolve(text: &str, here: &Here) -> Vec<Result<(String, Does), String>> {
    let line = commandline::parse::<K>(text);
    let mut out = Vec::new();
    for command in line.commands() {
        let main = &command[0];
        let value = main.value.trim();
        if main.minus {
            out.push(Err(format!("-{}: nothing to take off here", main.key.name())));
            continue;
        }
        out.push(match main.key {
            K::Alias if value.is_empty() => Err("alias: a name it's credited under (the list), or all".into()),
            K::Alias if value.eq_ignore_ascii_case("all") => Ok(("all names".into(), Does::AllNames)),
            K::Alias => match name_of(here, value) {
                Some(name) => Ok((format!("as {}: on or off", shown(here, &name)), Does::Name(name))),
                None => Err(format!("it isn't credited as “{value}” on any track")),
            },
            K::Og if here.og_count == 0 => Err("it has no original files".into()),
            K::Og => match value.to_lowercase().as_str() {
                "" | "yes" | "y" | "on" => Ok(("only original files".into(), Does::Og(true))),
                "no" | "n" | "off" => Ok(("all files, not only original ones".into(), Does::Og(false))),
                _ => Err("og: yes or no".into()),
            },
            K::Section(s) => match here.sections.iter().find(|(x, _)| *x == s) {
                None => Err(format!("no {} here", s.title().to_lowercase())),
                Some(_) if !value.is_empty() && matches!(s, Section::Releases | Section::Extras) => Err(format!("{}: nothing after it", main.key.name())),
                Some(_) if value.is_empty() => Ok((format!("{}, on its own", s.title()), Does::Act(Act::Expand(s, String::new())))),
                Some(_) => Ok((format!("{}, on its own, filtered: {value}", s.title()), Does::Act(Act::Expand(s, value.to_string())))),
            },
            K::Sources if here.has_sources => Ok(("its sources".into(), Does::Act(Act::Reveal(Reveal::Sources)))),
            K::Sources => Err("it has no sources".into()),
            K::Notes if here.has_notes => Ok(("its notes".into(), Does::Act(Act::Reveal(Reveal::Notes)))),
            K::Notes => Err("it has no notes".into()),
        });
    }
    // Words after a `|` (a section's name, say): only as an action's name, exactly.
    if !line.parts.is_empty() && !line.words.is_empty() {
        let typed = line.words.join(" ");
        out.push(match actions(&typed, here).into_iter().find(|r| r.label.split("  ").next().is_some_and(|l| l.eq_ignore_ascii_case(&typed))) {
            Some(Row { label, choice: Choice::Act(act), .. }) => Ok((label, Does::Act(act))),
            _ => Err(format!("“{typed}”: not a command")),
        });
    }
    out
}

/// The actions plain words find (and the keys to type, for the filters).
fn actions(typed: &str, here: &Here) -> Vec<Row> {
    let mut rows = Vec::new();
    let found = |label: &str, also: &[&str]| crate::catalog::query::loosely_in(typed, label) || also.iter().any(|a| a.starts_with(&typed.to_lowercase()));
    for &(section, count) in here.sections {
        let key = KEYS.iter().find(|(k, _)| *k == K::Section(section)).map(|(_, names)| *names).unwrap_or(&[]);
        if found(section.title(), key) {
            rows.push(Row { label: format!("{}  {}", section.title(), thousands(count)), sub: "on its own".into(), choice: Choice::Act(Act::Expand(section, String::new())) });
        }
    }
    if here.has_sources && found("Sources", &[]) {
        rows.push(Row { label: "Sources".into(), sub: "opened".into(), choice: Choice::Act(Act::Reveal(Reveal::Sources)) });
    }
    if here.has_notes && found("Notes", &[]) {
        rows.push(Row { label: "Notes".into(), sub: "opened".into(), choice: Choice::Act(Act::Reveal(Reveal::Notes)) });
    }
    if here.names.len() >= 2 && found("Names", &["alias", "as"]) {
        rows.push(Row { label: "alias:".into(), sub: "only what's credited under a name (or several)".into(), choice: Choice::Insert("alias:".into()) });
    }
    if here.og_count > 0 && found("Original files", &["og"]) {
        rows.push(Row { label: "og:".into(), sub: "only original files".into(), choice: Choice::Insert("og:".into()) });
    }
    if found("Play", &[]) {
        rows.push(Row { label: "Play".into(), sub: "its tracks".into(), choice: Choice::Act(Act::Play) });
    }
    if here.edit_mode {
        if found("Merge into another artist", &[]) {
            rows.push(Row { label: "Merge into another artist…".into(), sub: "Ctrl+M".into(), choice: Choice::Act(Act::Merge) });
        }
        if here.may_collective && found("Turn into a collective", &[]) {
            rows.push(Row { label: "Turn into a collective, or merge into one…".into(), sub: "shows what it would do first".into(), choice: Choice::Act(Act::Collective) });
        }
    }
    rows
}

/// The choices for the value being typed.
fn choices(part: &commandline::Part<K>, here: &Here, filters: &Filters) -> Vec<Row> {
    if part.minus {
        return Vec::new();
    }
    let typed = part.value.trim().to_lowercase();
    match part.key {
        K::Alias => {
            let mut rows: Vec<Row> = here
                .names
                .iter()
                .filter(|(n, _)| shown(here, n).to_lowercase().contains(&typed))
                .map(|(n, count)| {
                    let on = if filters.names.contains(n) { " · on" } else { "" };
                    Row { label: format!("as {}", shown(here, n)), sub: format!("{} tracks{on}", thousands(*count)), choice: Choice::Value(shown(here, n).to_string()) }
                })
                .collect();
            if "all".starts_with(&typed) && !filters.names.is_empty() {
                rows.push(Row { label: "All names".into(), sub: "every chip off".into(), choice: Choice::Value("all".into()) });
            }
            rows
        }
        K::Og if typed.is_empty() => {
            [("yes", "only original files"), ("no", "all files")].into_iter().map(|(v, sub)| Row { label: v.into(), sub: sub.into(), choice: Choice::Value(v.into()) }).collect()
        }
        _ => Vec::new(),
    }
}

impl ArtistCommands {
    /// The filters as the commandline would leave them: `committed` with what's typed
    /// (understood) on top. The page shows these while it's typed.
    pub fn preview(&self, here: &Here, committed: &Filters) -> Filters {
        let mut filters = committed.clone();
        for does in resolve(&self.cl.text, here).into_iter().flatten() {
            apply(&mut filters, does.1);
        }
        filters
    }

    /// The commandline, its list and what it would do. Returns what the page does now.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &Shell, here: &Here, filters: &mut Filters) -> Option<Act> {
        let ctx = ui.ctx().clone();
        // `/` outside a field: the cursor here.
        if !ctx.egui_wants_keyboard_input() && !shell.palette_open && !shell.modal_was_open && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "/"))) {
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Slash);
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
            });
            self.cl.focus = true;
        }
        let focused = self.cl.hold_focus(&ctx);
        let line = commandline::parse::<K>(&self.cl.text);
        let words = line.parts.is_empty() && !line.words.is_empty();
        let rows: Vec<Row> = match line.parts.last() {
            _ if words => actions(&line.words.join(" "), here),
            Some(last) => choices(last, here, filters),
            None => Vec::new(),
        };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));
        let mut take: Option<usize> = None;
        let mut enter = false;
        if focused {
            let pressed = self.cl.keys(&ctx, rows.len(), false, false);
            if (pressed.tab || pressed.enter) && !rows.is_empty() {
                take = Some(self.cl.highlighted);
            }
            enter = pressed.enter;
            if pressed.escape {
                self.notice = None;
            }
        }

        let hint = "/  alias:  og:  releases  tracks  featured  sources  notes  (a word finds the rest · | between commands)";
        if self.cl.field(ui, true, hint).changed() {
            self.notice = None;
        }
        let mut clicked = None;
        if !rows.is_empty() {
            let list: Vec<(String, String)> = rows.iter().map(|r| (r.label.clone(), r.sub.clone())).collect();
            cl::list(ui, list, self.cl.highlighted, &mut clicked, "↑/↓ choose · Tab: take it · Enter: take it and do it");
        }
        if clicked.is_some() {
            take = clicked;
        }

        let mut act = None;
        if let Some(n) = take
            && let Some(row) = rows.get(n)
        {
            match &row.choice {
                Choice::Value(text) => {
                    if let Some(part) = line.parts.last() {
                        let text = (*text != part.value).then(|| commandline::quoted::<K>(text));
                        self.cl.take(&ctx, part.value_start, part.end, text.as_deref(), None);
                    }
                }
                Choice::Insert(text) => {
                    self.cl.text = text.clone();
                    self.cl.cursor_to_end(&ctx);
                    enter = false;
                }
                Choice::Act(a) => {
                    act = Some(a.clone());
                    self.cl.text.clear();
                    enter = false;
                }
            }
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here);
            match done.iter().find_map(|d| d.as_ref().err()) {
                Some(err) => self.notice = Some(format!("Not done: {err}")),
                None => {
                    for (_, does) in done.into_iter().flatten() {
                        if let Does::Act(a) = does {
                            act = Some(a);
                        } else {
                            apply(filters, does);
                        }
                    }
                    self.cl.text.clear();
                    self.notice = None;
                }
            }
        }
        // What Enter would do, when no list is in the way.
        if rows.is_empty() && !self.cl.text.trim().is_empty() {
            let shown: Vec<Result<(String, Option<String>), String>> = resolve(&self.cl.text, here).into_iter().map(|r| r.map(|(label, _)| (label, None))).collect();
            cl::would_do(ui, &shown, "Nothing to do by that name");
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted).small());
        }
        // Something to look at: the keys go there.
        if act.is_some() {
            ctx.memory_mut(|m| m.surrender_focus(self.cl.id()));
        }
        act
    }
}

fn apply(filters: &mut Filters, does: Does) {
    match does {
        Does::Name(name) => filters.toggle(name),
        Does::AllNames => filters.names.clear(),
        Does::Og(on) => filters.og_only = on,
        Does::Act(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here<'a>(names: &'a [(Option<String>, usize)], sections: &'a [(Section, usize)]) -> Here<'a> {
        Here { own: "Zed", names, og_count: 2, sections, has_sources: true, has_notes: false, edit_mode: false, may_collective: false }
    }

    #[test]
    fn names_turn_on_and_off_live() {
        let names = [(None, 5), (Some("Zee".to_string()), 3), (Some("ZEE".to_string()), 1), (Some("Zeddy".to_string()), 2)];
        let here = here(&names, &[]);
        let mut commands = ArtistCommands::default();
        let none = Filters::default();
        commands.cl.text = "alias:Zee | alias:zeddy".into();
        let after = commands.preview(&here, &none);
        assert_eq!(after.names, [Some("Zee".to_string()), Some("Zeddy".to_string())].into_iter().collect());
        // Typed again: off. Its own name too. Not yet a whole name: nothing.
        commands.cl.text = "alias:zee".into();
        assert!(commands.preview(&here, &none).names.is_empty(), "zee is two names but for case: neither");
        commands.cl.text = "alias:Zee".into();
        assert!(commands.preview(&here, &after).names.contains(&Some("Zeddy".to_string())));
        assert!(!commands.preview(&here, &after).names.contains(&Some("Zee".to_string())));
        commands.cl.text = "alias:zed".into();
        assert_eq!(commands.preview(&here, &none).names, [None].into_iter().collect());
        commands.cl.text = "alias:Ze".into();
        assert_eq!(commands.preview(&here, &none), none);
        commands.cl.text = "alias:all".into();
        assert!(commands.preview(&here, &after).names.is_empty());
        commands.cl.text = "og: | alias:Zeddy".into();
        let f = commands.preview(&here, &none);
        assert!(f.og_only && f.shows(Some("Zeddy")) && !f.shows(None));
        commands.cl.text = "og:no".into();
        assert!(!commands.preview(&here, &f).og_only);
    }

    #[test]
    fn sections_and_their_keys() {
        let sections = [(Section::Releases, 4), (Section::Tracks, 30)];
        let here = here(&[], &sections);
        let does = |text: &str| resolve(text, &here).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        assert_eq!(does("releases:"), [Ok(Does::Act(Act::Expand(Section::Releases, String::new())))]);
        assert_eq!(does("release:"), [Ok(Does::Act(Act::Expand(Section::Releases, String::new())))], "not year:");
        assert_eq!(does("tracks:dark year:2019"), [Ok(Does::Act(Act::Expand(Section::Tracks, "dark year:2019".into())))]);
        assert!(does("featured:")[0].is_err(), "none here");
        assert!(does("releases:x")[0].is_err());
        assert_eq!(does("sources:"), [Ok(Does::Act(Act::Reveal(Reveal::Sources)))]);
        assert!(does("og:maybe")[0].is_err());
        assert_eq!(does("og:yes | tracks"), [Ok(Does::Og(true)), Ok(Does::Act(Act::Expand(Section::Tracks, String::new())))]);
        assert!(does("og:yes | nonsense")[1].is_err());
        // Plain words find them.
        let labels: Vec<String> = actions("rel", &here).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["Releases  4"]);
        assert!(actions("merge", &here).is_empty(), "merging is edit mode's");
    }
}
