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
//!   apart from others with its name, `why:` is kept with the change.
//! * `source:<link>`, `source:custom label, description`, `source:<platform> field:…` adds a
//!   source (as the track editor's `source:`, with an artist's platforms; `edit_sources`),
//!   `-source:#2` puts its second one in the trash. Each is sent on its own: one failing
//!   leaves the rest (it stays staged, saying why).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::artist::Section;
use crate::commandline::{self, Keys};
use crate::account::SourceShape;
use crate::details::{AliasInfo, Source};
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
    Name,
    Disambiguation,
    Why,
    Source,
    /// One of a source's fields (after `source:<platform>`): its name is the part's.
    Field,
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
    (K::Name, &["name", "rename"]),
    (K::Disambiguation, &["disambiguation", "disamb", "dis", "tells"]),
    (K::Why, &["why", "reason"]),
    (K::Source, &["source"]),
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

    fn takes(_: K, _: K) -> bool {
        false
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
    /// A source added: its platform and data.
    AddSource(String, serde_json::Value),
    /// A source to the trash.
    TrashSource(Uuid),
}

impl Edit {
    /// Saved through the artist's form (one revision), not on its own.
    fn in_form(&self) -> bool {
        !matches!(self, Edit::AddSource(..) | Edit::TrashSource(_))
    }
}

/// A staged change: what it was typed as, what it does, and why it wasn't saved (if it
/// was tried).
struct Staged {
    text: String,
    label: String,
    edit: Edit,
    failed: Option<String>,
}

/// A source as listed: "soundcloud someone", "custom “vk”".
fn source_label(s: &Source) -> String {
    let data = s.snapshots.first().map(|x| x.data.clone()).unwrap_or_default();
    match (s.platform.as_str(), crate::ui::edit::editor::model::source_name(s.external_id.as_deref(), &data).as_str()) {
        (p, "") => p.to_string(),
        ("custom", name) => format!("custom “{name}”"),
        (p, name) => format!("{p} {name}"),
    }
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
    /// Staged as it is (a new name that's another one's but for case).
    Stage(String, String, Edit),
    /// The value being typed becomes this, as it is (not quoted: `#2`); on Tab, the next
    /// part to type after it.
    Raw(String, Option<String>),
}

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
    /// Sources being saved, each on its own: (the change, what the server said).
    sent: Arc<Mutex<Vec<(Staged, Result<(), String>)>>>,
    sending: usize,
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
            sent: Arc::default(),
            sending: 0,
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

/// `source:…` (and its fields) or `-source:#N`.
fn source(main: &commandline::Part<K>, fields: &[commandline::Part<K>], here: &Here) -> Result<(String, Does), String> {
    editing(here)?;
    if !here.may_source {
        return Err("sources need the edit_sources permission".into());
    }
    let value = main.value.trim();
    if main.minus {
        let n = match (main.hashes.as_slice(), value.is_empty()) {
            ([h], true) => h.parse::<usize>().ok().filter(|n| *n >= 1).ok_or_else(|| format!("#{h}: a source's place is a number from 1"))?,
            _ => return Err("-source:#N puts its Nth source in the trash (the list says which is which)".into()),
        };
        let s = here.sources.get(n - 1).ok_or_else(|| format!("it has {} sources", here.sources.len()))?;
        return Ok((format!("source #{n} ({}) → the trash", source_label(s)), Does::Edit(Edit::TrashSource(s.id))));
    }
    if value.is_empty() {
        return Err("source: a link, a platform and its fields, or custom and where it's from (custom vk, its page)".into());
    }
    let fields: Vec<(&str, &str)> = fields.iter().map(|p| (p.name(), p.value.trim())).collect();
    let typed = crate::ui::edit::sources::typed(value, &fields, "artist", here.shapes)?;
    Ok((format!("source: + {}", typed.shown), Does::Edit(Edit::AddSource(typed.platform, typed.data))))
}

/// Each command typed: its text, and what it does (or why it can't).
fn resolve(text: &str, here: &Here) -> Vec<Result<(String, Does), String>> {
    let line = commandline::parse::<K>(text);
    let mut out = Vec::new();
    for command in line.commands() {
        let main = &command[0];
        let value = main.value.trim();
        if main.minus && !matches!(main.key, K::Alias | K::Disambiguation | K::Source) {
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
            K::Sources if here.has_sources => Ok(("its sources".into(), Does::Act(Act::Reveal(Reveal::Sources)))),
            K::Sources => Err("it has no sources".into()),
            K::Notes if here.has_notes => Ok(("its notes".into(), Does::Act(Act::Reveal(Reveal::Notes)))),
            K::Notes => Err("it has no notes".into()),
            K::Name if value.is_empty() => Err("name: its new name".into()),
            K::Name => edit(format!("name → {value}"), Edit::Rename(value.to_string())),
            K::Disambiguation if value.is_empty() => edit("tells apart: nothing".into(), Edit::Disambiguation(String::new())),
            K::Disambiguation => edit(format!("tells apart: “{value}”"), Edit::Disambiguation(value.to_string())),
            K::Why => edit(format!("why: {value}"), Edit::Why(value.to_string())),
            K::Source => source(main, &command[1..], here),
            K::Field => Err(format!("{}: goes after source: and its platform (source:soundcloud artist_permalinks:…)", main.name())),
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
    if editing(here).is_ok() {
        for (key, label, sub) in [
            ("alias:", "New name (alias:)", "another name tracks can be credited as"),
            ("-alias:", "Take a name off (-alias:)", "if nothing's credited as it"),
            ("name:", "Rename (name:)", "its name"),
            ("disambiguation:", "Tells apart (disambiguation:)", "for two artists with the same name"),
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
fn choices(part: &commandline::Part<K>, group: &[commandline::Part<K>], here: &Here, filters: &Filters) -> Vec<Row> {
    let typed = part.value.trim().to_lowercase();
    let sources_here = editing(here).is_ok() && here.may_source;
    if part.minus && part.key == K::Source && sources_here {
        return here
            .sources
            .iter()
            .enumerate()
            .map(|(n, s)| Row { label: format!("#{} {}", n + 1, source_label(s)), sub: "to the trash".into(), choice: Choice::Raw(format!("#{}", n + 1), None) })
            .collect();
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
        K::Source if sources_here && part.hash.is_none() => {
            crate::ui::edit::sources::platform_choices(part.value.trim(), "", "artist", here.shapes).into_iter().map(|c| Row { label: c.label, sub: c.sub, choice: Choice::Raw(c.text, c.next) }).collect()
        }
        K::Field if sources_here => {
            let Some(owner) = group.first().filter(|o| o.key == K::Source) else { return Vec::new() };
            let given: Vec<&str> = group.iter().skip(1).chain([part]).map(|p| p.name()).collect();
            crate::ui::edit::sources::field_choices(&owner.value, part.name(), part.value.trim(), &given, "artist", here.shapes)
                .into_iter()
                .map(|c| Row { label: c.label, sub: c.sub, choice: Choice::Raw(c.text, c.next) })
                .collect()
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
        for does in resolve(&self.cl.text, here).into_iter().flatten() {
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
            Some(last) => choices(last, line.last_goes_with(), here, filters),
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
        if can_edit && (self.has_changes() || (focused && !self.cl.text.trim().is_empty())) && !shell.palette_open && !shell.modal_was_open {
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
            match &row.choice {
                Choice::Raw(text, next) => {
                    if let Some(part) = line.parts.last() {
                        let text = (*text != part.value).then_some(text.as_str());
                        self.cl.take(&ctx, part.value_start, part.end, text, next.as_deref().filter(|_| tab));
                    }
                }
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
                Choice::Stage(text, label, edit) => {
                    // Only this command: the rest of the line stays to go on with.
                    if let Some(part) = line.parts.last() {
                        let start = self.cl.text[..part.value_start].rfind(|c: char| c.is_whitespace() || c == '|').map_or(0, |w| w + 1);
                        let start = self.cl.text[..start].trim_end().trim_end_matches('|').trim_end().len();
                        self.cl.text.truncate(start);
                    }
                    self.staged.push(Staged { text: text.clone(), label: label.clone(), edit: edit.clone(), failed: None });
                    self.cl.cursor_to_end(&ctx);
                    enter = false;
                }
            }
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here);
            let texts = commands_text(&self.cl.text);
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
            let shown: Vec<Result<(String, Option<String>), String>> = resolve(&self.cl.text, here).into_iter().map(|r| r.map(|(label, _)| (label, None))).collect();
            cl::would_do(ui, &shown, "Nothing to do by that name");
        }
        for (_, _, name) in &self.asking {
            ui.label(RichText::new(format!("Checking where “{name}” is credited…")).color(P.faint).small());
        }
        if let Some((name, uses)) = &self.blocked {
            artist_form::uses_links(ui, shell, name, uses);
        }
        if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(P.muted).small());
        }
        if self.sending > 0 {
            ui.label(RichText::new(format!("Saving {} source change(s)…", self.sending)).color(P.faint).small());
        }
        for s in self.staged.iter().filter(|s| s.failed.is_some()) {
            ui.label(RichText::new(format!("Not saved: {}: {}", s.text, s.failed.as_deref().unwrap_or(""))).color(P.danger).small());
        }
        // The changes staged (outside edit mode too: kept, saved in it).
        if !self.staged.is_empty() {
            ui.add_space(4.0);
            let chips: Vec<cl::Chip> = self.staged.iter().map(|s| cl::Chip { text: &s.text, label: &s.label, warning: s.failed.as_deref() }).collect();
            if let Some(n) = cl::chips(ui, &chips, true) {
                self.staged.remove(n);
            }
            let text = if can_edit { "Ctrl+Enter saves them · Backspace on an empty line takes the last back" } else { "Not saved: they're saved in edit mode (e, then Ctrl+Enter)" };
            ui.label(RichText::new(text).color(P.faint).small());
        }
        if self.has_changes() {
            shell.form_dirty = true;
        }
        // Something to look at: the keys go there.
        if act.is_some() {
            ctx.memory_mut(|m| m.surrender_focus(self.cl.id()));
        }
        act
    }

    /// What the server said about the sources sent (one failing: staged again, saying why),
    /// and where the names to take off are credited: nowhere, staged; somewhere, said.
    fn answers(&mut self, shell: &mut Shell) {
        let sent = std::mem::take(&mut *self.sent.lock().unwrap());
        if !sent.is_empty() {
            self.sending -= sent.len().min(self.sending);
            if let Some(id) = self.artist {
                shell.artist_details.refresh(id);
            }
            for (mut change, result) in sent {
                if let Err(err) = result {
                    change.failed = Some(err);
                    self.staged.push(change);
                }
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
                            Edit::AddSource(..) | Edit::TrashSource(_) => {}
                        }
                    }
                    form.save(shell);
                    shell.artist_forms.insert(id, form);
                    notices.push("Saving… (the artist's form below says how it went)");
                }
            }
        }
        for change in others {
            let (method, path, body) = match &change.edit {
                Edit::AddSource(platform, data) => (reqwest::Method::POST, format!("/api/artists/{id}/sources"), Some(serde_json::json!({ "platform": platform, "data": data }))),
                Edit::TrashSource(source) => (reqwest::Method::DELETE, format!("/api/sources/{source}"), None),
                _ => continue,
            };
            self.sending += 1;
            let (api, ctx, sent) = (shell.api.clone(), shell.ctx.clone(), self.sent.clone());
            shell.runtime.spawn(async move {
                let result = api.send_json::<serde_json::Value>(method, &path, body).await.map(|_| ()).map_err(|e| e.message);
                sent.lock().unwrap().push((change, result));
                ctx.request_repaint();
            });
        }
        self.notice = (!notices.is_empty()).then(|| notices.join(" "));
    }
}

/// Each command's text, as typed (to put back in the line).
fn commands_text(text: &str) -> Vec<String> {
    let line = commandline::parse::<K>(text);
    line.commands()
        .map(|c| {
            let start = text[..c[0].value_start].rfind(|ch: char| ch.is_whitespace() || ch == '|').map_or(0, |w| w + 1);
            text[start..c[c.len() - 1].end].trim().to_string()
        })
        .collect()
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
        Here { own: "Zed", names, og_count: 2, sections, has_sources: true, has_notes: false, edit_mode: false, may_edit: false, may_collective: false, aliases: &[], disambiguation: "", sources: &[], may_source: false, shapes: &[] }
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

    #[test]
    fn changes_are_edit_modes() {
        let names = [(None, 5), (Some("Zee".to_string()), 3)];
        let aliases = [AliasInfo { id: Uuid::from_u128(1), name: "Zee".into() }, AliasInfo { id: Uuid::from_u128(2), name: "Unused".into() }];
        let listen = Here { aliases: &aliases, ..here(&names, &[]) };
        let edit = Here { edit_mode: true, may_edit: true, ..Here { aliases: &aliases, ..here(&names, &[]) } };
        let does = |text: &str, here: &Here| resolve(text, here).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
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
        let labels: Vec<String> = choices(&part, &[], &edit, &Filters::default()).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["as Zee", "New name “zee”"]);
        assert_eq!(commands_text("og:yes | alias:a b"), ["og:yes", "alias:a b"]);
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
        let does = |text: &str, here: &Here| resolve(text, here).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
        assert_eq!(
            does("source:https://soundcloud.com/someone | -source:#1", &edit),
            [Ok(Does::Edit(Edit::AddSource("soundcloud".into(), serde_json::json!({ "artist_permalinks": ["someone"] })))), Ok(Does::Edit(Edit::TrashSource(Uuid::from_u128(7))))]
        );
        assert_eq!(does("source:custom vk, their page", &edit), [Ok(Does::Edit(Edit::AddSource("custom".into(), serde_json::json!({ "label": "vk", "description": "their page" }))))]);
        // A platform's fields, typed after it (any `name:` is one of them).
        assert_eq!(does("source:soundcloud artist_permalinks:someone", &edit), [Ok(Does::Edit(Edit::AddSource("soundcloud".into(), serde_json::json!({ "artist_permalinks": ["someone"] }))))]);
        assert!(does("-source:#2", &edit)[0].as_ref().unwrap_err().contains("has 1"));
        assert!(does("source:https://x.y", &Here { may_source: false, ..Here { edit_mode: true, may_edit: true, shapes: &shapes, ..here(&[], &[]) } })[0].as_ref().unwrap_err().contains("edit_sources"));
        // The list: the artist's platforms; Tab on one adds its first field.
        let line = commandline::parse::<K>("source:so");
        let rows = choices(&line.parts[0], &[], &edit, &Filters::default());
        assert!(matches!(rows.as_slice(), [Row { choice: Choice::Raw(text, Some(next)), .. }] if text == "soundcloud" && next == "artist_permalinks:"));
    }
}
