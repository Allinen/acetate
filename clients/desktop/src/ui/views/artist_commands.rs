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
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::artist::Section;
use crate::commandline::{self, Keys};
use crate::account::SourceShape;
use crate::details::{AliasInfo, Source};
use crate::ui::edit::extras::{GroupChoice, Groups};
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
    /// Its extras (taken off ones too), every extra in the library, and whether who sees
    /// one can be said here (`manage_access`).
    pub extras: &'a [crate::details::Extra],
    pub library_extras: &'a [crate::catalog::model::Extra],
    pub may_access: bool,
    /// Its notes (taken off ones too).
    pub notes: &'a [crate::details::Note],
}

/// What the commandline knows besides the page: extras and notes chosen in its list (by
/// name, lowercase), and the groups (to say who sees an extra; None: not asked for yet).
#[derive(Default)]
pub struct Known {
    pub pins: std::collections::HashMap<(Pinned, String), Uuid>,
    pub groups: Option<Vec<GroupChoice>>,
}

/// What a name chosen in the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pinned {
    Extra,
    Note,
}

impl Known {
    fn pinned(&self, kind: Pinned, name: &str) -> Option<Uuid> {
        self.pins.get(&(kind, name.to_lowercase())).copied()
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
    Extra(ExtraChange),
    /// An extra not on this artist anymore.
    UnlinkExtra(Uuid),
    /// A note taken off (kept).
    HideNote(Uuid),
}

/// An extra: a new one, or one there already (linked here or not yet), and what changes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExtraChange {
    /// None: a new one.
    pub id: Option<Uuid>,
    pub name: String,
    /// Linked to this artist already.
    pub linked: bool,
    pub files: Vec<std::path::PathBuf>,
    /// One there already: its name, kind, description and source as they'll be (its own,
    /// with what was typed on top), when any of them changes. A new one: all of them.
    pub about: Option<(String, String, String)>,
    pub seen: Option<Seen>,
}

/// Who sees an extra.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Everyone,
    /// Like the artist (a new one: the server's default).
    LikeArtist,
    /// Admins and these groups.
    Groups(Vec<Uuid>),
}

impl Edit {
    /// Saved through the artist's form (one revision), not on its own.
    fn in_form(&self) -> bool {
        !matches!(self, Edit::AddSource(..) | Edit::TrashSource(_) | Edit::Extra(_) | Edit::UnlinkExtra(_) | Edit::HideNote(_))
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
    /// An extra or a note chosen: its name in the line, and that one from now on.
    Pin(Pinned, String, Uuid),
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
    sent: Arc<Mutex<Vec<(Staged, Result<Option<String>, String>)>>>,
    sending: usize,
    known: Known,
    groups: Groups,
    /// Files chosen in the dialog (`file:choose`), to put in the commandline.
    chosen: Arc<Mutex<Vec<std::path::PathBuf>>>,
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
            sent: Arc::default(),
            sending: 0,
            known: Known::default(),
            groups: Groups::default(),
            chosen: Arc::default(),
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

/// An extra by name: one chosen in the list, else this artist's, else the library's (one
/// of each name; several: chosen in the list). None: there's none (a new one).
fn extra_named(here: &Here, known: &Known, name: &str) -> Result<Option<(Uuid, bool)>, String> {
    let linked = |id: Uuid| here.extras.iter().any(|e| e.id == id && !e.hidden);
    if let Some(id) = known.pinned(Pinned::Extra, name) {
        return Ok(Some((id, linked(id))));
    }
    let own: Vec<Uuid> = here.extras.iter().filter(|e| !e.hidden && e.name.eq_ignore_ascii_case(name)).map(|e| e.id).collect();
    match own.as_slice() {
        [id] => return Ok(Some((*id, true))),
        [_, _, ..] => return Err(format!("several of its extras are called “{name}”: choose one in the list")),
        [] => {}
    }
    let library: Vec<Uuid> = here.library_extras.iter().filter(|e| !e.hidden && e.name.eq_ignore_ascii_case(name)).map(|e| e.id).collect();
    match library.as_slice() {
        [id] => Ok(Some((*id, linked(*id)))),
        [_, _, ..] => Err(format!("several extras in the library are called “{name}”: choose one in the list")),
        [] => Ok(None),
    }
}

/// A path typed: `~/` is the home folder.
fn path_of(typed: &str) -> std::path::PathBuf {
    match typed.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => std::path::PathBuf::from(typed),
    }
}

/// Who sees an extra, as typed: everyone, like the artist (`new` only), or groups by name.
fn seen(value: &str, here: &Here, known: &Known, new: bool) -> Result<Seen, String> {
    if !here.may_access {
        return Err("who sees an extra: that needs the manage_access permission".into());
    }
    match value.to_lowercase().as_str() {
        "" => Err("visibility: everyone, artist (like the artist), or groups (Group A, Group B)".into()),
        "everyone" | "all" | "public" => Ok(Seen::Everyone),
        "artist" | "like artist" | "like the artist" | "default" if new => Ok(Seen::LikeArtist),
        "artist" | "like artist" | "like the artist" | "default" => Err("for one there already: everyone, or groups".into()),
        _ => {
            let Some(groups) = known.groups.as_ref() else { return Err("loading the groups…".into()) };
            let mut ids = Vec::new();
            for name in value.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let g = groups.iter().find(|g| g.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no group “{name}”"))?;
                ids.push(g.id);
            }
            Ok(Seen::Groups(ids))
        }
    }
}

/// `extra:Name` and its arguments, or `-extra:Name`.
fn extra(main: &commandline::Part<K>, args: &[commandline::Part<K>], here: &Here, known: &Known) -> Result<(String, Does), String> {
    editing(here)?;
    let name = main.value.trim();
    if main.minus {
        let found = here.extras.iter().find(|e| !e.hidden && (known.pinned(Pinned::Extra, name) == Some(e.id) || e.name.eq_ignore_ascii_case(name)));
        let e = found.ok_or_else(|| if name.is_empty() { "-extra:Name: not on this artist anymore".to_string() } else { format!("it has no extra “{name}”") })?;
        return Ok((format!("“{}” not on this artist (it stays, wherever else it's linked)", e.name), Does::Edit(Edit::UnlinkExtra(e.id))));
    }
    if name.is_empty() {
        return Err("extra: its name (the list has this artist's, and the library's)".into());
    }
    let found = extra_named(here, known, name)?;
    let (mut kind, mut description, mut source, mut visibility, mut files) = (None, None, None, None, Vec::new());
    let mut said = Vec::new();
    for a in args {
        let v = a.value.trim();
        match a.key {
            K::Kind if v.is_empty() => return Err("kind: vocals, stems, sample… (any word)".into()),
            K::Kind => kind = Some(v.to_string()),
            K::File => match v {
                "" => return Err("file: a path (Tab completes it), paste or choose".into()),
                "paste" => said.push("the files copied (read on Enter)".to_string()),
                "choose" => said.push("files chosen in the dialog (on Enter)".to_string()),
                _ => {
                    let path = path_of(v);
                    if path.is_dir() {
                        return Err(format!("{v}: a folder (its files one by one: Tab lists them)"));
                    }
                    if !path.is_file() {
                        return Err(format!("no file at {v}"));
                    }
                    files.push(path);
                }
            },
            K::Description => description = Some(v.to_string()),
            K::Source => source = Some(v.to_string()),
            K::Visibility => visibility = Some(seen(v, here, known, found.is_none())?),
            _ => {}
        }
    }
    if !files.is_empty() {
        said.push(crate::ui::edit::extras::files_text(files.len()));
    }
    let seen_text = |s: &Seen| match s {
        Seen::Everyone => "seen by everyone".to_string(),
        Seen::LikeArtist => "seen like the artist".to_string(),
        Seen::Groups(g) => format!("seen by admins and {} group(s)", g.len()),
    };
    let change = match found {
        None => {
            let kind = kind.unwrap_or_else(|| "other".to_string());
            let mut label = format!("+ new extra “{name}” ({kind})");
            for text in said.iter().cloned().chain(visibility.as_ref().map(seen_text)) {
                label.push_str(&format!(" · {text}"));
            }
            let about = Some((kind, description.unwrap_or_default(), source.unwrap_or_default()));
            (label, ExtraChange { id: None, name: name.to_string(), linked: false, files, about, seen: visibility })
        }
        Some((id, linked)) => {
            let changes_about = kind.is_some() || description.is_some() || source.is_some();
            let own = here.extras.iter().find(|e| e.id == id);
            if !linked && (changes_about || visibility.is_some()) {
                return Err(format!("“{name}” isn't on this artist: link it first (extra:{name}), then change it; or on its own page"));
            }
            let about = match own {
                Some(e) if changes_about => Some((kind.clone().unwrap_or_else(|| e.kind.clone()), description.clone().unwrap_or_else(|| e.description.clone()), source.clone().unwrap_or_else(|| e.source.clone()))),
                _ => None,
            };
            let shown = own.map(|e| e.name.clone()).or_else(|| here.library_extras.iter().find(|e| e.id == id).map(|e| e.name.clone())).unwrap_or_else(|| name.to_string());
            let mut what: Vec<String> = Vec::new();
            if !linked {
                what.push("linked here".into());
            }
            if let Some(k) = &kind {
                what.push(format!("kind {k}"));
            }
            if description.is_some() {
                what.push("its description".into());
            }
            if source.is_some() {
                what.push("where it came from".into());
            }
            what.extend(said.iter().map(|t| format!("+ {t}")));
            what.extend(visibility.as_ref().map(seen_text));
            if what.is_empty() {
                return Err(format!("“{shown}” is on this artist already: say what changes (kind:, file:, description:, source:, visibility:)"));
            }
            (format!("extra “{shown}”: {}", what.join(" · ")), ExtraChange { id: Some(id), name: shown, linked, files, about, seen: visibility })
        }
    };
    Ok((change.0, Does::Edit(Edit::Extra(change.1))))
}

/// A note's title as listed (no title: its first line).
fn note_title(n: &crate::details::Note) -> String {
    if n.title.trim().is_empty() { n.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string() } else { n.title.clone() }
}

/// `note:Title` (that note, else a new one), `note:paste`, or `-note:Title`.
fn note(main: &commandline::Part<K>, here: &Here, known: &Known) -> Result<(String, Does), String> {
    editing(here)?;
    let title = main.value.trim();
    let found = here.notes.iter().filter(|n| !n.hidden).find(|n| known.pinned(Pinned::Note, title) == Some(n.id) || note_title(n).eq_ignore_ascii_case(title));
    if main.minus {
        let n = found.ok_or_else(|| if title.is_empty() { "-note:Title takes that note off (kept)".to_string() } else { format!("it has no note “{title}”") })?;
        return Ok((format!("note “{}” taken off (kept: “Taken off” brings it back)", note_title(n)), Does::Edit(Edit::HideNote(n.id))));
    }
    match (title, found) {
        ("", _) => Err("note: a title (a new note), one of its notes (the list), or paste (copied text files)".into()),
        (t, None) if t.eq_ignore_ascii_case("paste") => Ok(("the text files copied: a note each (listed first)".into(), Does::Act(Act::PasteNotes))),
        (_, Some(n)) => Ok((format!("the note “{}”, to edit", note_title(n)), Does::Act(Act::Note(Some(n.id), note_title(n))))),
        (t, None) => Ok((format!("a new note “{t}”"), Does::Act(Act::Note(None, t.to_string())))),
    }
}

/// Paths starting with what's typed (a folder's entries; folders end in `/`).
fn paths_like(typed: &str) -> Vec<String> {
    let Some(cut) = typed.rfind('/') else { return Vec::new() };
    let (folder, start) = (&typed[..=cut], typed[cut + 1..].to_lowercase());
    let Ok(entries) = std::fs::read_dir(path_of(folder)) else { return Vec::new() };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !start.starts_with('.') {
                return None;
            }
            name.to_lowercase().starts_with(&start).then(|| format!("{folder}{name}{}", if e.path().is_dir() { "/" } else { "" }))
        })
        .collect();
    out.sort();
    out.truncate(20);
    out
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
            K::Extra => extra(main, &command[1..], here, known),
            K::Note => note(main, here, known),
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
    let sources_here = editing(here).is_ok() && here.may_source;
    if part.minus && part.key == K::Source && sources_here {
        return here
            .sources
            .iter()
            .enumerate()
            .map(|(n, s)| Row { label: format!("#{} {}", n + 1, source_label(s)), sub: "to the trash".into(), choice: Choice::Raw(format!("#{}", n + 1), None) })
            .collect();
    }
    if part.key == K::Note && editing(here).is_ok() {
        let mut rows: Vec<Row> = here
            .notes
            .iter()
            .filter(|n| !n.hidden && note_title(n).to_lowercase().contains(&typed))
            .map(|n| Row { label: format!("{}{}", if part.minus { "− " } else { "" }, note_title(n)), sub: if part.minus { "taken off (kept)".into() } else { "edit it".into() }, choice: Choice::Pin(Pinned::Note, note_title(n), n.id) })
            .collect();
        if !part.minus {
            if "paste".starts_with(&typed) {
                rows.push(Row { label: "paste".into(), sub: "the text files copied in a file manager: a note each".into(), choice: Choice::Raw("paste".into(), None) });
            }
            let value = part.value.trim();
            if !value.is_empty() && !value.eq_ignore_ascii_case("paste") && !here.notes.iter().any(|n| !n.hidden && note_title(n).eq_ignore_ascii_case(value)) {
                rows.push(Row { label: format!("New note “{value}”"), sub: "its text next".into(), choice: Choice::Raw(value.to_string(), None) });
            }
        }
        return rows;
    }
    if part.minus && part.key == K::Extra && editing(here).is_ok() {
        return here
            .extras
            .iter()
            .filter(|e| !e.hidden && e.name.to_lowercase().contains(&typed))
            .map(|e| Row { label: format!("− {}", e.name), sub: format!("{} · not on this artist anymore", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) })
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
        K::Extra if editing(here).is_ok() => {
            let mut rows: Vec<Row> = here
                .extras
                .iter()
                .filter(|e| !e.hidden && e.name.to_lowercase().contains(&typed))
                .map(|e| Row { label: e.name.clone(), sub: format!("{} · on this artist: change it", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) })
                .collect();
            let linked = |id: Uuid| here.extras.iter().any(|e| e.id == id);
            rows.extend(
                here.library_extras
                    .iter()
                    .filter(|e| !e.hidden && !linked(e.id) && !typed.is_empty() && e.name.to_lowercase().contains(&typed))
                    .take(12)
                    .map(|e| Row { label: e.name.clone(), sub: format!("{} · in the library: linked here", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) }),
            );
            let value = part.value.trim();
            if !value.is_empty() {
                rows.push(Row { label: format!("New extra “{value}”"), sub: "made when it's saved · Tab: its kind".into(), choice: Choice::Raw(value.to_string(), Some("kind:".into())) });
            }
            rows
        }
        K::Kind if group.first().is_some_and(|g| g.key == K::Extra) => {
            crate::ui::edit::extras::KINDS.iter().filter(|k| k.starts_with(&typed)).map(|k| Row { label: k.to_string(), sub: "a kind (any word works)".into(), choice: Choice::Raw(k.to_string(), None) }).collect()
        }
        K::File if group.first().is_some_and(|g| g.key == K::Extra) => {
            let mut rows: Vec<Row> = [("paste", "the files copied in a file manager"), ("choose", "the file dialog")]
                .into_iter()
                .filter(|(w, _)| w.starts_with(&typed))
                .map(|(w, sub)| Row { label: w.into(), sub: sub.into(), choice: Choice::Raw(w.into(), None) })
                .collect();
            let value = part.value.trim();
            if value.starts_with('/') || value.starts_with('~') {
                rows.extend(paths_like(value).into_iter().map(|p| Row { sub: if p.ends_with('/') { "a folder".into() } else { "a file".into() }, label: p.clone(), choice: Choice::Raw(p, None) }));
            }
            rows
        }
        K::Visibility if group.first().is_some_and(|g| g.key == K::Extra) && here.may_access => {
            let mut rows: Vec<Row> = [("everyone", "seen by everyone"), ("artist", "like the artist (a new one)")]
                .into_iter()
                .filter(|(w, _)| w.starts_with(&typed))
                .map(|(w, sub)| Row { label: w.into(), sub: sub.into(), choice: Choice::Raw(w.into(), None) })
                .collect();
            rows.extend(known.groups.iter().flatten().filter(|g| g.name != "admin" && g.name.to_lowercase().starts_with(&typed)).map(|g| Row { label: g.name.clone(), sub: "admins and this group (more: Group A, Group B)".into(), choice: Choice::Raw(g.name.clone(), None) }));
            rows
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
        if here.may_access && self.known.groups.is_none() && commandline::parse::<K>(&self.cl.text).parts.iter().any(|p| p.key == K::Visibility) {
            crate::ui::edit::extras::ask_groups(shell, &self.groups);
            if let Some(Ok(groups)) = &*self.groups.lock().unwrap()
                && !groups.is_empty()
            {
                self.known.groups = Some(groups.clone());
            }
        }
        // Files chosen in the dialog (`file:choose`): their paths in the line.
        let chosen = std::mem::take(&mut *self.chosen.lock().unwrap());
        if !chosen.is_empty() {
            for path in chosen {
                self.cl.text = format!("{} file:{}", self.cl.text.trim_end(), path.display());
            }
            self.cl.place_cursor = true;
            self.cl.focus = true;
        }
        // `/` (the app's key, by the character typed) puts the cursor here; the page just
        // shown: here too, unless another field has it.
        shell.slash_target = Some(self.cl.id());
        let pass = ctx.cumulative_pass_nr();
        if pass > self.last_pass + 1 {
            self.cl.focus = true;
            self.cl.gently = true;
        }
        self.last_pass = pass;
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
            match &row.choice {
                Choice::Pin(kind, name, id) => {
                    self.known.pins.insert((*kind, name.to_lowercase()), *id);
                    if let Some(part) = line.parts.last() {
                        let text = (*name != part.value).then(|| commandline::quoted::<K>(name));
                        self.cl.take(&ctx, part.value_start, part.end, text.as_deref(), None);
                    }
                }
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
        // `file:paste` and `file:choose`: the paths in the line first (what's staged is what
        // the line says).
        if enter && let Err(err) = self.expand_files(shell) {
            self.notice = Some(err);
            enter = false;
            save = false;
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here, &self.known);
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
            let shown: Vec<Result<(String, Option<String>), String>> = resolve(&self.cl.text, here, &self.known).into_iter().map(|r| r.map(|(label, _)| (label, None))).collect();
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
            let text = if can_edit { "Ctrl+Enter saves them · Backspace on an empty commandline takes the last back" } else { "Not saved: they're saved in edit mode (e, then Ctrl+Enter)" };
            ui.label(RichText::new(text).color(P.faint).small());
        }
        if self.has_changes() {
            shell.form_dirty = true;
        }
        // Something to look at (a section, the note editor): the keys go there, and the
        // commandline doesn't take the cursor back. Edit mode switched: it stays here.
        if act.as_ref().is_some_and(|a| *a != Act::ToggleEdit) {
            self.cl.focus = false;
            self.cl.place_cursor = false;
            ctx.memory_mut(|m| m.surrender_focus(self.cl.id()));
        }
        act
    }

    /// `file:paste` becomes the copied files' paths (`file:/…` each); `file:choose` opens the
    /// dialog and comes out of the line (its paths are put in when chosen: Enter again).
    /// Err: why not now (no files copied; the dialog is open).
    fn expand_files(&mut self, shell: &Shell) -> Result<(), String> {
        let line = commandline::parse::<K>(&self.cl.text);
        let mut choosing = false;
        for part in line.parts.iter().rev().filter(|p| p.key == K::File && p.arg) {
            let value = part.value.trim();
            if value != "paste" && value != "choose" {
                continue;
            }
            let start = self.cl.text[..part.value_start].rfind(char::is_whitespace).map_or(0, |w| w + 1);
            let with = if value == "paste" {
                let files = crate::ui::files::clipboard_files();
                if files.is_empty() {
                    return Err("There are no files in the clipboard (copy them in a file manager first).".into());
                }
                files.iter().map(|f| format!("file:{}", f.display())).collect::<Vec<_>>().join(" ")
            } else {
                choosing = true;
                String::new()
            };
            self.cl.text.replace_range(start..part.end, &with);
        }
        if choosing {
            crate::ui::files::choose(shell, "Files for the extra", ("Any file", &[]), true, &self.chosen);
            return Err("Choose the files: their paths go in the commandline, then Enter.".into());
        }
        Ok(())
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
            shell.act(crate::ui::Action::RefreshLibrary);
            let mut said = Vec::new();
            for (mut change, result) in sent {
                match result {
                    Ok(Some(text)) => said.push(text),
                    Ok(None) => {}
                    Err(err) => {
                        change.failed = Some(err);
                        self.staged.push(change);
                    }
                }
            }
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
                            Edit::AddSource(..) | Edit::TrashSource(_) | Edit::Extra(_) | Edit::UnlinkExtra(_) | Edit::HideNote(_) => {}
                        }
                    }
                    form.save(shell);
                    shell.artist_forms.insert(id, form);
                    notices.push("Saving… (the artist's form below says how it went)");
                }
            }
        }
        for change in others {
            self.sending += 1;
            let (api, ctx, sent) = (shell.api.clone(), shell.ctx.clone(), self.sent.clone());
            shell.runtime.spawn(async move {
                let result = send(&api, &ctx, id, &change.edit).await;
                sent.lock().unwrap().push((change, result));
                ctx.request_repaint();
            });
        }
        self.notice = (!notices.is_empty()).then(|| notices.join(" "));
    }
}

/// A change sent on its own (a source, an extra). Ok: what's worth saying, if anything.
async fn send(api: &crate::api::Api, ctx: &egui::Context, artist: Uuid, edit: &Edit) -> Result<Option<String>, String> {
    use reqwest::Method;
    use serde_json::{json, Value};
    let call = |method: Method, path: String, body: Option<Value>| async move { api.send_json::<Value>(method, &path, body).await.map_err(|e| e.message) };
    let link = crate::ui::edit::extras::Owner::Artist(artist).link();
    match edit {
        Edit::AddSource(platform, data) => call(Method::POST, format!("/api/artists/{artist}/sources"), Some(json!({ "platform": platform, "data": data }))).await.map(|_| None),
        Edit::TrashSource(source) => call(Method::DELETE, format!("/api/sources/{source}"), None).await.map(|_| None),
        Edit::UnlinkExtra(extra) => call(Method::POST, format!("/api/extras/{extra}/links"), Some(json!({ "link": link, "undo": true }))).await.map(|_| None),
        Edit::HideNote(note) => call(Method::POST, format!("/api/notes/{note}/hidden"), Some(json!({ "hidden": true }))).await.map(|_| None),
        Edit::Extra(c) => {
            let access = |seen: &Seen| match seen {
                Seen::Everyone => Some(json!({ "restricted": false })),
                Seen::Groups(groups) => Some(json!({ "restricted": true, "groups": groups })),
                Seen::LikeArtist => None,
            };
            let id = match c.id {
                Some(id) => {
                    if !c.linked {
                        call(Method::POST, format!("/api/extras/{id}/links"), Some(json!({ "link": link }))).await?;
                    }
                    if let Some((kind, description, source)) = &c.about {
                        call(Method::PUT, format!("/api/extras/{id}"), Some(json!({ "name": c.name, "kind": kind, "description": description, "source": source }))).await?;
                    }
                    if let Some(body) = c.seen.as_ref().and_then(access) {
                        call(Method::PUT, format!("/api/extras/{id}/access"), Some(body)).await?;
                    }
                    id.to_string()
                }
                None => {
                    let (kind, description, source) = c.about.clone().unwrap_or_default();
                    let mut body = json!({ "name": c.name, "kind": kind, "links": [link], "description": description, "source": source });
                    if let Some(a) = c.seen.as_ref().and_then(access) {
                        body["access"] = a;
                    }
                    let made = call(Method::POST, "/api/extras".into(), Some(body)).await?;
                    made["id"].as_str().unwrap_or_default().to_string()
                }
            };
            if c.files.is_empty() {
                return Ok(None);
            }
            let progress = std::sync::atomic::AtomicUsize::new(0);
            let name = c.name.clone();
            crate::ui::edit::extras::upload_all(api, &id, &c.files, &progress, ctx, |n| format!("“{name}”: {} added.", crate::ui::edit::extras::files_text(n))).await.map(Some)
        }
        _ => Ok(None),
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
        Here { own: "Zed", names, og_count: 2, sections, has_sources: true, has_notes: false, edit_mode: false, may_edit: false, may_collective: false, aliases: &[], disambiguation: "", sources: &[], may_source: false, shapes: &[], extras: &[], library_extras: &[], may_access: false, notes: &[] }
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
        let does = |text: &str, here: &Here| resolve(text, here, &Known::default()).into_iter().map(|r| r.map(|(_, d)| d)).collect::<Vec<_>>();
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
            Ok(Does::Edit(Edit::Extra(c))) => c,
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
        assert_eq!(does("-extra:Glass Harbor (vocals)", &edit), [Ok(Does::Edit(Edit::UnlinkExtra(Uuid::from_u128(1))))]);
        assert!(does("extra:x file:/no/such/file.wav", &edit)[0].as_ref().unwrap_err().contains("no file"));
        assert!(does("extra:x visibility:everyone", &edit)[0].as_ref().unwrap_err().contains("manage_access"));
        let access = Here { may_access: true, ..edit };
        assert_eq!(match does("extra:x visibility:everyone", &access).remove(0) { Ok(Does::Edit(Edit::Extra(c))) => c.seen, _ => None }, Some(Seen::Everyone));
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
        assert_eq!(does("-note:Tour dates", &edit), [Ok(Does::Edit(Edit::HideNote(Uuid::from_u128(3))))]);
        assert!(does("-note:Nope", &edit)[0].is_err());
        assert!(does("note:Interview", &here(&[], &[]))[0].as_ref().unwrap_err().contains("edit mode"));
    }
}
