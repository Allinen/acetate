//! The artist page's commandline (a `ui::edit::commandline`; the language is
//! `crate::commandline`'s). It's there in listen mode too, for moving around and filtering.
//!
//! * `alias:Name` turns that name's chip on or off: live while it's typed, kept with Enter.
//!   `alias:all` goes back to all names. `og:` / `og:yes` / `og:no` sets the Original files
//!   chip. The chips are the state; the commandline only changes them, and Esc puts back
//!   what they were.
//! * `releases`, `tracks`, `featured`, `produced`, `extras` (a word, or `key:`) show that
//!   section on its own. `tracks:words` also sets its filter. `sources`, `notes` and
//!   `history` are opened and scrolled to (a folded one that's open is closed).
//! * Plain words find these too, and `play`, plus `merge` and `collective` in edit mode.
//!
//! Edit mode (and `edit_metadata`) adds changes, staged as chips and saved together with
//! Ctrl+Enter (through the artist's form: one revision, the form says how it went). Outside
//! edit mode they're not suggested, and typed they say why not.
//!
//! * `alias:New name` (a name it isn't credited as yet) adds it. Under `alias:` the list has
//!   the names to filter by first, then "New name". `-alias:Name` takes one off: where it's
//!   still credited is asked first, and if it is anywhere, it isn't staged (the credits are
//!   listed, as links).
//! * `name:` renames it, `disambiguation:` (`-disambiguation` clears it) says what tells it
//!   apart from others with its name, `why:` is kept with the change (alone it has nothing
//!   to go with, and saving says so).
//! * `source:<link>`, `source:custom label, description`, `source:<platform> field:…` adds a
//!   source (as the track editor's `source:`, with an artist's platforms; `edit_sources`),
//!   `-source:#2` puts its second one in the trash. Each is sent on its own: one failing
//!   leaves the rest (it stays staged, saying why).
//! * `extra:Name` with its arguments: `kind:`, `file:` (a path, Tab completes it;
//!   `file:paste` the files copied in a file manager, `file:choose` the file dialog: both
//!   put the paths in the line), `description:`, `source:` (where it came from),
//!   `visibility:everyone` / `artist` / `Group A, Group B` (`manage_access`). A name the
//!   artist has an extra by changes that one; one in the library links it here; else it's
//!   a new extra (chosen in the list: that one). `-extra:Name`: not on this artist anymore
//!   (it stays, and wherever else it's linked).
//! * `note:Title` opens the note editor: one of its notes by that title, else a new one with
//!   it. `note:paste` lists the text files copied (a note each, made with Ctrl+Enter).
//!   `-note:Title` takes one off (kept, "Taken off" brings it back).

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::artist::Section;
use super::page_commands::{self as pc, Arg, Change, Chosen, Known, NoteDoes, Sending, Taken, Things, Whose};
use crate::commandline::{self, Keys};
use crate::account::SourceShape;
use crate::details::{AliasInfo, Source};
use crate::ui::edit::extras::Groups;
use crate::ui::edit::artist_form::{self, ArtistForm, Uses, UsesAnswer};
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
    History,
    Name,
    Disambiguation,
    Why,
    Source,
    /// One of a source's fields (after `source:<platform>`): its name is the part's.
    Field,
    Extra,
    Kind,
    File,
    Description,
    Visibility,
    Note,
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
    (K::History, &["history", "revisions"]),
    (K::Name, &["name", "rename"]),
    (K::Disambiguation, &["disambiguation", "disamb", "dis", "tells"]),
    (K::Why, &["why", "reason"]),
    (K::Source, &["source"]),
    (K::Extra, &["extra"]),
    (K::Kind, &["kind", "type"]),
    (K::File, &["file", "files"]),
    (K::Description, &["description", "desc"]),
    (K::Visibility, &["visibility", "vis", "seen"]),
    (K::Note, &["note"]),
];

impl Keys for K {
    fn key(name: &str) -> Option<K> {
        let name = name.to_lowercase();
        KEYS.iter().find(|(_, names)| names.contains(&name.as_str())).map(|(k, _)| *k)
    }

    fn name(self) -> &'static str {
        KEYS.iter().find(|(k, _)| *k == self).map(|(_, names)| names[0]).unwrap_or("")
    }

    fn is_list(self) -> bool {
        self == K::Disambiguation
    }

    fn takes_hash(self) -> bool {
        self == K::Source
    }

    fn takes(main: K, arg: K) -> bool {
        main == K::Extra && matches!(arg, K::Kind | K::File | K::Description | K::Source | K::Visibility)
    }

    fn field(main: K) -> Option<K> {
        (main == K::Source).then_some(K::Field)
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
    pub id: Uuid,
    /// The artist's own name.
    pub own: &'a str,
    /// The names it's credited under, with how many tracks (its own: None).
    pub names: &'a [(Option<String>, usize)],
    pub og_count: usize,
    /// The sections shown, with how many in each.
    pub sections: &'a [(Section, usize)],
    pub has_sources: bool,
    pub has_notes: bool,
    /// Its history is there (for those who may see it).
    pub has_history: bool,
    /// The folded parts' state (open or not).
    pub folds: &'a [(Reveal, bool)],
    pub edit_mode: bool,
    /// Edit mode, and `edit_metadata`: the artist can be changed here.
    pub may_edit: bool,
    /// "Turn into a collective…" is there (edit mode, `edit_sources`).
    pub may_collective: bool,
    /// Its other names (all of them, credited or not), and what tells it apart.
    pub aliases: &'a [AliasInfo],
    pub disambiguation: &'a str,
    /// Its sources, and whether they can be changed here (edit mode, `edit_sources`).
    pub sources: &'a [Source],
    pub may_source: bool,
    /// Every platform's sources' fields (an artist's are those of kind "artist").
    pub shapes: &'a [SourceShape],
    /// Its extras (taken off ones too), every extra in the library, and whether who sees
    /// one can be said here (`manage_access`).
    pub extras: &'a [crate::details::Extra],
    pub library_extras: &'a [crate::catalog::model::Extra],
    pub may_access: bool,
    /// Its notes (taken off ones too).
    pub notes: &'a [crate::details::Note],
}

impl Here<'_> {
    /// Its sources, extras and notes, for what the pages share.
    fn things(&self) -> Things<'_> {
        Things {
            whose: Whose::Artist(self.id),
            editing: editing(self),
            sources: self.sources,
            may_source: self.may_source,
            shapes: self.shapes,
            extras: self.extras,
            library_extras: self.library_extras,
            may_access: self.may_access,
            notes: self.notes,
        }
    }
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
    /// Edit mode on or off.
    ToggleEdit,
    /// The note editor: for this note, or (None) a new one with this title.
    Note(Option<Uuid>, String),
    /// The text files copied in a file manager, listed to be made notes.
    PasteNotes,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reveal {
    Sources,
    Notes,
    History,
}

impl Reveal {
    fn title(self) -> &'static str {
        match self {
            Reveal::Sources => "Sources",
            Reveal::Notes => "Notes",
            Reveal::History => "History",
        }
    }
}

/// What showing a part of the page does: a folded one opens or closes, the rest are
/// scrolled to.
fn reveal_label(here: &Here, r: Reveal) -> String {
    match here.folds.iter().find(|(f, _)| *f == r) {
        Some((_, true)) => format!("{}: closed", r.title()),
        Some((_, false)) => format!("{}: opened", r.title()),
        None => format!("{}: scrolled to", r.title()),
    }
}

/// What a command does.
#[derive(Debug, Clone, PartialEq)]
enum Does {
    /// A name's chip on or off.
    Name(Option<String>),
    AllNames,
    Og(bool),
    Act(Act),
    /// A change, staged.
    Edit(Edit),
}

/// A change to the artist, staged until Ctrl+Enter.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    NewAlias(String),
    /// Taken off (where it's credited is asked first).
    RemoveAlias(Uuid, String),
    Rename(String),
    Disambiguation(String),
    Why(String),
    /// A source, an extra or a note: sent on its own.
    Thing(Change),
}

impl Edit {
    /// Saved through the artist's form (one revision), not on its own.
    fn in_form(&self) -> bool {
        !matches!(self, Edit::Thing(_))
    }
}

type Staged = pc::Staged<Edit>;
type Row = pc::Row<Act, Edit>;
type Choice = pc::Choice<Act, Edit>;

pub struct ArtistCommands {
    cl: CommandLine,
    notice: Option<String>,
    /// The artist they're for (another one: they start over, and these are kept for when
    /// it's back).
    artist: Option<Uuid>,
    staged: Vec<Staged>,
    /// Other artists' changes staged, not saved yet.
    kept: std::collections::HashMap<Uuid, Vec<Staged>>,
    /// Names to take off, asked where they're credited first: (alias, typed as, name).
    asking: Vec<(Uuid, String, String)>,
    uses: UsesAnswer,
    /// A name that couldn't go, and where it's still credited.
    blocked: Option<(String, Uses)>,
    /// Sources, extras and notes being saved, each on its own.
    sending: Sending<Edit>,
    known: Known,
    groups: Groups,
    /// Files chosen in the dialog (`file:choose`), to put in the commandline.
    chosen: Chosen,
    /// The frame it was last shown in (not the one before: the page was just opened).
    last_pass: u64,
}

impl Default for ArtistCommands {
    fn default() -> Self {
        ArtistCommands {
            cl: CommandLine::new(egui::Id::new("artist-commandline")),
            notice: None,
            artist: None,
            staged: Vec::new(),
            kept: Default::default(),
            asking: Vec::new(),
            uses: UsesAnswer::default(),
            blocked: None,
            sending: Sending::default(),
            known: Known::default(),
            groups: Groups::default(),
            chosen: Chosen::new("file"),
            last_pass: 0,
        }
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

/// Whether the artist can be changed here (else why not).
fn editing(here: &Here) -> Result<(), String> {
    if !here.edit_mode {
        Err("changing the artist is in edit mode: e".into())
    } else if !here.may_edit {
        Err("changing the artist needs the edit_metadata permission".into())
    } else {
        Ok(())
    }
}

/// One of its other names, by name: exactly, else the one that's the same but for case.
fn alias_named<'a>(here: &'a Here, typed: &str) -> Option<&'a AliasInfo> {
    if let Some(a) = here.aliases.iter().find(|a| a.name == typed) {
        return Some(a);
    }
    let mut alike = here.aliases.iter().filter(|a| a.name.to_lowercase() == typed.to_lowercase());
    match (alike.next(), alike.next()) {
        (Some(a), None) => Some(a),
        _ => None,
    }
}

/// `typed` as a new other name, if it can be one.
fn new_alias(here: &Here, typed: &str) -> Result<Edit, String> {
    editing(here)?;
    if typed == here.own {
        return Err(format!("“{typed}” is its own name"));
    }
    if here.aliases.iter().any(|a| a.name == typed) {
        return Err(format!("“{typed}” is one of its names already (nothing's credited as it yet)"));
    }
    Ok(Edit::NewAlias(typed.to_string()))
}

/// An `extra:` argument's key, as the shared ones know it.
fn arg_of(key: K) -> Option<Arg> {
    Some(match key {
        K::Kind => Arg::Kind,
        K::File => Arg::File,
        K::Description => Arg::Description,
        K::Source => Arg::Source,
        K::Visibility => Arg::Visibility,
        _ => return None,
    })
}

/// A source, an extra or a note, as the artist's command.
fn thing(done: Result<(String, Change), String>) -> Result<(String, Does), String> {
    done.map(|(label, change)| (label, Does::Edit(Edit::Thing(change))))
}

/// Each command typed: its text, and what it does (or why it can't).
fn resolve(text: &str, here: &Here, known: &Known) -> Vec<Result<(String, Does), String>> {
    let line = commandline::parse::<K>(text);
    let mut out = Vec::new();
    for command in line.commands() {
        let main = &command[0];
        let value = main.value.trim();
        if main.minus && !matches!(main.key, K::Alias | K::Disambiguation | K::Source | K::Extra | K::Note) {
            out.push(Err(format!("-{}: nothing to take off here", main.key.name())));
            continue;
        }
        let edit = |label: String, edit: Edit| editing(here).map(|_| (label, Does::Edit(edit)));
        out.push(match main.key {
            K::Alias if main.minus => match editing(here) {
                Err(err) => Err(err),
                Ok(()) if value.is_empty() => Err("-alias:Name takes that name off it".into()),
                Ok(()) => match alias_named(here, value) {
                    Some(a) => Ok((format!("− other name “{}” (if nothing's credited as it)", a.name), Does::Edit(Edit::RemoveAlias(a.id, a.name.clone())))),
                    None => Err(format!("it has no other name “{value}”")),
                },
            },
            K::Alias if value.is_empty() => Err("alias: a name it's credited under (the list), or all".into()),
            K::Alias if value.eq_ignore_ascii_case("all") => Ok(("all names".into(), Does::AllNames)),
            K::Alias => match name_of(here, value) {
                Some(name) => Ok((format!("as {}: on or off", shown(here, &name)), Does::Name(name))),
                None if !here.edit_mode => Err(format!("it isn't credited as “{value}” on any track (a new name: in edit mode, e)")),
                None => new_alias(here, value).map(|e| (format!("+ other name “{value}”"), Does::Edit(e))),
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
            K::Sources if here.has_sources => Ok((reveal_label(here, Reveal::Sources), Does::Act(Act::Reveal(Reveal::Sources)))),
            K::Sources => Err("it has no sources".into()),
            K::Notes if here.has_notes => Ok((reveal_label(here, Reveal::Notes), Does::Act(Act::Reveal(Reveal::Notes)))),
            K::Notes => Err("it has no notes".into()),
            K::History if here.has_history => Ok((reveal_label(here, Reveal::History), Does::Act(Act::Reveal(Reveal::History)))),
            K::History => Err("its history isn't shown to you (view_history)".into()),
            K::Name if value.is_empty() => Err("name: its new name".into()),
            K::Name => edit(format!("name → {value}"), Edit::Rename(value.to_string())),
            K::Disambiguation if value.is_empty() => edit("tells apart: nothing".into(), Edit::Disambiguation(String::new())),
            K::Disambiguation => edit(format!("tells apart: “{value}”"), Edit::Disambiguation(value.to_string())),
            K::Why => edit(format!("why: {value} (kept with a change to its names or what tells it apart)"), Edit::Why(value.to_string())),
            K::Source => {
                let fields: Vec<(&str, &str)> = command[1..].iter().map(|p| (p.name(), p.value.trim())).collect();
                thing(pc::source(main.minus, &main.value, &main.hashes, &fields, &here.things()))
            }
            K::Field => Err(format!("{}: goes after source: and its platform (source:soundcloud artist_permalinks:…)", main.name())),
            K::Extra => {
                let args: Vec<(Arg, &str)> = command[1..].iter().filter_map(|a| Some((arg_of(a.key)?, a.value.as_str()))).collect();
                thing(pc::extra(&main.value, main.minus, &args, &here.things(), known))
            }
            K::Note => pc::note(&main.value, main.minus, &here.things(), known).map(|(label, does)| {
                let does = match does {
                    NoteDoes::Open(note, title) => Does::Act(Act::Note(note, title)),
                    NoteDoes::Paste => Does::Act(Act::PasteNotes),
                    NoteDoes::Hide(note) => Does::Edit(Edit::Thing(Change::HideNote(note))),
                };
                (label, does)
            }),
            K::Kind | K::File | K::Description | K::Visibility => Err(format!("{}: goes after extra:Name", main.key.name())),
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
    for (reveal, there) in [(Reveal::Sources, here.has_sources), (Reveal::Notes, here.has_notes), (Reveal::History, here.has_history)] {
        if there && found(reveal.title(), &[]) {
            let label = reveal_label(here, reveal);
            let sub = label.rsplit(": ").next().unwrap_or("").to_string();
            rows.push(Row { label: reveal.title().into(), sub, choice: Choice::Act(Act::Reveal(reveal)) });
        }
    }
    if here.names.len() >= 2 && found("Names", &["alias", "as"]) {
        rows.push(Row { label: "alias:".into(), sub: "only what's credited under a name (or several)".into(), choice: Choice::Insert("alias:".into()) });
    }
    if here.og_count > 0 && found("Original files", &["og"]) {
        rows.push(Row { label: "og:".into(), sub: "only original files".into(), choice: Choice::Insert("og:".into()) });
    }
    if found(if here.edit_mode { "Edit mode off" } else { "Edit mode on" }, &["edit"]) {
        rows.push(Row { label: if here.edit_mode { "Edit mode off".into() } else { "Edit mode on".into() }, sub: "Alt+E, anywhere".into(), choice: Choice::Act(Act::ToggleEdit) });
    }
    if found("Play", &[]) {
        rows.push(Row { label: "Play".into(), sub: "its tracks".into(), choice: Choice::Act(Act::Play) });
    }
    if editing(here).is_ok() {
        for (key, label, sub) in [
            ("alias:", "New name (alias:)", "another name tracks can be credited as"),
            ("-alias:", "Take a name off (-alias:)", "if nothing's credited as it"),
            ("name:", "Rename (name:)", "its name"),
            ("disambiguation:", "Tells apart (disambiguation:)", "for two artists with the same name"),
            ("why:", "Why (why:)", "kept with the artist's changes"),
            ("note:", "Note (note:)", "a new one, or one of its notes to edit"),
            ("extra:", "Extra (extra:)", "a new one, one of its own to change, or one in the library"),
            ("source:", "Source (source:)", "a link, a platform and its fields, or custom"),
        ] {
            if found(label, &[key.trim_end_matches(':').trim_start_matches('-')]) {
                rows.push(Row { label: label.into(), sub: sub.into(), choice: Choice::Insert(key.into()) });
            }
        }
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
/// The choices for the value being typed (`group`: what it goes with, if it's an argument
/// or a field).
fn choices(part: &commandline::Part<K>, group: &[commandline::Part<K>], here: &Here, known: &Known, filters: &Filters) -> Vec<Row> {
    let typed = part.value.trim().to_lowercase();
    let things = here.things();
    match part.key {
        K::Source if !part.arg => return pc::source_rows(part.minus, &part.value, part.hash.is_some(), &things),
        K::Note => return pc::note_rows(part.minus, &part.value, &things),
        K::Extra => return pc::extra_rows(part.minus, &part.value, &things),
        _ => {}
    }
    if part.minus {
        if part.key != K::Alias || editing(here).is_err() {
            return Vec::new();
        }
        return here
            .aliases
            .iter()
            .filter(|a| a.name.to_lowercase().contains(&typed))
            .map(|a| Row { label: format!("− {}", a.name), sub: "taken off, if nothing's credited as it".into(), choice: Choice::Value(a.name.clone()) })
            .collect();
    }
    if let Some(arg) = arg_of(part.key).filter(|_| group.first().is_some_and(|g| g.key == K::Extra)) {
        return pc::extra_arg_rows(arg, &part.value, &things, known);
    }
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
            // A new one: what's typed, exactly (a name that's another one's but for case too).
            let value = part.value.trim();
            if !value.is_empty()
                && !value.eq_ignore_ascii_case("all")
                && let Ok(edit) = new_alias(here, value)
            {
                let text = format!("alias:{}", commandline::quoted::<K>(value));
                rows.push(Row { label: format!("New name “{value}”"), sub: "saved with Ctrl+Enter".into(), choice: Choice::Stage(text, format!("+ other name “{value}”"), edit) });
            }
            rows
        }
        K::Field => {
            let Some(owner) = group.first().filter(|o| o.key == K::Source) else { return Vec::new() };
            let given: Vec<&str> = group.iter().skip(1).chain([part]).map(|p| p.name()).collect();
            pc::field_rows(&owner.value, part.name(), &part.value, &given, &things)
        }
        // What it is now, to change.
        K::Name if typed.is_empty() && editing(here).is_ok() => vec![Row { label: here.own.to_string(), sub: "its name now".into(), choice: Choice::Value(here.own.to_string()) }],
        K::Disambiguation if typed.is_empty() && !here.disambiguation.is_empty() && editing(here).is_ok() => {
            vec![Row { label: here.disambiguation.to_string(), sub: "what tells it apart now".into(), choice: Choice::Value(here.disambiguation.to_string()) }]
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
        for does in resolve(&self.cl.text, here, &self.known).into_iter().flatten() {
            apply(&mut filters, does.1);
        }
        filters
    }

    /// Changes are staged (the page has changes not saved).
    pub fn has_changes(&self) -> bool {
        !self.staged.is_empty() || !self.asking.is_empty()
    }

    /// The commandline for the artist `id`, its list and what it would do, and the changes
    /// staged. Returns what the page does now.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, here: &Here, filters: &mut Filters) -> Option<Act> {
        if self.artist != Some(id) {
            let mut kept = std::mem::take(&mut self.kept);
            if let Some(was) = self.artist
                && !self.staged.is_empty()
            {
                kept.insert(was, std::mem::take(&mut self.staged));
            }
            let staged = kept.remove(&id).unwrap_or_default();
            *self = ArtistCommands { artist: Some(id), staged, kept, ..Default::default() };
        }
        let ctx = ui.ctx().clone();
        self.answers(shell);
        // Who can see an extra: the groups, asked for once it's typed.
        pc::ask_groups_if::<K>(&self.cl.text, |k| k == K::Visibility, &here.things(), shell, &mut self.known, &self.groups);
        // Files chosen in the dialog (`file:choose`): their paths in the line.
        self.chosen.put_in_line(&mut self.cl);
        pc::arrive(&mut self.cl, &ctx, shell, &mut self.last_pass);
        let focused = self.cl.hold_focus(&ctx);
        let line = commandline::parse::<K>(&self.cl.text);
        let words = line.parts.is_empty() && !line.words.is_empty();
        let rows: Vec<Row> = match line.parts.last() {
            _ if words => actions(&line.words.join(" "), here),
            Some(last) => choices(last, line.last_goes_with(), here, &self.known, filters),
            None => Vec::new(),
        };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));
        let can_edit = editing(here).is_ok();
        // (The row, and whether it was Tab: the next part written after it.)
        let mut take: Option<(usize, bool)> = None;
        let mut enter = false;
        if focused {
            let pressed = self.cl.keys(&ctx, rows.len(), false, can_edit && !self.staged.is_empty());
            if (pressed.tab || pressed.enter) && !rows.is_empty() {
                take = Some((self.cl.highlighted, pressed.tab));
            }
            enter = pressed.enter;
            if pressed.escape {
                self.notice = None;
            }
            // Backspace on an empty line: the last change back, to edit.
            if pressed.back
                && let Some(last) = self.staged.pop()
            {
                self.cl.text = last.text;
                self.cl.cursor_to_end(&ctx);
            }
        }
        // Ctrl+Enter: what's typed first, then the changes saved. (Without changes here,
        // the form below takes it.)
        let mut save = false;
        let elsewhere = !focused && ctx.egui_wants_keyboard_input();
        if can_edit && !elsewhere && (self.has_changes() || (focused && !self.cl.text.trim().is_empty())) && !shell.palette_open && !shell.modal_was_open {
            save = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
            enter |= save && !self.cl.text.trim().is_empty();
        }

        let hint = if can_edit {
            "/  alias:  -alias:  name:  disambiguation:  og:  releases  tracks  sources  (a word finds the rest · | between commands · Ctrl+Enter saves)"
        } else {
            "/  alias:  og:  releases  tracks  featured  sources  notes  (a word finds the rest · | between commands)"
        };
        if self.cl.field(ui, true, hint).changed() {
            self.notice = None;
            self.blocked = None;
        }
        let mut clicked = None;
        if !rows.is_empty() {
            let list: Vec<(String, String)> = rows.iter().map(|r| (r.label.clone(), r.sub.clone())).collect();
            cl::list(ui, list, self.cl.highlighted, &mut clicked, "↑/↓ choose · Tab: take it · Enter: take it and do it");
        }
        if let Some(n) = clicked {
            take = Some((n, true));
        }

        let mut act = None;
        if let Some((n, tab)) = take
            && let Some(row) = rows.get(n)
        {
            match pc::take(&mut self.cl, &ctx, &mut self.known, &line, &row.choice, tab) {
                Taken::Filled => {}
                Taken::Inserted => enter = false,
                Taken::Act(a) => {
                    act = Some(a);
                    enter = false;
                }
                Taken::Stage(staged) => {
                    self.staged.push(staged);
                    enter = false;
                }
            }
        }
        // `file:paste` and `file:choose`: the paths in the line first (what's staged is what
        // the line says).
        if enter && let Err(err) = self.expand_files(shell) {
            self.notice = Some(err);
            enter = false;
            save = false;
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here, &self.known);
            let texts = pc::commands_text::<K>(&self.cl.text);
            match done.iter().find_map(|d| d.as_ref().err()) {
                Some(err) => {
                    self.notice = Some(format!("Not done: {err}"));
                    save = false;
                }
                None => {
                    for (n, (label, does)) in done.into_iter().flatten().enumerate() {
                        let text = texts.get(n).cloned().unwrap_or_default();
                        match does {
                            Does::Act(a) => act = Some(a),
                            Does::Edit(Edit::RemoveAlias(alias, name)) => {
                                artist_form::ask_uses(shell, id, alias, &self.uses);
                                self.asking.push((alias, text, name));
                            }
                            Does::Edit(edit) => self.staged.push(Staged { text, label, edit, failed: None }),
                            does => apply(filters, does),
                        }
                    }
                    self.cl.text.clear();
                    self.notice = None;
                }
            }
        }
        if save {
            self.save(shell, id);
        }

        // What Enter would do, when no list is in the way.
        if rows.is_empty() && !self.cl.text.trim().is_empty() {
            let shown: Vec<Result<(String, Option<String>), String>> = resolve(&self.cl.text, here, &self.known).into_iter().map(|r| r.map(|(label, _)| (label, None))).collect();
            cl::would_do(ui, &shown, "Nothing to do by that name");
        }
        for (_, _, name) in &self.asking {
            ui.label(RichText::new(format!("Checking where “{name}” is credited…")).color(P.faint).small());
        }
        if let Some((name, uses)) = &self.blocked {
            artist_form::uses_links(ui, shell, name, uses);
        }
        if let Some(n) = pc::status(ui, self.notice.as_deref(), self.sending.count, &self.staged, can_edit) {
            self.staged.remove(n);
        }
        if self.has_changes() {
            shell.form_dirty = true;
        }
        // Something to look at (a section, the note editor): the keys go there, and the
        // commandline doesn't take the cursor back. Edit mode switched: it stays here.
        if act.as_ref().is_some_and(|a| *a != Act::ToggleEdit) {
            pc::let_go(&mut self.cl, &ctx);
        }
        act
    }

    /// `file:paste` becomes the copied files' paths (`file:/…` each); `file:choose` opens the
    /// dialog and comes out of the line (its paths are put in when chosen: Enter again).
    /// Err: why not now (no files copied; the dialog is open).
    fn expand_files(&mut self, shell: &Shell) -> Result<(), String> {
        let dialog = pc::Dialog { title: "Files for the extra", kinds: ("Any file", &[]), many: true };
        pc::expand_files::<K>(&mut self.cl.text, shell, |p| p.key == K::File && p.arg, true, dialog, &self.chosen)
    }

    /// What the server said about the sources sent (one failing: staged again, saying why),
    /// and where the names to take off are credited: nowhere, staged; somewhere, said.
    fn answers(&mut self, shell: &mut Shell) {
        if let Some(said) = self.sending.answers(&mut self.staged) {
            if let Some(id) = self.artist {
                shell.artist_details.refresh(id);
            }
            shell.act(crate::ui::Action::RefreshLibrary);
            if !said.is_empty() {
                self.notice = Some(said.join(" "));
            }
        }
        for (alias, result) in std::mem::take(&mut *self.uses.lock().unwrap()) {
            let Some(at) = self.asking.iter().position(|(a, _, _)| *a == alias) else { continue };
            let (_, text, name) = self.asking.remove(at);
            match result {
                Ok(uses) if uses.is_empty() => {
                    let label = format!("− other name “{name}”");
                    self.staged.push(Staged { text, label, edit: Edit::RemoveAlias(alias, name), failed: None });
                }
                Ok(uses) => self.blocked = Some((name, uses)),
                Err(err) => self.notice = Some(format!("Couldn't check where “{name}” is credited: {err}")),
            }
        }
    }

    /// The changes saved. The artist's own (names, what tells it apart) onto its form (its
    /// draft, kept in the shell) and saved from there: one revision, the form says how it
    /// went. Sources each on their own (`answers` takes what the server said).
    fn save(&mut self, shell: &mut Shell, id: Uuid) {
        if self.staged.is_empty() {
            return;
        }
        let (form_edits, others): (Vec<Staged>, Vec<Staged>) = std::mem::take(&mut self.staged).into_iter().partition(|s| s.edit.in_form());
        let mut notices = Vec::new();
        if !form_edits.is_empty() {
            let d = shell.artist_details.get(id).value.clone();
            match d.as_ref().and_then(|d| shell.artist_forms.remove(&id).or_else(|| ArtistForm::from_detail(d))) {
                None => {
                    notices.push(if d.is_none() { "The artist is still loading: try again in a moment." } else { "This artist has no revision to build on, so it can't be changed here." });
                    self.staged.extend(form_edits);
                }
                Some(mut form) => {
                    for s in form_edits {
                        match s.edit {
                            Edit::NewAlias(name) => form.data.aliases.push(artist_form::AliasRow { id: None, name }),
                            Edit::RemoveAlias(alias, _) => form.data.aliases.retain(|a| a.id != Some(alias)),
                            Edit::Rename(name) => form.data.name = name,
                            Edit::Disambiguation(text) => form.data.disambiguation = text,
                            Edit::Why(why) => form.set_reason(&why),
                            Edit::Thing(_) => {}
                        }
                    }
                    if form.dirty() {
                        form.save(shell);
                        notices.push("Saving… (the artist's form below says how it went)");
                    } else {
                        notices.push("Its names and what tells it apart are as they were: nothing to save there (why: is kept with a change to those).");
                    }
                    shell.artist_forms.insert(id, form);
                }
            }
        }
        for change in others {
            self.sending.spawn(shell, change, move |api, ctx, edit| async move {
                match edit {
                    Edit::Thing(change) => pc::send(&api, &ctx, Whose::Artist(id), &change).await,
                    _ => Ok(None),
                }
            });
        }
        self.notice = (!notices.is_empty()).then(|| notices.join(" "));
    }
}

fn apply(filters: &mut Filters, does: Does) {
    match does {
        Does::Name(name) => filters.toggle(name),
        Does::AllNames => filters.names.clear(),
        Does::Og(on) => filters.og_only = on,
        Does::Act(_) | Does::Edit(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here<'a>(names: &'a [(Option<String>, usize)], sections: &'a [(Section, usize)]) -> Here<'a> {
        Here { id: Uuid::nil(), own: "Zed", names, og_count: 2, sections, has_sources: true, has_notes: false, has_history: true, folds: &[(Reveal::Sources, true)], edit_mode: false, may_edit: false, may_collective: false, aliases: &[], disambiguation: "", sources: &[], may_source: false, shapes: &[], extras: &[], library_extras: &[], may_access: false, notes: &[] }
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
        let does = |text: &str| resolve(text, &here, &Known::default()).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        assert_eq!(does("releases:"), [Ok(Does::Act(Act::Expand(Section::Releases, String::new())))]);
        assert_eq!(does("release:"), [Ok(Does::Act(Act::Expand(Section::Releases, String::new())))], "not year:");
        assert_eq!(does("tracks:dark year:2019"), [Ok(Does::Act(Act::Expand(Section::Tracks, "dark year:2019".into())))]);
        assert!(does("featured:")[0].is_err(), "none here");
        assert!(does("releases:x")[0].is_err());
        assert_eq!(does("sources:"), [Ok(Does::Act(Act::Reveal(Reveal::Sources)))]);
        assert_eq!(resolve("sources: | history:", &here, &Known::default()).into_iter().map(|r| r.unwrap().0).collect::<Vec<_>>(), ["Sources: closed", "History: scrolled to"]);
        assert!(does("og:maybe")[0].is_err());
        assert_eq!(does("og:yes | tracks"), [Ok(Does::Og(true)), Ok(Does::Act(Act::Expand(Section::Tracks, String::new())))]);
        assert!(does("og:yes | nonsense")[1].is_err());
        // Plain words find them.
        let labels: Vec<String> = actions("rel", &here).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["Releases  4"]);
        assert!(actions("merge", &here).is_empty(), "merging is edit mode's");
    }

    #[test]
    fn changes_are_edit_modes() {
        let names = [(None, 5), (Some("Zee".to_string()), 3)];
        let aliases = [AliasInfo { id: Uuid::from_u128(1), name: "Zee".into() }, AliasInfo { id: Uuid::from_u128(2), name: "Unused".into() }];
        let listen = Here { aliases: &aliases, ..here(&names, &[]) };
        let edit = Here { edit_mode: true, may_edit: true, ..Here { aliases: &aliases, ..here(&names, &[]) } };
        let does = |text: &str, here: &Here| resolve(text, here, &Known::default()).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        // A name it isn't credited as: new, in edit mode only.
        assert!(does("alias:Zeta", &listen)[0].as_ref().unwrap_err().contains("edit mode"));
        assert_eq!(does("alias:Zeta", &edit), [Ok(Does::Edit(Edit::NewAlias("Zeta".into())))]);
        // One it's credited as filters, in both; one it has but isn't credited as can't be new.
        assert_eq!(does("alias:zee", &edit), [Ok(Does::Name(Some("Zee".into())))]);
        assert!(does("alias:Unused", &edit)[0].as_ref().unwrap_err().contains("already"));
        assert_eq!(does("alias:Zed", &edit), [Ok(Does::Name(None))], "its own name filters");
        // Taking one off, renaming, what tells it apart: edit mode's.
        assert_eq!(does("-alias:unused", &edit), [Ok(Does::Edit(Edit::RemoveAlias(Uuid::from_u128(2), "Unused".into())))]);
        assert!(does("-alias:Nobody", &edit)[0].is_err());
        assert!(does("-alias:Zee", &listen)[0].is_err());
        assert_eq!(does("name:Zed Two | disambiguation:US rapper", &edit), [Ok(Does::Edit(Edit::Rename("Zed Two".into()))), Ok(Does::Edit(Edit::Disambiguation("US rapper".into())))]);
        assert_eq!(does("-disambiguation", &edit), [Ok(Does::Edit(Edit::Disambiguation(String::new())))]);
        assert!(does("name:x", &listen)[0].is_err());
        assert!(does("name:x", &Here { may_edit: false, ..Here { edit_mode: true, ..here(&names, &[]) } })[0].as_ref().unwrap_err().contains("edit_metadata"));
        // In the list: the name to filter by first, then a new one exactly as typed.
        let part = commandline::parse::<K>("alias:zee").parts.remove(0);
        let labels: Vec<String> = choices(&part, &[], &edit, &Known::default(), &Filters::default()).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["as Zee", "New name “zee”"]);
        assert_eq!(pc::commands_text::<K>("og:yes | alias:a b"), ["og:yes", "alias:a b"]);
    }

    #[test]
    fn sources_added_and_trashed() {
        use crate::account::ShapeField;
        let field = |name: &str, kind: &str, required| ShapeField { name: name.into(), kind: kind.into(), required };
        let shapes = [
            SourceShape { platform: "soundcloud".into(), kind: "artist".into(), id_field: None, fields: vec![field("artist_permalinks", "strs", true)] },
            SourceShape { platform: "custom".into(), kind: "artist".into(), id_field: None, fields: vec![field("label", "str", true), field("description", "str", false), field("url", "str", false)] },
            SourceShape { platform: "soundcloud".into(), kind: "track".into(), id_field: None, fields: vec![field("song_permalinks", "strs", true)] },
        ];
        let sources: Vec<Source> = serde_json::from_value(serde_json::json!([{ "id": Uuid::from_u128(7), "platform": "custom", "external_id": null, "snapshots": [{ "captured_at": "2026-01-01", "data": { "label": "vk" } }] }])).unwrap();
        let base = here(&[], &[]);
        let edit = Here { edit_mode: true, may_edit: true, may_source: true, shapes: &shapes, sources: &sources, ..base };
        let does = |text: &str, here: &Here| resolve(text, here, &Known::default()).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        assert_eq!(
            does("source:https://soundcloud.com/someone | -source:#1", &edit),
            [Ok(Does::Edit(Edit::Thing(Change::AddSource("soundcloud".into(), serde_json::json!({ "artist_permalinks": ["someone"] }))))), Ok(Does::Edit(Edit::Thing(Change::TrashSource(Uuid::from_u128(7)))))]
        );
        assert_eq!(does("source:custom vk, their page", &edit), [Ok(Does::Edit(Edit::Thing(Change::AddSource("custom".into(), serde_json::json!({ "label": "vk", "description": "their page" })))))]);
        // A platform's fields, typed after it (any `name:` is one of them).
        assert_eq!(does("source:soundcloud artist_permalinks:someone", &edit), [Ok(Does::Edit(Edit::Thing(Change::AddSource("soundcloud".into(), serde_json::json!({ "artist_permalinks": ["someone"] })))))]);
        assert!(does("-source:#2", &edit)[0].as_ref().unwrap_err().contains("has 1"));
        assert!(does("source:https://x.y", &Here { may_source: false, ..Here { edit_mode: true, may_edit: true, shapes: &shapes, ..here(&[], &[]) } })[0].as_ref().unwrap_err().contains("edit_sources"));
        // The list: the artist's platforms; Tab on one adds its first field.
        let line = commandline::parse::<K>("source:so");
        let rows = choices(&line.parts[0], &[], &edit, &Known::default(), &Filters::default());
        assert!(matches!(rows.as_slice(), [Row { choice: Choice::Raw(text, Some(next)), .. }] if text == "soundcloud" && next == "artist_permalinks:"));
    }

    #[test]
    fn extras_made_changed_linked_and_unlinked() {
        let own: Vec<crate::details::Extra> = serde_json::from_value(serde_json::json!([
            { "id": Uuid::from_u128(1), "name": "Glass Harbor (vocals)", "kind": "vocals", "description": "dry", "source": "the session", "is_restricted": false }
        ]))
        .unwrap();
        let library = vec![crate::catalog::model::Extra { id: Uuid::from_u128(2), name: "Stems".into(), kind: "stems".into(), is_restricted: false, hidden: false, links: vec![], files: 0, first_file: None, duration_ms: None, video: None, thumb_key: None }];
        let edit = Here { edit_mode: true, may_edit: true, extras: &own, library_extras: &library, ..here(&[], &[]) };
        let known = Known::default();
        let does = |text: &str, here: &Here| resolve(text, here, &known).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        let change = |text: &str| match does(text, &edit).remove(0) {
            Ok(Does::Edit(Edit::Thing(Change::Extra(c)))) => c,
            other => panic!("{text}: {other:?}"),
        };
        // A new one: its kind (other if not said), description, source.
        let c = change("extra:Demo take kind:clip description:the first one source:my phone");
        assert_eq!((c.id, c.name.as_str(), c.about), (None, "Demo take", Some(("clip".into(), "the first one".into(), "my phone".into()))));
        assert_eq!(change("extra:Demo take").about, Some(("other".into(), String::new(), String::new())));
        // Its own: what's typed on top of what it has.
        let c = change("extra:glass harbor (vocals) kind:stems");
        assert_eq!((c.id, c.linked, c.about), (Some(Uuid::from_u128(1)), true, Some(("stems".into(), "dry".into(), "the session".into()))));
        assert!(does("extra:Glass Harbor (vocals)", &edit)[0].as_ref().unwrap_err().contains("say what changes"));
        // The library's: linked here; changing it waits until it is.
        let c = change("extra:stems");
        assert_eq!((c.id, c.linked), (Some(Uuid::from_u128(2)), false));
        assert!(does("extra:stems kind:sample", &edit)[0].as_ref().unwrap_err().contains("link it first"));
        // Not on this artist anymore; files must be there; who sees it needs manage_access.
        assert_eq!(does("-extra:Glass Harbor (vocals)", &edit), [Ok(Does::Edit(Edit::Thing(Change::UnlinkExtra(Uuid::from_u128(1)))))]);
        assert!(does("extra:x file:/no/such/file.wav", &edit)[0].as_ref().unwrap_err().contains("no file"));
        assert!(does("extra:x visibility:everyone", &edit)[0].as_ref().unwrap_err().contains("manage_access"));
        let access = Here { may_access: true, ..edit };
        assert_eq!(match does("extra:x visibility:everyone", &access).remove(0) { Ok(Does::Edit(Edit::Thing(Change::Extra(c)))) => c.seen, _ => None }, Some(pc::Seen::Everyone));
        assert!(does("extra:stems visibility:artist", &access)[0].is_err(), "like the artist: a new one's");
        // Its arguments go with it; `source:` there is its own (not the artist's).
        let line = commandline::parse::<K>("extra:x source:a link | source:https://y.z");
        assert_eq!(line.parts.iter().map(|p| (p.key, p.arg)).collect::<Vec<_>>(), [(K::Extra, false), (K::Source, true), (K::Source, false)]);
        assert!(does("kind:vocals", &edit)[0].as_ref().unwrap_err().contains("goes after extra:"));
        assert!(does("extra:x", &here(&[], &[]))[0].as_ref().unwrap_err().contains("edit mode"));
    }

    #[test]
    fn notes_opened_made_and_taken_off() {
        let notes: Vec<crate::details::Note> = serde_json::from_value(serde_json::json!([
            { "id": Uuid::from_u128(3), "revision_id": Uuid::from_u128(4), "title": "", "body": "\nTour dates\nmore", "updated_at": "2026-01-01" },
        ]))
        .unwrap();
        let edit = Here { edit_mode: true, may_edit: true, notes: &notes, ..here(&[], &[]) };
        let known = Known::default();
        let does = |text: &str, here: &Here| resolve(text, here, &known).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        // No title: its first line is its name.
        assert_eq!(does("note:tour dates", &edit), [Ok(Does::Act(Act::Note(Some(Uuid::from_u128(3)), "Tour dates".into())))]);
        assert_eq!(does("note:Interview", &edit), [Ok(Does::Act(Act::Note(None, "Interview".into())))]);
        assert_eq!(does("note:paste", &edit), [Ok(Does::Act(Act::PasteNotes))]);
        assert_eq!(does("-note:Tour dates", &edit), [Ok(Does::Edit(Edit::Thing(Change::HideNote(Uuid::from_u128(3)))))]);
        assert!(does("-note:Nope", &edit)[0].is_err());
        assert!(does("note:Interview", &here(&[], &[]))[0].as_ref().unwrap_err().contains("edit mode"));
    }
}
