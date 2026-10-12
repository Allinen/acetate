//! The album page's commandline (a `ui::edit::commandline`), declared once (`SCHEMA`, the
//! language is `crate::commandline::schema`'s; what it shares with the artist page's is in
//! `page_commands`). It's there in listen mode too, for listening and moving around.
//!
//! * `play` plays the album, `play:3` (or a title) from its third track; `queue` puts it at
//!   the end of the queue, `queue:next` before what's queued. `star` (`star:no`, `-star`)
//!   and `rating:3.5` (`-rating` clears it) are yours, saved at once.
//! * `tracks:words` filters its tracks (as the library's filter does: `tracks:year:2019`),
//!   `tracks:` shows them all; `og:` / `og:yes` / `og:no` sets the Original files chip. Live
//!   while they're typed, kept with Enter; Esc puts back what they were.
//! * `credits`, `covers`, `sources`, `notes`, `extras`, `history` (and `merges` in edit
//!   mode) open that part of the page and scroll to it; a folded one that's open is closed.
//! * `edit:3` opens the track editor for its third track (a quick fix, listen mode too, as
//!   Ctrl+E); `edit:all` or `edit:2 5` for several, marked (edit mode).
//! * Plain words find these too, its artists and groupings (to go to), and copying its link
//!   or a share link; merging it in edit mode.
//!
//! Edit mode (and `edit_metadata`) adds changes, staged as chips and saved together with
//! Ctrl+Enter. Outside edit mode they're not suggested, and typed they say why not.
//!
//! * `title:`, `kind:`, `date:` (`-date` clears it), `why:` (kept with those and the
//!   artists: alone it has nothing to go with, and saving says so), and its artists:
//!   `artist:Name role:… as:… n:2` credits one (or, credited already, changes its role, the
//!   name it's credited under, its place), `feat:Name` as featured, `-artist:Name` takes one
//!   off, `-artist` all of them. Saved through the album's form: one revision, the form
//!   says how it went.
//! * `cover:paste` (an image or an image file copied), `cover:choose` (the file dialog: its
//!   path goes in the line), `cover:/path`, `cover:<album or track>` (a copy of its cover)
//!   add one; `cover:3` makes the third the main one, `cover:3 n:2` moves it; `-cover:2`
//!   puts one in the trash. "Main cover on all its tracks" (found by words) puts it on
//!   those that haven't got it.
//! * `group:Name` puts it in a grouping (a new one: `group:Name kind:era`), `-group:Name`
//!   takes it out.
//! * `source:`, `-source:N`, `extra:`, `-extra:`, `note:`, `-note:`: as on the artist page.
//! * `move:5 n:2` moves its fifth track to the second place, at once, onto the tracklist's
//!   draft (as Alt+↑/↓ does: the bar above the tracks saves it, Ctrl+Enter too).
//!
//! The album's own changes go through its form (one revision); the rest are sent one after
//! another, in order: one failing stays staged, saying why, and the rest are sent anyway.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, RichText};
use uuid::Uuid;

use super::album_rows;
use super::page_commands::{self as pc, Change, Chosen, Known, NoteDoes, Pinned, Sending, Taken, ThingDoes, Things, Whose};
use crate::account::Vocab;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::commandline::schema::{self, Kind, Lists, Minus, Need, Next, Node, Schema, Spec, Word};
use crate::details::AliasInfo;
use crate::ui::edit::album_form::AlbumForm;
use crate::ui::edit::commandline::{self as cl, CommandLine, Pressed};
use crate::ui::edit::credits;
use crate::ui::edit::extras::Groups;
use crate::ui::edit::part_of;
use crate::ui::theme::PALETTE as P;
use crate::ui::{Page, Shell};

/// The parts of the page the commandline opens and scrolls to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    Credits,
    Covers,
    Sources,
    Notes,
    Extras,
    Merges,
    History,
}

impl Reveal {
    pub fn title(self) -> &'static str {
        match self {
            Reveal::Credits => "Credits",
            Reveal::Covers => "Covers",
            Reveal::Sources => "Sources",
            Reveal::Notes => "Notes",
            Reveal::Extras => "Extras",
            Reveal::Merges => "Merged into this album",
            Reveal::History => "History",
        }
    }
}

/// The parts revealed, by key.
const REVEALS: &[(&str, Reveal)] = &[("credits", Reveal::Credits), ("covers", Reveal::Covers), ("sources", Reveal::Sources), ("notes", Reveal::Notes), ("extras", Reveal::Extras), ("merges", Reveal::Merges), ("history", Reveal::History)];

const OG: &[Word] = &[Word { word: "yes", help: "only original files" }, Word { word: "no", help: "all files" }];
const QUEUE: &[Word] = &[Word { word: "end", help: "at the end of the queue" }, Word { word: "next", help: "before what's queued" }];
const STAR: &[Word] = &[Word { word: "yes", help: "starred" }, Word { word: "no", help: "not starred" }];
const OWN: &[Word] = &[Word { word: "own", help: "its own name" }];

/// A credited artist's arguments.
const CREDIT_ARGS: &[Spec] = &[
    Spec::new("role", Kind::Text, "its role: primary, featured, producer… (another role: a credit of its own)").with_minus(),
    Spec::new("as", Kind::Or(OWN, &Kind::AliasOf(&[])), "the name it's credited under"),
    Spec::new("n", Kind::Place, "its place among the artists"),
];

/// The keys that change the album (edit mode only).
const EDITS: &[&str] = &["title", "kind", "date", "why", "artist", "feat", "cover", "group", "move", "source", "extra", "note"];

/// Every command, in the order the list offers them.
pub static SCHEMA: Schema = Schema {
    commands: &[
        Spec::new("play", Kind::Maybe(&Kind::Track), "the album, or from a track (its number or title)"),
        Spec::new("queue", Kind::Maybe(&Kind::Words(QUEUE)), "at the end, or next").aliases(&["enqueue"]),
        Spec::new("star", Kind::Maybe(&Kind::Words(STAR)), "yours (alone: the other way; -star: not starred)").aliases(&["starred"]).minus(Minus::OffOrClear),
        Spec::new("rating", Kind::Text, "yours: 0 to 5 stars, 3.5 too (-rating clears it)").aliases(&["rate", "stars"]).minus(Minus::OffOrClear),
        Spec::new("tracks", Kind::Maybe(&Kind::Text), "filters its tracks, as the library's filter (alone: all of them)").aliases(&["track", "filter"]),
        Spec::new("og", Kind::Maybe(&Kind::Words(OG)), "only original files").aliases(&["original"]),
        Spec::new("credits", Kind::Nothing, "opened and scrolled to"),
        Spec::new("covers", Kind::Nothing, "opened and scrolled to"),
        Spec::new("sources", Kind::Nothing, "opened and scrolled to"),
        Spec::new("notes", Kind::Nothing, "opened and scrolled to"),
        Spec::new("extras", Kind::Nothing, "opened and scrolled to"),
        Spec::new("merges", Kind::Nothing, "opened and scrolled to").aliases(&["merged"]),
        Spec::new("history", Kind::Nothing, "opened and scrolled to").aliases(&["revisions"]),
        Spec::new("edit", Kind::Text, "its tracks in the track editor: 3, several (2 5), all, or a title"),
        Spec::new("title", Kind::Text, "its title").aliases(&["name", "rename"]),
        Spec::new("kind", Kind::Text, "album, EP, single…").aliases(&["type"]),
        Spec::new("date", Kind::Text, "released, as the source wrote it: 2019, 2019-05-12… (-date clears it)").aliases(&["released", "year"]).minus(Minus::OffOrClear),
        Spec::new("why", Kind::Text, "kept with the album's changes").aliases(&["reason"]),
        Spec::new("artist", Kind::Artist, "credit one, or change one's role:, as:, n: (-artist: one off; -artist: all)").aliases(&["artists"]).minus(Minus::OffOrClear).next(Next::Args(CREDIT_ARGS)),
        Spec::new("feat", Kind::Artist, "credit one as featured").aliases(&["featured", "featuring"]).minus(Minus::Off).next(Next::Args(CREDIT_ARGS)),
        Spec::new("cover", Kind::Text, "paste, choose, a path, an album or track; a number: that one made the main one").minus(Minus::Off).next(Next::Args(&[Spec::new("n", Kind::Place, "where it goes (1: the main one)")])),
        Spec::new("group", Kind::Grouping, "put it in a grouping, or a new one (kind:)").aliases(&["grouping"]).minus(Minus::Off).next(Next::Args(&[Spec::new("kind", Kind::Text, "a new grouping's kind: collective, era, project…")])),
        Spec::new("move", Kind::Track, "a track (its number or title) to another place (n:)").next(Next::Args(&[Spec::new("n", Kind::Place, "its new place").need(Need::Required)])),
        pc::SOURCE,
        pc::EXTRA,
        pc::NOTE,
    ],
};

/// What the page shows: its tracks filtered by these words (the library's filter), and
/// only original files or not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Filters {
    pub words: String,
    pub og_only: bool,
}

impl Filters {
    /// The track list's filter text.
    pub fn text(&self) -> String {
        let og = if self.og_only { "og:" } else { "" };
        format!("{} {og}", self.words.trim()).trim().to_string()
    }
}

/// What the page knows, for the commandline.
pub struct Here<'a> {
    pub id: Uuid,
    pub catalog: &'a Catalog,
    pub vocab: &'a Vocab,
    /// Its tracks (positions in the catalog), in its order as shown.
    pub tracks: &'a [u32],
    pub og_count: usize,
    /// The parts of the page that are there now, and the folded ones' state (open or not).
    pub sections: &'a [Reveal],
    pub folds: &'a [(Reveal, bool)],
    pub starred: bool,
    /// Yours, 0 to 10 (half stars).
    pub rating: i16,
    pub edit_mode: bool,
    /// Edit mode, and `edit_metadata`: the album can be changed here.
    pub may_edit: bool,
    /// `edit_metadata` (in either mode: a track's quick fix).
    pub may_metadata: bool,
    /// Its details are loaded (the rows of its fields wait for them).
    pub loaded: bool,
    pub title: &'a str,
    pub kind: &'a str,
    pub release_date: &'a str,
    /// Its credits (from its details: empty while they load).
    pub credits: &'a [crate::details::Credit],
    /// The other names of artists typed with `as:` (those looked up so far).
    pub aliases: &'a [(Uuid, Vec<AliasInfo>)],
    pub groupings: &'a [Uuid],
    pub covers: &'a [crate::details::Cover],
    pub sources: &'a [crate::details::Source],
    pub may_source: bool,
    pub extras: &'a [crate::details::Extra],
    pub may_access: bool,
    pub notes: &'a [crate::details::Note],
}

impl Here<'_> {
    /// Its sources, extras and notes, for what the pages share.
    fn things(&self) -> Things<'_> {
        Things {
            whose: Whose::Album(self.id),
            editing: editing(self),
            sources: self.sources,
            may_source: self.may_source,
            shapes: &self.vocab.source_shapes,
            extras: self.extras,
            library_extras: &self.catalog.extras,
            extra_kinds: &self.vocab.extra_kinds,
            may_access: self.may_access,
            notes: self.notes,
        }
    }
}

/// What the page does for a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Play its tracks from this one (a place in `Here::tracks`).
    Play(usize),
    /// Queue it: at the end, or (true) next.
    Queue(bool),
    /// Starred (true) or not.
    Star(bool),
    /// Your rating, 0 to 10.
    Rate(i16),
    CopyLink,
    ShareLink,
    Go(Page),
    /// Opened and scrolled to.
    Reveal(Reveal),
    /// The track at this place to that one (the tracklist's draft).
    Move(usize, usize),
    /// These tracks (places) in the track editor.
    EditTracks(Vec<usize>),
    Merge,
    /// Edit mode on or off.
    ToggleEdit,
    /// The note editor: for this note, or (None) a new one with this title.
    Note(Option<Uuid>, String),
    /// The text files copied in a file manager, listed to be made notes.
    PasteNotes,
}

/// What a command does.
#[derive(Debug, Clone, PartialEq)]
enum Does {
    Og(bool),
    Words(String),
    Act(Act),
    /// The image in the clipboard as a new cover (read when it's staged).
    PasteCover,
    /// Done when Enter is pressed (the file dialog), not before.
    OnEnter,
    /// A change, staged.
    Edit(Edit),
}

/// A change to the album, staged until Ctrl+Enter.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Title(String),
    Kind(String),
    /// As written; empty: none.
    Date(String),
    Why(String),
    Credit(Credit),
    /// An artist's credit taken off: in one role, or (None) all of them.
    Uncredit(Uuid, Option<String>),
    NoCredits,
    /// A source, an extra or a note.
    Thing(Change),
    Cover(CoverChange),
    Group(part_of::Change),
}

/// An artist credited: one in the library (or None: a new one by `name`, made when it's
/// saved), with what's said of it. With a role, it's the artist's credit in that role (an
/// artist can have several); without, its credit in any role (see `album_rows::credit_into`).
#[derive(Debug, Clone, PartialEq)]
pub struct Credit {
    pub artist: Option<Uuid>,
    pub name: String,
    pub role: Option<String>,
    /// Some(None): credited under its own name.
    pub alias: Option<Option<(Uuid, String)>>,
    /// Its place (from 0).
    pub at: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CoverChange {
    /// This cover to this place (0: the main one).
    Move(Uuid, usize),
    Trash(Uuid),
    /// A copy of another track's or album's cover.
    Copy(Uuid),
    File(PathBuf),
    /// An image pasted: its bytes and a file name.
    Image(Arc<Vec<u8>>, String),
    /// Put on each of its tracks that hasn't got it.
    OnTracks(Uuid),
}

impl Edit {
    /// Saved through the album's form (one revision), not on its own.
    fn in_form(&self) -> bool {
        matches!(self, Edit::Title(_) | Edit::Kind(_) | Edit::Date(_) | Edit::Why(_) | Edit::Credit(_) | Edit::Uncredit(..) | Edit::NoCredits)
    }
}

type Staged = pc::Staged<Edit>;
/// The rows as the shared helpers make them, and as the list shows them.
type PcRow = pc::Row<Act, Edit>;
type Choice = pc::Choice<Act, Edit>;
type Row = schema::Row<Choice>;

/// Whether the album can be changed here (else why not).
fn editing(here: &Here) -> Result<(), String> {
    if !here.edit_mode {
        Err("changing the album is in edit mode: e".into())
    } else if !here.may_edit {
        Err("changing the album needs the edit_metadata permission".into())
    } else {
        Ok(())
    }
}

/// What showing a part of the page does: a folded one opens or closes, the rest are
/// scrolled to.
fn reveal_label(here: &Here, r: Reveal) -> String {
    match here.folds.iter().find(|(f, _)| *f == r) {
        Some((_, true)) => format!("{}: close (currently opened)", r.title()),
        Some((_, false)) => format!("{}: open (currently folded)", r.title()),
        None => format!("{}: scrolled to", r.title()),
    }
}

/// `edit:3` (one: listen mode too), `edit:2 5`, `edit:all`, or a title.
fn edit_tracks(c: &Node, here: &Here) -> Result<(String, Does), String> {
    if !here.may_metadata {
        return Err("changing tracks needs the edit_metadata permission".into());
    }
    let value = c.value.trim();
    let numbers: Vec<&str> = value.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    let places: Vec<usize> = if value.eq_ignore_ascii_case("all") {
        (0..here.tracks.len()).collect()
    } else if value.is_empty() {
        return Err("edit: a track's number, several (2 5), all, or a title".into());
    } else if numbers.iter().all(|w| w.chars().all(|c| c.is_ascii_digit())) {
        numbers.iter().map(|n| place(n, here.tracks.len(), "tracks")).collect::<Result<_, _>>()?
    } else {
        vec![track_at(here, value)?]
    };
    let title = |n: usize| here.catalog.track_at(here.tracks[n]).title();
    match places.as_slice() {
        [] => Err("it has no tracks you can see".into()),
        [n] => Ok((format!("the track editor: track {} “{}”", n + 1, title(*n)), Does::Act(Act::EditTracks(places)))),
        _ if !here.edit_mode => Err("several tracks at once is in edit mode: e (one alone works here: edit:3)".into()),
        _ => Ok((format!("the track editor: {} tracks, marked", places.len()), Does::Act(Act::EditTracks(places)))),
    }
}

/// A place typed (from 1) as one from 0 (`what`: what it's the place of, for saying it's
/// wrong).
pub(super) fn place(typed: &str, count: usize, what: &str) -> Result<usize, String> {
    match typed.trim().parse::<usize>() {
        Ok(n) if n >= 1 && n <= count => Ok(n - 1),
        Ok(n) if n >= 1 => Err(format!("{n}: it has {count} {what}")),
        _ => Err(format!("{}: a place is a number from 1", typed.trim())),
    }
}

/// The track a `play:` or `move:` value means: its number, else the one whose title has
/// the words.
fn track_at(here: &Here, typed: &str) -> Result<usize, String> {
    let typed = typed.trim();
    if !typed.is_empty() && typed.chars().all(|c| c.is_ascii_digit()) {
        return place(typed, here.tracks.len(), "tracks");
    }
    let title = |n: usize| here.catalog.track_at(here.tracks[n]).title();
    let found: Vec<usize> = (0..here.tracks.len()).filter(|&n| crate::catalog::query::loosely_in(typed, title(n))).collect();
    match found.as_slice() {
        [n] => Ok(*n),
        [] => Err(format!("no track like “{typed}” on it")),
        _ => Err(format!("{} tracks like “{typed}”: choose one in the list", found.len())),
    }
}

/// The artist a credit's value means: one chosen in the list (or a new one chosen there:
/// None), else one it credits by that name (or the name it's credited under), else the one
/// in the library with exactly that name.
fn artist_named(catalog: &Catalog, credits: &[crate::details::Credit], known: &Known, typed: &str) -> Result<(Option<Uuid>, String), String> {
    let typed = typed.trim();
    if let Some(id) = known.pinned(Pinned::Artist, typed) {
        if id == pc::NEW {
            return Ok((None, typed.to_string()));
        }
        let name = catalog.artist(id).map(|a| a.name.clone()).unwrap_or_else(|| typed.to_string());
        return Ok((Some(id), name));
    }
    let credited = credits.iter().find(|c| c.artist_name.eq_ignore_ascii_case(typed) || c.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(typed)));
    if let Some(c) = credited {
        return Ok((Some(c.artist_id), c.artist_name.clone()));
    }
    let named: Vec<&crate::catalog::model::Artist> = catalog.artists.iter().filter(|a| a.name.eq_ignore_ascii_case(typed) || a.label().eq_ignore_ascii_case(typed)).collect();
    match named.as_slice() {
        [a] => Ok((Some(a.id), a.name.clone())),
        [] => Err(format!("no artist called “{typed}” (a new one: “New artist” in the list)")),
        _ => Err(format!("{} artists are called “{typed}”: choose one in the list", named.len())),
    }
}

/// `artist:Name role:… as:… n:N`, `feat:Name`, `-artist:Name` or `-artist`.
fn credit(c: &Node, here: &Here, known: &Known) -> Result<(String, Edit), String> {
    editing(here)?;
    let featured = c.spec.key == "feat";
    if c.minus && !c.colon {
        return Ok(("artists → none".into(), Edit::NoCredits));
    }
    let value = c.value.trim();
    if value.is_empty() {
        return Err(format!("{}: an artist's name (the list has them)", c.spec.key));
    }
    let (id, name) = artist_named(here.catalog, here.credits, known, value)?;
    let typed_role = match c.arg("role") {
        Some(p) => Some(crate::ui::edit::editor::find_in(&here.vocab.credit_roles, &p.value).ok_or_else(|| format!("no role like “{}”", p.value.trim()))?),
        None => None,
    };
    let theirs: Vec<&crate::details::Credit> = here.credits.iter().filter(|cr| Some(cr.artist_id) == id).collect();
    if c.minus {
        if theirs.is_empty() {
            return Err(format!("it doesn't credit “{value}”"));
        }
        return match typed_role {
            Some(role) if !theirs.iter().any(|cr| cr.role == role) => Err(format!("it doesn't credit {name} as {role}")),
            Some(role) => Ok((format!("artists: − {name} as {role}"), Edit::Uncredit(id.unwrap_or_default(), Some(role)))),
            None => Ok((format!("artists: − {name}"), Edit::Uncredit(id.unwrap_or_default(), None))),
        };
    }
    let role = typed_role.or_else(|| featured.then(|| "featured".to_string()));
    let alias = match c.arg("as") {
        None => None,
        Some(p) if p.value.trim().is_empty() || p.value.trim().eq_ignore_ascii_case(&name) || p.value.trim().eq_ignore_ascii_case("own") => Some(None),
        Some(p) => {
            let Some(id) = id else { return Err(format!("a new artist has no other names yet (as:{})", p.value.trim())) };
            let Some((_, list)) = here.aliases.iter().find(|(a, _)| *a == id) else { return Err(format!("looking up {name}'s names…")) };
            let names: Vec<String> = list.iter().map(|a| a.name.clone()).collect();
            let found = crate::ui::edit::editor::find_in(&names, &p.value).ok_or_else(|| format!("{name} has no name like “{}”", p.value.trim()))?;
            list.iter().find(|a| a.name == found).map(|a| Some((a.id, a.name.clone())))
        }
    };
    let at = match c.arg("n") {
        Some(n) => Some(place(&n.value, here.credits.len() + 1, "places for artists")?),
        None => None,
    };
    // The credit it is: the artist in that role, or (no role) in the one role it has.
    let credited = match &role {
        Some(role) => theirs.iter().any(|cr| cr.role == *role),
        None => !theirs.is_empty(),
    };
    if role.is_none() && theirs.len() > 1 && (alias.is_some() || at.is_some()) {
        let roles: Vec<&str> = theirs.iter().map(|cr| cr.role.as_str()).collect();
        return Err(format!("{name} is credited as {}: say which with role:", roles.join(" and ")));
    }
    let mut label = if credited { format!("artists: {name}") } else { format!("artists: + {name}") };
    if id.is_none() {
        label.push_str(" (new)");
    }
    match (&role, credited) {
        (Some(r), _) => label.push_str(&format!(" as {r}")),
        (None, false) => label.push_str(" (primary if it has none, else featured)"),
        (None, true) => {}
    }
    if let Some(alias) = &alias {
        let shown = alias.as_ref().map_or("its own name", |a| a.1.as_str());
        label.push_str(&format!(", credited as {shown}"));
    }
    if let Some(at) = at {
        label.push_str(&format!(", place {}", at + 1));
    }
    if credited && alias.is_none() && at.is_none() {
        return Err(format!("it credits {name}{} already: say what changes (as:, n:), or another role:", role.as_ref().map(|r| format!(" as {r}")).unwrap_or_default()));
    }
    Ok((label, Edit::Credit(Credit { artist: id, name, role, alias, at })))
}

/// A cover as listed: "1200×1200 · upload".
fn cover_label(c: &crate::details::Cover) -> String {
    match (c.width, c.height) {
        (Some(w), Some(h)) => format!("{w}×{h} · {}", c.source),
        _ => c.source.clone(),
    }
}

/// `cover:…` (an image added, or one moved: `cover:3`, `cover:3 n:2`) or `-cover:N`.
fn cover(c: &Node, here: &Here, known: &Known) -> Result<(String, Does), String> {
    editing(here)?;
    let value = c.value.trim();
    let edit = |label: String, change: CoverChange| Ok((label, Does::Edit(Edit::Cover(change))));
    let which = |h: &str| place(h, here.covers.len(), "covers").map(|n| (n, &here.covers[n]));
    let number = !value.is_empty() && value.chars().all(|c| c.is_ascii_digit());
    let to = c.arg("n");
    if c.minus {
        return match number {
            true => {
                let (n, cv) = which(value)?;
                edit(format!("cover {} ({}) → the trash", n + 1, cover_label(cv)), CoverChange::Trash(cv.id))
            }
            false => Err("-cover:N puts its Nth cover in the trash (the list says which is which)".into()),
        };
    }
    if to.is_some() && !number {
        return Err("n: moves one of its covers: cover:3 n:1".into());
    }
    match value {
        _ if number => {
            let (n, cv) = which(value)?;
            let to = match to {
                Some(t) => place(&t.value, here.covers.len(), "covers")?,
                None => 0,
            };
            if n == to {
                return Err(if to == 0 { format!("{} is the main cover already", n + 1) } else { format!("{} is there already", n + 1) });
            }
            let label = if to == 0 { format!("cover {} → the main one", n + 1) } else { format!("cover {} → place {}", n + 1, to + 1) };
            edit(label, CoverChange::Move(cv.id, to))
        }
        "" => Err("cover: paste, choose, a path, or an album or track (a copy of its cover); cover:N makes one the main one".into()),
        "paste" => Ok(("the image copied, as a new cover (read on Enter)".into(), Does::PasteCover)),
        "choose" => Ok(("an image chosen in the dialog (on Enter: its path goes in the commandline)".into(), Does::OnEnter)),
        v if v.starts_with('/') || v.starts_with('~') => {
            let path = pc::path_of(v);
            if path.is_dir() {
                return Err(format!("{v}: a folder (Tab lists its files)"));
            }
            if !path.is_file() {
                return Err(format!("no file at {v}"));
            }
            if !crate::ui::files::IMAGES.contains(&crate::ui::files::extension(&path).as_str()) {
                return Err(format!("{v}: not an image (JPEG, PNG, WebP or GIF)"));
            }
            edit(format!("+ cover from {}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), CoverChange::File(path))
        }
        v => {
            let (cover_id, what) = cover_of(here, known, v)?;
            edit(format!("+ a copy of {what}'s cover"), CoverChange::Copy(cover_id))
        }
    }
}

/// The cover of an album or track by name: one chosen in the list, else the best match
/// (albums first). Its cover's id, and what it is (in words).
fn cover_of(here: &Here, known: &Known, typed: &str) -> Result<(Uuid, String), String> {
    let album = |id: Uuid| here.catalog.album(id).map(|a| (a.cover_id(), format!("the album “{}”", a.title())));
    let track = |id: Uuid| here.catalog.track(id).map(|t| (t.cover_id(), format!("the track “{}”", t.title())));
    let found = match (known.pinned(Pinned::Album, typed), known.pinned(Pinned::Track, typed)) {
        (Some(id), _) => album(id),
        (_, Some(id)) => track(id),
        _ => {
            let hit = |part| Search::default().run_part(here.catalog, typed, part, 1).first().map(|h| h.0);
            match (hit(SearchPart::Albums), hit(SearchPart::Tracks)) {
                (Some(i), _) => album(here.catalog.album_at(i).id()),
                (_, Some(i)) => track(here.catalog.track_at(i).id()),
                _ => None,
            }
        }
    };
    match found {
        Some((Some(cover), what)) => Ok((cover, what)),
        Some((None, what)) => Err(format!("{what} has no cover")),
        None => Err(format!("no album or track like “{typed}”")),
    }
}

/// `group:Name` (and `kind:` for a new one), or `-group:Name`.
fn group(main: &Node, here: &Here, known: &Known) -> Result<(String, Edit), String> {
    editing(here)?;
    let name = main.value.trim();
    if name.is_empty() {
        return Err(if main.minus { "-group:Name takes it out of that grouping".into() } else { "group: a grouping's name (the list has them), or a new one's".into() });
    }
    let named = |g: &&crate::catalog::model::Grouping| known.pinned(Pinned::Grouping, name) == Some(g.id) || g.name.eq_ignore_ascii_case(name);
    if main.minus {
        let g = here.groupings.iter().filter_map(|id| here.catalog.grouping(*id)).find(named).ok_or_else(|| format!("it isn't in “{name}”"))?;
        return Ok((format!("out of {} “{}”", g.kind, g.name), Edit::Group(part_of::Change::Remove(g.id))));
    }
    let kind = main.arg("kind").map(|p| p.value.trim().to_string());
    let found: Vec<&crate::catalog::model::Grouping> = here.catalog.groupings.iter().filter(named).collect();
    match (found.as_slice(), kind) {
        (_, Some(kind)) if kind.is_empty() => Err(format!("kind: {} (or any word)", crate::ui::views::grouping::KINDS.join(", "))),
        ([], Some(kind)) => Ok((format!("into a new {kind} “{name}”"), Edit::Group(part_of::Change::New { kind, name: name.to_string() }))),
        ([], None) => Err(format!("no grouping “{name}” (a new one: group:{name} kind:collective, era, project…)")),
        ([g], None) if here.groupings.contains(&g.id) => Err(format!("it's in “{}” already", g.name)),
        ([g], None) => Ok((format!("into {} “{}”", g.kind, g.name), Edit::Group(part_of::Change::Add(g.id)))),
        ([_], Some(_)) => Err(format!("“{name}” is there already: group:{name} without kind:")),
        (_, _) => Err(format!("several groupings are called “{name}”: choose one in the list")),
    }
}

/// `rating:3.5` (stars, halves too) as 0 to 10.
pub(super) fn rating(value: &str) -> Result<i16, String> {
    let stars: f32 = value.trim().replace(',', ".").parse().map_err(|_| format!("rating: 0 to 5 stars (3.5 too), not “{}”", value.trim()))?;
    let halves = (stars * 2.0).round();
    if !(0.0..=10.0).contains(&halves) || (stars * 2.0 - halves).abs() > 0.01 {
        return Err("rating: 0 to 5 stars, in halves".into());
    }
    Ok(halves as i16)
}

/// Stars, as said: "3.5 stars".
pub(super) fn stars(rating: i16) -> String {
    let n = rating as f32 / 2.0;
    match rating {
        0 => "no rating".into(),
        2 => "1 star".into(),
        _ => format!("{n} stars"),
    }
}

/// What a command does, or why it can't (problems in how it's typed after the page's own).
fn resolve_one(c: &Node, here: &Here, known: &Known) -> Result<(String, Does), String> {
    if let Some(done) = pc::resolve_thing(c, &here.things(), known) {
        return done.map(|(label, does)| {
            let does = match does {
                ThingDoes::Change(change) => Does::Edit(Edit::Thing(change)),
                ThingDoes::Note(NoteDoes::Open(note, title)) => Does::Act(Act::Note(note, title)),
                ThingDoes::Note(NoteDoes::Paste) => Does::Act(Act::PasteNotes),
                ThingDoes::Note(NoteDoes::Hide(note)) => Does::Edit(Edit::Thing(Change::HideNote(note))),
            };
            (label, does)
        });
    }
    let value = c.value.trim();
    let edit = |label: String, edit: Edit| editing(here).map(|_| (label, Does::Edit(edit)));
    let done = match c.spec.key {
        "og" if here.og_count == 0 => Err("it has no original files".into()),
        "og" => match Kind::Words(OG).word(value) {
            None if value.is_empty() => Ok(("only original files".into(), Does::Og(true))),
            Some("yes") => Ok(("only original files".into(), Does::Og(true))),
            Some(_) => Ok(("all files, not only original ones".into(), Does::Og(false))),
            None => Err("og: yes or no".into()),
        },
        "tracks" if value.is_empty() => Ok(("all its tracks (no filter)".into(), Does::Words(String::new()))),
        "tracks" => Ok((format!("its tracks, filtered: {value}"), Does::Words(value.to_string()))),
        "play" if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
        "play" if value.is_empty() => Ok(("play it".into(), Does::Act(Act::Play(0)))),
        "play" => track_at(here, value).map(|n| (format!("play it from track {} “{}”", n + 1, here.catalog.track_at(here.tracks[n]).title()), Does::Act(Act::Play(n)))),
        "queue" if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
        "queue" => match value.to_lowercase().as_str() {
            "" | "end" | "last" => Ok(("queued, at the end".into(), Does::Act(Act::Queue(false)))),
            "next" | "first" => Ok(("queued, next".into(), Does::Act(Act::Queue(true)))),
            _ => Err("queue: next, or nothing (at the end)".into()),
        },
        "star" if c.minus => Ok(("not starred".into(), Does::Act(Act::Star(false)))),
        "star" => match value.to_lowercase().as_str() {
            "" => Ok((if here.starred { "not starred" } else { "starred" }.into(), Does::Act(Act::Star(!here.starred)))),
            "yes" | "y" | "on" => Ok(("starred".into(), Does::Act(Act::Star(true)))),
            "no" | "n" | "off" => Ok(("not starred".into(), Does::Act(Act::Star(false)))),
            _ => Err("star: yes or no (or nothing: the other way)".into()),
        },
        "rating" if c.minus => Ok(("no rating".into(), Does::Act(Act::Rate(0)))),
        "rating" if value.is_empty() => Err("rating: 0 to 5 stars (3.5 too)".into()),
        "rating" => rating(value).map(|r| (format!("your rating → {}", stars(r)), Does::Act(Act::Rate(r)))),
        "title" if value.is_empty() => Err("title: its new title".into()),
        "title" => edit(format!("title → {value}"), Edit::Title(value.to_string())),
        "kind" if value.is_empty() => Err(format!("kind: {}", here.vocab.album_kinds.join(", "))),
        "kind" => match crate::ui::edit::editor::find_in(&here.vocab.album_kinds, value) {
            Some(kind) => edit(format!("kind → {kind}"), Edit::Kind(kind)),
            None => Err(format!("no kind like “{value}” ({})", here.vocab.album_kinds.join(", "))),
        },
        "date" if c.minus || value.is_empty() => edit("released → none".into(), Edit::Date(String::new())),
        "date" => edit(format!("released → {value}"), Edit::Date(value.to_string())),
        "why" => edit(format!("why: {value} (kept with a change to its title, kind, date or artists)"), Edit::Why(value.to_string())),
        "edit" if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
        "edit" => edit_tracks(c, here),
        "artist" | "feat" => credit(c, here, known).map(|(label, edit)| (label, Does::Edit(edit))),
        "cover" => cover(c, here, known),
        "group" => group(c, here, known).map(|(label, edit)| (label, Does::Edit(edit))),
        "move" => match editing(here) {
            Err(err) => Err(err),
            Ok(()) => match c.arg("n") {
                None => Err("move: a track, then n: its new place (move:5 n:2)".into()),
                Some(n) => match (track_at(here, value), place(&n.value, here.tracks.len(), "tracks")) {
                    (Ok(a), Ok(b)) if a == b => Err(format!("track {} is there already", a + 1)),
                    (Ok(a), Ok(b)) => Ok((format!("track {} “{}” → place {} (the tracklist's draft)", a + 1, here.catalog.track_at(here.tracks[a]).title(), b + 1), Does::Act(Act::Move(a, b)))),
                    (Err(err), _) | (_, Err(err)) => Err(err),
                },
            },
        },
        key => match REVEALS.iter().find(|(k, _)| *k == key).map(|(_, r)| *r) {
            Some(r) if !here.sections.contains(&r) => Err(format!("no {} here", r.title().to_lowercase())),
            Some(r) => Ok((reveal_label(here, r), Does::Act(Act::Reveal(r)))),
            None => Err(format!("{key}: not a command here")),
        },
    };
    let wrong = schema::problems(c);
    match (done, wrong.is_empty()) {
        (Err(e), _) => Err(e),
        (Ok(_), false) => Err(wrong.join(" · ")),
        (ok, true) => ok,
    }
}

/// Each command typed: what it does (or why it can't).
fn resolve(text: &str, here: &Here, known: &Known) -> Vec<Result<(String, Does), String>> {
    let line = schema::parse(&SCHEMA, text);
    let mut out: Vec<Result<(String, Does), String>> = line.commands.iter().map(|c| resolve_one(c, here, known)).collect();
    // Words after a `|` (a section's name, say): only as an action's name, exactly. (Words
    // alone: the list's, Enter takes its row.)
    if !line.commands.is_empty() && !line.words.is_empty() {
        let typed = line.words.join(" ");
        out.push(match actions(&typed, here).into_iter().find(|r| r.label.split("  ").next().is_some_and(|l| l.eq_ignore_ascii_case(&typed))) {
            Some(schema::Row { label, take: schema::Take::Page(Choice::Act(act)), .. }) => Ok((label, Does::Act(act))),
            _ => Err(SCHEMA.not_a_command(&line.words[0])),
        });
    } else if line.commands.is_empty() && line.words.first().is_some_and(|w| w.contains(':')) {
        out.push(Err(SCHEMA.not_a_command(&line.words[0])));
    }
    out
}

/// The actions plain words find (the keys are listed after them).
fn actions(typed: &str, here: &Here) -> Vec<Row> {
    let mut rows = Vec::new();
    let found = |label: &str, also: &[&str]| crate::catalog::query::loosely_in(typed, label) || also.iter().any(|a| a.starts_with(&typed.to_lowercase()));
    let act = |label: &str, sub: &str, act: Act| Row { label: label.into(), sub: sub.into(), take: schema::Take::Page(Choice::Act(act)) };
    if !here.tracks.is_empty() {
        if found("Play", &[]) {
            rows.push(act("Play", "the album", Act::Play(0)));
        }
        if found("Queue it", &["queue", "enqueue"]) {
            rows.push(act("Queue it", "at the end of the queue", Act::Queue(false)));
        }
        if found("Play next", &["queue", "next"]) {
            rows.push(act("Play next", "before what's queued", Act::Queue(true)));
        }
    }
    let star = if here.starred { "Unstar" } else { "Star" };
    if found(star, &["star", "favourite", "favorite", "like"]) {
        rows.push(act(star, "yours", Act::Star(!here.starred)));
    }
    if found("Copy link", &["link", "share"]) {
        rows.push(act("Copy link", "for someone with an account (Ctrl+L)", Act::CopyLink));
    }
    if found("Make a share link", &["share", "link"]) {
        rows.push(act("Make a share link…", "anyone can open it (Ctrl+Shift+L)", Act::ShareLink));
    }
    for &section in here.sections {
        if found(section.title(), &[section.title().split(' ').next().unwrap_or("")]) {
            let label = reveal_label(here, section);
            let sub = label.rsplit(": ").next().unwrap_or("");
            rows.push(act(section.title(), sub, Act::Reveal(section)));
        }
    }
    // Its artists and groupings: their pages.
    if !typed.trim().is_empty() {
        if let Some(album) = here.catalog.album(here.id) {
            let mut seen = Vec::new();
            for c in album.credits().iter() {
                let Some(a) = here.catalog.artist(c.0).filter(|_| !seen.contains(&c.0)) else { continue };
                seen.push(c.0);
                if found(&a.name, &[]) || c.2.is_some_and(|alias| found(alias, &[])) {
                    rows.push(act(&a.name, "its page", Act::Go(Page::Artist(a.id))));
                }
            }
        }
        for g in here.groupings.iter().filter_map(|id| here.catalog.grouping(*id)) {
            if found(&g.name, &[]) {
                rows.push(act(&g.name, &format!("{}: its page", g.kind), Act::Go(Page::Grouping(g.id))));
            }
        }
    }
    let edit_label = if here.edit_mode { "Edit mode off" } else { "Edit mode on" };
    if found(edit_label, &["edit"]) {
        rows.push(act(edit_label, "Alt+E, anywhere", Act::ToggleEdit));
    }
    if editing(here).is_ok()
        && let Some(main) = here.covers.first()
        && found("Main cover on all its tracks", &["cover"])
    {
        let label = "the main cover on each of its tracks that hasn't got it";
        rows.push(Row { label: "Main cover on all its tracks".into(), sub: "staged".into(), take: schema::Take::Page(Choice::Stage("main cover on its tracks".into(), label.into(), Edit::Cover(CoverChange::OnTracks(main.id)))) });
    }
    if here.edit_mode && found("Merge into another album", &[]) {
        rows.push(act("Merge into another album…", "Ctrl+M", Act::Merge));
    }
    rows
}

/// Its tracks as rows: "3 Title", the value `3`.
fn track_rows(here: &Here, typed: &str, sub: &str) -> Vec<PcRow> {
    here.tracks
        .iter()
        .enumerate()
        .filter(|&(_, &t)| typed.is_empty() || crate::catalog::query::loosely_in(typed, here.catalog.track_at(t).title()))
        .map(|(n, &t)| PcRow { label: format!("{} {}", n + 1, here.catalog.track_at(t).title()), sub: sub.into(), choice: Choice::Raw(format!("{}", n + 1), None) })
        .collect()
}

/// Rows for a list of words: each as it is.
fn word_rows(words: &[String], typed: &str, sub: &str) -> Vec<PcRow> {
    words.iter().filter(|w| w.to_lowercase().starts_with(&typed.to_lowercase())).map(|w| PcRow { label: w.clone(), sub: sub.into(), choice: Choice::Raw(w.clone(), None) }).collect()
}

/// What the page knows, for the list.
struct Lister<'a> {
    here: &'a Here<'a>,
    known: &'a Known,
}

impl Lists<Choice> for Lister<'_> {
    fn choices(&self, _kind: Kind, _typed: &str, node: &Node, command: &Node) -> Vec<Row> {
        let (here, known) = (self.here, self.known);
        if let Some(rows) = pc::thing_choices(node, command, &here.things(), known) {
            return rows;
        }
        album_choices(node, command, here, known).into_iter().map(pc::to_schema).collect()
    }

    fn words(&self, typed: &str) -> Vec<Row> {
        actions(typed, self.here)
    }

    fn fields(&self, node: &Node, given: &[&str]) -> Vec<(String, String)> {
        pc::thing_fields(node, given, &self.here.things())
    }

    fn offers(&self, spec: &Spec) -> bool {
        // (Arguments come after their command, which was offered.)
        if !SCHEMA.commands.iter().any(|c| std::ptr::eq(c, spec)) {
            return true;
        }
        match spec.key {
            "edit" => self.here.may_metadata && !self.here.tracks.is_empty(),
            "source" => editing(self.here).is_ok() && self.here.may_source,
            key if EDITS.contains(&key) => editing(self.here).is_ok(),
            _ => true,
        }
    }
}

/// The choices for the value being typed (`c`: its command).
fn album_choices(node: &Node, c: &Node, here: &Here, known: &Known) -> Vec<PcRow> {
    let value = node.value.trim();
    let typed = value.to_lowercase();
    let can = editing(here).is_ok();
    let is_command = std::ptr::eq(node, c);
    let covers = |sub: &str| -> Vec<PcRow> { here.covers.iter().enumerate().map(|(n, cv)| PcRow { label: format!("{} {}{}", n + 1, cover_label(cv), if n == 0 { " · main" } else { "" }), sub: sub.into(), choice: Choice::Raw(format!("{}", n + 1), None) }).collect() };
    match (c.spec.key, node.spec.key) {
        ("play", "play") => track_rows(here, value, "play from here"),
        ("rating", "rating") if typed.is_empty() && here.rating > 0 && !c.minus => {
            let now = format!("{}", here.rating as f32 / 2.0);
            vec![PcRow { label: now.clone(), sub: format!("yours now: {}", stars(here.rating)), choice: Choice::Raw(now, None) }]
        }
        ("edit", "edit") if here.may_metadata => {
            let mut rows = Vec::new();
            if here.edit_mode && "all".starts_with(&typed) {
                rows.push(PcRow { label: "all".into(), sub: "every track, marked".into(), choice: Choice::Raw("all".into(), None) });
            }
            rows.extend(track_rows(here, value, "in the track editor"));
            rows
        }
        ("move", "move") if can => track_rows(here, value, "to move (then n: its new place)"),
        ("group", "kind") => {
            let kinds: Vec<String> = crate::ui::views::grouping::KINDS.iter().map(|k| k.to_string()).collect();
            word_rows(&kinds, value, "a new grouping's kind (any word works)")
        }
        ("kind", "kind") if can => word_rows(&here.vocab.album_kinds, value, if here.kind.is_empty() { "" } else { "a kind" }),
        ("title", "title") if can && typed.is_empty() => vec![PcRow { label: here.title.to_string(), sub: "its title now".into(), choice: Choice::Value(here.title.to_string()) }],
        ("date", "date") if can && typed.is_empty() && !here.release_date.is_empty() && !c.minus => {
            vec![PcRow { label: here.release_date.to_string(), sub: "released, now".into(), choice: Choice::Value(here.release_date.to_string()) }]
        }
        ("artist" | "feat", _) if can && is_command && c.minus => here
            .credits
            .iter()
            .filter(|cr| cr.artist_name.to_lowercase().contains(&typed))
            .map(|cr| PcRow { label: format!("− {}", cr.artist_name), sub: format!("{}: taken off", cr.role), choice: Choice::Pin(Pinned::Artist, cr.artist_name.clone(), cr.artist_id, None) })
            .collect(),
        ("artist" | "feat", _) if can && is_command => {
            let mut rows: Vec<PcRow> = Vec::new();
            if typed.is_empty() {
                rows.extend(here.credits.iter().map(|cr| PcRow { label: cr.artist_name.clone(), sub: format!("{}: credited, change it", cr.role), choice: Choice::Pin(Pinned::Artist, cr.artist_name.clone(), cr.artist_id, None) }));
                return rows;
            }
            for (i, _) in Search::default().run_part(here.catalog, value, SearchPart::Artists, 8) {
                let (name, sub) = crate::catalog::search::describe(here.catalog, SearchPart::Artists, i);
                let a = &here.catalog.artists[i as usize];
                let sub = if here.credits.iter().any(|cr| cr.artist_id == a.id) { format!("{sub} · credited") } else { sub };
                rows.push(PcRow { label: name.clone(), sub, choice: Choice::Pin(Pinned::Artist, name, a.id, None) });
            }
            rows.push(PcRow { label: format!("New artist “{value}”"), sub: "made when it's saved".into(), choice: Choice::Pin(Pinned::Artist, value.to_string(), pc::NEW, None) });
            rows
        }
        ("artist" | "feat", "role") if can => word_rows(&here.vocab.credit_roles, value, "a role"),
        ("artist" | "feat", "as") if can => {
            let Ok((Some(id), _)) = artist_named(here.catalog, here.credits, known, &c.value) else { return Vec::new() };
            let names: Vec<String> = here.aliases.iter().find(|(a, _)| *a == id).map(|(_, l)| l.iter().map(|a| a.name.clone()).collect::<Vec<_>>()).unwrap_or_default();
            word_rows(&names, value, "a name it goes by")
        }
        ("cover", "n") if can => covers("its place"),
        ("cover", "cover") if can && c.minus => covers("to the trash"),
        ("cover", "cover") if can => {
            let mut rows = if typed.is_empty() { covers("made the main one") } else { Vec::new() };
            rows.extend(pc::file_words(&typed, "the image (or image file) copied", "the file dialog"));
            rows.extend(pc::path_rows(value));
            if !typed.is_empty() && !value.starts_with('/') && !value.starts_with('~') && !typed.chars().all(|ch| ch.is_ascii_digit()) {
                for (part, kind) in [(SearchPart::Albums, Pinned::Album), (SearchPart::Tracks, Pinned::Track)] {
                    for (i, _) in Search::default().run_part(here.catalog, value, part, 5) {
                        let (title, sub) = crate::catalog::search::describe(here.catalog, part, i);
                        let id = match kind {
                            Pinned::Album => here.catalog.album_at(i).id(),
                            _ => here.catalog.track_at(i).id(),
                        };
                        rows.push(PcRow { label: title.clone(), sub: format!("{sub} · a copy of its cover"), choice: Choice::Pin(kind, title, id, None) });
                    }
                }
            }
            rows
        }
        ("group", "group") if can && c.minus => here
            .groupings
            .iter()
            .filter_map(|id| here.catalog.grouping(*id))
            .filter(|g| g.name.to_lowercase().contains(&typed))
            .map(|g| PcRow { label: format!("− {}", g.name), sub: format!("{}: taken out of it", g.kind), choice: Choice::Pin(Pinned::Grouping, g.name.clone(), g.id, None) })
            .collect(),
        ("group", "group") if can => {
            let mut rows: Vec<PcRow> = here
                .catalog
                .groupings
                .iter()
                .filter(|g| !here.groupings.contains(&g.id) && g.name.to_lowercase().contains(&typed))
                .take(12)
                .map(|g| PcRow { label: g.name.clone(), sub: format!("{}: put it in", g.kind), choice: Choice::Pin(Pinned::Grouping, g.name.clone(), g.id, None) })
                .collect();
            if !value.is_empty() && !here.catalog.groupings.iter().any(|g| g.name.eq_ignore_ascii_case(value)) {
                rows.push(PcRow { label: format!("New grouping “{value}”"), sub: "made when it's saved (then kind:)".into(), choice: Choice::Raw(value.to_string(), None) });
            }
            rows
        }
        _ => Vec::new(),
    }
}

pub struct AlbumCommands {
    cl: CommandLine,
    notice: Option<String>,
    /// The album they're for (another one: they start over, and these are kept for when
    /// it's back).
    album: Option<Uuid>,
    staged: Vec<Staged>,
    /// The row of the album's fields the keys are on (↓ from an empty line).
    row: Option<usize>,
    /// Other albums' changes staged, not saved yet.
    kept: HashMap<Uuid, Vec<Staged>>,
    /// Changes being saved on their own (sources, extras, notes, covers, groupings).
    sending: Sending<Edit>,
    known: Known,
    groups: Groups,
    /// Files chosen in the dialog: for an extra (`file:choose`), a cover (`cover:choose`).
    chosen: Chosen,
    chosen_cover: Chosen,
    /// The frame it was last shown in (not the one before: the page was just opened).
    last_pass: u64,
}

impl Default for AlbumCommands {
    fn default() -> Self {
        AlbumCommands {
            cl: CommandLine::new(egui::Id::new("album-commandline")),
            notice: None,
            album: None,
            staged: Vec::new(),
            row: None,
            kept: HashMap::new(),
            sending: Sending::default(),
            known: Known::default(),
            groups: Groups::default(),
            chosen: Chosen::new("file"),
            chosen_cover: Chosen::new("cover"),
            last_pass: 0,
        }
    }
}

impl AlbumCommands {
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
        !self.staged.is_empty()
    }

    /// The artists whose other names the line needs (an `as:` after them): their details
    /// are looked up, for `Here::aliases`.
    pub fn artists_named(&self, catalog: &Catalog, credits: &[crate::details::Credit]) -> Vec<Uuid> {
        let line = schema::parse(&SCHEMA, &self.cl.text);
        line.commands
            .iter()
            .filter(|c| matches!(c.spec.key, "artist" | "feat") && c.arg("as").is_some())
            .filter_map(|c| artist_named(catalog, credits, &self.known, &c.value).ok()?.0)
            .collect()
    }

    /// The commandline for the album `id`, its list and what it would do, and the changes
    /// staged. Returns what the page does now.
    pub fn show(&mut self, ui: &mut egui::Ui, shell: &mut Shell, id: Uuid, here: &Here, filters: &mut Filters) -> Option<Act> {
        if self.album != Some(id) {
            let mut kept = std::mem::take(&mut self.kept);
            if let Some(was) = self.album
                && !self.staged.is_empty()
            {
                kept.insert(was, std::mem::take(&mut self.staged));
            }
            let staged = kept.remove(&id).unwrap_or_default();
            *self = AlbumCommands { album: Some(id), staged, kept, ..Default::default() };
        }
        let ctx = ui.ctx().clone();
        self.answers(shell);
        let visibility = schema::parse(&SCHEMA, &self.cl.text).commands.iter().any(|c| c.arg("visibility").is_some());
        pc::ask_groups_when(visibility, &here.things(), shell, &mut self.known, &self.groups);
        self.chosen.put_in_line(&mut self.cl);
        self.chosen_cover.put_in_line(&mut self.cl);
        pc::arrive(&mut self.cl, &ctx, shell, &mut self.last_pass);
        let focused = self.cl.hold_focus(&ctx);
        let line = schema::parse(&SCHEMA, &self.cl.text);
        // (Nothing typed: no list.)
        let rows: Vec<Row> = if self.cl.text.trim().is_empty() { Vec::new() } else { schema::rows(&SCHEMA, &line, &self.cl.text, &Lister { here, known: &self.known }) };
        self.cl.highlighted = self.cl.highlighted.min(rows.len().saturating_sub(1));
        let can_edit = editing(here).is_ok();
        // The album's fields as rows: as they'll be with the staged changes (on what a
        // failed save left in the form, if it did).
        let pending = shell.album_forms.get(&id).filter(|f| f.dirty()).map(|f| f.data.clone());
        let base = pending.unwrap_or_else(|| acetate_core::editor::forms::AlbumData {
            title: here.title.to_string(),
            kind: here.kind.to_string(),
            release_date: here.release_date.to_string(),
            credits: credits::rows_from(here.credits),
        });
        let fields: Vec<album_rows::Row> = if can_edit && here.loaded { album_rows::rows(&base, self.staged.iter().map(|s| &s.edit)) } else { Vec::new() };
        self.row = self.row.and_then(|n| (!fields.is_empty()).then(|| n.min(fields.len() - 1)));
        if self.row.is_some() {
            // On the rows the keys are theirs (Esc too: back to the line, not off the page).
            shell.page_takes_keys = true;
            shell.page_takes_back = true;
        }
        let mut pressed = Pressed::default();
        if focused {
            pressed = self.cl.keys(&ctx, rows.len(), !fields.is_empty(), can_edit && !self.staged.is_empty());
            if pressed.down && rows.is_empty() && !fields.is_empty() {
                // Into the rows.
                self.row = Some(0);
                ctx.memory_mut(|m| m.surrender_focus(self.cl.id()));
            }
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
        // ---- Keys on a row ----
        if let Some(n) = self.row.filter(|_| !ctx.egui_wants_keyboard_input() && !shell.palette_open && !shell.modal_was_open)
            && let Some(row) = fields.get(n)
        {
            use egui::{Key, Modifiers};
            let pressed = |k: Key, m: Modifiers| ctx.input_mut(|i| i.consume_key(m, k));
            // Alt first: a key asked for without Alt matches with it too.
            let earlier = pressed(Key::ArrowUp, Modifiers::ALT) || pressed(Key::K, Modifiers::ALT);
            let later = !earlier && (pressed(Key::ArrowDown, Modifiers::ALT) || pressed(Key::J, Modifiers::ALT));
            let credits_len = fields.iter().filter(|r| matches!(r.kind, album_rows::Field::Credit(_))).count();
            if earlier || later {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                let key = if earlier { RowKey::Earlier } else { RowKey::Later };
                if self.row_key(row, key, credits_len, here) {
                    // The cursor goes with the artist moved (the credits are rows next to each other).
                    if let album_rows::Field::Credit(at) = row.kind {
                        if earlier && at > 0 {
                            self.row = Some(n - 1);
                        } else if later && at + 1 < credits_len {
                            self.row = Some(n + 1);
                        }
                    }
                }
            } else if pressed(Key::ArrowDown, Modifiers::NONE) || pressed(Key::J, Modifiers::NONE) {
                self.row = Some((n + 1).min(fields.len() - 1));
            } else if pressed(Key::ArrowUp, Modifiers::NONE) || pressed(Key::K, Modifiers::NONE) {
                if n == 0 {
                    self.row = None;
                    self.cl.focus = true;
                } else {
                    self.row = Some(n - 1);
                }
            } else if pressed(Key::Escape, Modifiers::NONE) {
                self.row = None;
                self.cl.focus = true;
            } else {
                let key = [(Key::Enter, RowKey::Edit), (Key::R, RowKey::Role), (Key::A, RowKey::Alias), (Key::Delete, RowKey::Remove)]
                    .into_iter()
                    .find_map(|(k, row_key)| pressed(k, Modifiers::NONE).then_some(row_key));
                if let Some(key) = key {
                    // The letter typed is still in this frame's input: not into the line.
                    ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(_))));
                    self.row_key(row, key, credits_len, here);
                }
            }
        }
        // Ctrl+Enter: what's typed first, then the changes saved. (Without changes here,
        // the form below takes it.)
        let mut save = false;
        let elsewhere = !focused && ctx.egui_wants_keyboard_input();
        if can_edit && !elsewhere && (self.has_changes() || (focused && !self.cl.text.trim().is_empty())) && !shell.palette_open && !shell.modal_was_open {
            save = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        }

        let hint = if can_edit {
            "/  title:  artist:  cover:  group:  move:  play  tracks:  (a word finds the rest · | between commands · Ctrl+Enter saves)"
        } else {
            "/  play  queue  star  rating:  tracks:  og:  credits  notes  (a word finds the rest · | between commands)"
        };
        if self.cl.field(ui, true, hint).changed() {
            self.notice = None;
        }
        let driven = self.cl.drive(ui, pressed, &rows, &SCHEMA, &mut self.known.pins, true, "↑/↓ choose · Tab: take it · Enter: take it and do it");
        let mut enter = driven.run || (save && !self.cl.text.trim().is_empty());
        let mut act = None;
        if let Some(choice) = &driven.page {
            match pc::take_page(&mut self.cl, &ctx, &line, choice) {
                Taken::Act(a) => act = Some(a),
                Taken::Stage(staged) => self.staged.push(staged),
                Taken::Filled | Taken::Inserted => {}
            }
            enter = false;
        }
        // `file:paste`, `file:choose`, `cover:choose`: the paths in the line first (what's
        // staged is what the line says).
        if enter && let Err(err) = self.expand_files(shell) {
            self.notice = Some(err);
            enter = false;
            save = false;
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here, &self.known);
            let texts = command_texts(&self.cl.text);
            // `cover:paste`: the image read now (none there: nothing's done).
            let pasted = done.iter().flatten().any(|d| d.1 == Does::PasteCover).then(crate::ui::edit::cover_list::clipboard_image);
            let problem = match &pasted {
                Some(None) => Some("there's no image in the clipboard (copy one, or an image file)".to_string()),
                Some(Some(Err(err))) => Some(err.clone()),
                _ => None,
            };
            match done.iter().find_map(|d| d.as_ref().err()).cloned().or(problem) {
                Some(err) => {
                    self.notice = Some(format!("Not done: {err}"));
                    save = false;
                }
                None => {
                    let mut pasted = pasted.flatten().and_then(Result::ok);
                    for (n, (label, does)) in done.into_iter().flatten().enumerate() {
                        let text = texts.get(n).cloned().unwrap_or_default();
                        match does {
                            Does::Act(a) => act = Some(a),
                            Does::Edit(edit) => self.staged.push(Staged { text, label, edit, failed: None }),
                            Does::PasteCover => {
                                if let Some((bytes, name)) = pasted.take() {
                                    let label = format!("+ cover: the image pasted ({name})");
                                    self.staged.push(Staged { text, label, edit: Edit::Cover(CoverChange::Image(Arc::new(bytes), name)), failed: None });
                                }
                            }
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
        // The changes staged right under the commandline (as on every page), then the rows.
        if let Some(n) = pc::status(ui, self.notice.as_deref(), self.sending.count, &self.staged, can_edit) {
            self.staged.remove(n);
        }
        if !fields.is_empty() {
            ui.add_space(6.0);
            if let Some(n) = album_rows::show(ui, &fields, self.row) {
                self.row = Some(n);
                ctx.memory_mut(|m| m.surrender_focus(self.cl.id()));
            }
            if self.row.is_some() {
                ui.label(RichText::new("Enter edits it · on an artist: r role, a credited as, Alt+↑/↓ (or Alt+k/j) its place · Delete takes it off · Esc back to the commandline").color(P.faint).small());
            }
        }
        if self.has_changes() {
            shell.form_dirty = true;
        }
        // Something to look at (a section, the note editor, a track moved): the keys go
        // there, and the commandline doesn't take the cursor back. Edit mode switched, or
        // something yours (a star, a rating, a link copied): it stays here.
        if act.as_ref().is_some_and(|a| matches!(a, Act::Reveal(_) | Act::Note(..) | Act::PasteNotes | Act::Move(..) | Act::Merge | Act::Go(_) | Act::Play(_) | Act::EditTracks(_))) {
            pc::let_go(&mut self.cl, &ctx);
        }
        act
    }

    /// What a key does on a row: the line filled in to edit it, or the change staged at once
    /// (the same commands the line takes). False: nothing (said in the notice).
    fn row_key(&mut self, row: &album_rows::Row, key: RowKey, credits: usize, here: &Here) -> bool {
        use album_rows::Field as R;
        let q = |name: &str| SCHEMA.quoted(name);
        let credit = |this: &mut Self| -> Option<String> {
            if row.artist.is_none() {
                this.notice = Some(format!("{} isn't made yet: Backspace on an empty commandline takes the change that added it back.", row.name));
                return None;
            }
            Some(q(&row.name))
        };
        let (fill, stage): (Option<String>, Option<String>) = match (row.kind, key) {
            (R::Title, RowKey::Edit) => (Some(format!("title:{}", q(&row.value))), None),
            (R::Kind, RowKey::Edit) => (Some("kind:".into()), None),
            (R::Date, RowKey::Edit) => (Some(format!("date:{}", row.value)), None),
            (R::Date, RowKey::Remove) => (None, Some("-date".into())),
            (R::AddArtist, RowKey::Edit) => (Some("artist:".into()), None),
            // Another role: this one taken off, and the artist credited in the role typed, here.
            (R::Credit(at), RowKey::Edit | RowKey::Role) => (credit(self).map(|n| format!("-artist:{n} role:{} | artist:{n} n:{} role:", q(&row.role), at + 1)), None),
            (R::Credit(_), RowKey::Alias) => (credit(self).map(|n| format!("artist:{n} role:{} as:", q(&row.role))), None),
            (R::Credit(_), RowKey::Remove) => (None, credit(self).map(|n| format!("-artist:{n} role:{}", q(&row.role)))),
            (R::Credit(at), RowKey::Earlier) if at > 0 => (None, credit(self).map(|n| format!("artist:{n} role:{} n:{at}", q(&row.role)))),
            (R::Credit(at), RowKey::Later) if at + 1 < credits => (None, credit(self).map(|n| format!("artist:{n} role:{} n:{}", q(&row.role), at + 2))),
            _ => (None, None),
        };
        if let Some(text) = fill {
            self.cl.text = text;
            self.cl.highlighted = 0;
            self.row = None;
            self.cl.focus = true;
            self.cl.gently = false;
            self.cl.place_cursor = true;
            return true;
        }
        if let Some(text) = stage {
            return self.stage_text(&text, here);
        }
        false
    }

    /// Commands staged as if typed and entered (the changes only; false: one can't be).
    fn stage_text(&mut self, text: &str, here: &Here) -> bool {
        let done = resolve(text, here, &self.known);
        if let Some(err) = done.iter().find_map(|d| d.as_ref().err()) {
            self.notice = Some(format!("Not done: {err}"));
            return false;
        }
        let texts = command_texts(text);
        for (n, (label, does)) in done.into_iter().flatten().enumerate() {
            if let Does::Edit(edit) = does {
                self.staged.push(Staged { text: texts.get(n).cloned().unwrap_or_default(), label, edit, failed: None });
            }
        }
        self.notice = None;
        true
    }

    /// The keys are on a row of the album's fields (the page's other keys leave them be).
    pub fn on_rows(&self) -> bool {
        self.row.is_some()
    }

    /// `file:paste` becomes the copied files' paths; `file:choose` and `cover:choose` open the
    /// dialog and come out of the line (the paths are put in when chosen: Enter again).
    fn expand_files(&mut self, shell: &Shell) -> Result<(), String> {
        let dialog = pc::Dialog { title: "Files for the extra", kinds: ("Any file", &[]), many: true };
        let spans = pc::file_spans(&schema::parse(&SCHEMA, &self.cl.text), "extra", "file");
        pc::expand_file_spans(&mut self.cl.text, shell, spans, true, dialog, &self.chosen)?;
        let dialog = pc::Dialog { title: "A cover", kinds: ("Images", crate::ui::files::IMAGES), many: false };
        let line = schema::parse(&SCHEMA, &self.cl.text);
        let spans = line.commands.iter().filter(|c| c.spec.key == "cover" && !c.minus).map(|c| (c.start, c.end, c.value.trim().to_string())).collect();
        pc::expand_file_spans(&mut self.cl.text, shell, spans, false, dialog, &self.chosen_cover)
    }

    /// What the server said about the changes sent (one failing: staged again, saying why).
    fn answers(&mut self, shell: &mut Shell) {
        if let Some(said) = self.sending.answers(&mut self.staged) {
            if let Some(id) = self.album {
                shell.album_details.refresh(id);
            }
            shell.act(crate::ui::Action::RefreshLibrary);
            if !said.is_empty() {
                self.notice = Some(said.join(" "));
            }
        }
    }

    /// The changes saved. The album's own (title, kind, date, artists) onto its form (its
    /// draft, kept in the shell) and saved from there: one revision, the form says how it
    /// went. The rest one after another, in order (`answers` takes what the server said).
    fn save(&mut self, shell: &mut Shell, id: Uuid) {
        if self.staged.is_empty() {
            return;
        }
        let (form_edits, others): (Vec<Staged>, Vec<Staged>) = std::mem::take(&mut self.staged).into_iter().partition(|s| s.edit.in_form());
        let mut notices = Vec::new();
        if !form_edits.is_empty() {
            let d = shell.album_details.get(id).value.clone();
            match d.as_ref().and_then(|d| shell.album_forms.remove(&id).or_else(|| AlbumForm::from_detail(d))) {
                None => {
                    notices.push(if d.is_none() { "The album is still loading: try again in a moment." } else { "This album has no revision to build on, so it can't be changed here." });
                    self.staged.extend(form_edits);
                }
                Some(mut form) => {
                    for s in form_edits {
                        match s.edit {
                            Edit::Why(why) => form.set_reason(&why),
                            edit => album_rows::apply(&mut form.data, &edit),
                        }
                    }
                    if form.dirty() {
                        form.save(shell);
                        notices.push("Saving…");
                    } else {
                        notices.push("The album's title, kind, date and artists are as they were: nothing to save there (why: is kept with a change to those).");
                    }
                    shell.album_forms.insert(id, form);
                }
            }
        }
        if !others.is_empty() {
            self.sending.spawn_in_order(shell, others, move |api, ctx, edit| async move { send(&api, &ctx, id, edit).await });
        }
        self.notice = (!notices.is_empty()).then(|| notices.join(" "));
    }
}

/// Each command's text, as typed (on its chip).
fn command_texts(text: &str) -> Vec<String> {
    schema::parse(&SCHEMA, text).commands.iter().map(|c| schema::command_text(text, c).to_string()).collect()
}

/// What a key on a row of the album's fields asks.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RowKey {
    Edit,
    Role,
    Alias,
    Remove,
    Earlier,
    Later,
}

/// A change sent on its own. Ok: what's worth saying, if anything.
async fn send(api: &crate::api::Api, ctx: &egui::Context, album: Uuid, edit: Edit) -> Result<Option<String>, String> {
    match edit {
        Edit::Thing(change) => pc::send(api, ctx, Whose::Album(album), &change).await,
        Edit::Group(change) => part_of::send(api, part_of::Member::Album(album), &change).await.map(|_| None),
        Edit::Cover(change) => send_cover(api, album, change).await.map(|_| None),
        _ => Ok(None),
    }
}

async fn send_cover(api: &crate::api::Api, album: Uuid, change: CoverChange) -> Result<(), String> {
    use reqwest::Method;
    use serde_json::json;
    let path = crate::ui::edit::cover_list::Owner::Album(album).path();
    let failed = |e: crate::api::ApiError| e.message;
    match change {
        CoverChange::Move(id, to) => {
            // From its covers as they are now (an earlier change may have added one).
            let d: crate::details::AlbumDetail = api.send_json(Method::GET, &format!("/api/albums/{album}"), None).await.map_err(failed)?;
            let mut ids: Vec<Uuid> = d.covers.iter().map(|c| c.id).collect();
            let from = ids.iter().position(|c| *c == id).ok_or("that cover isn't the album's anymore")?;
            ids.remove(from);
            ids.insert(to.min(ids.len()), id);
            api.send(Method::PUT, &format!("{path}/order"), Some(json!({ "cover_ids": ids }))).await.map_err(failed)
        }
        CoverChange::Trash(id) => api.send(Method::DELETE, &format!("/api/covers/{id}"), None).await.map_err(failed),
        CoverChange::Copy(id) => api.send(Method::POST, &format!("{path}/copy"), Some(json!({ "cover_id": id }))).await.map_err(failed),
        CoverChange::File(file) => {
            let (bytes, name) = crate::ui::edit::cover_list::read_image(&file)?;
            api.upload::<serde_json::Value>(&path, bytes, &name).await.map(|_| ()).map_err(failed)
        }
        CoverChange::Image(bytes, name) => api.upload::<serde_json::Value>(&path, (*bytes).clone(), &name).await.map(|_| ()).map_err(failed),
        CoverChange::OnTracks(id) => api.send(Method::POST, &format!("/api/albums/{album}/covers/apply-to-tracks"), Some(json!({ "cover_id": id }))).await.map_err(failed),
    }
}

fn apply(filters: &mut Filters, does: Does) {
    match does {
        Does::Og(on) => filters.og_only = on,
        Does::Words(words) => filters.words = words,
        Does::Act(_) | Does::Edit(_) | Does::PasteCover | Does::OnEnter => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANN: &str = "00000000-0000-7000-8000-00000000a001";
    const BO: &str = "00000000-0000-7000-8000-00000000a002";
    const CY1: &str = "00000000-0000-7000-8000-00000000a003";
    const CY2: &str = "00000000-0000-7000-8000-00000000a004";
    const LP: &str = "00000000-0000-7000-8000-00000000b001";
    const EP: &str = "00000000-0000-7000-8000-00000000b002";
    const CREW: &str = "00000000-0000-7000-8000-00000000c001";

    fn id(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    fn catalog() -> Catalog {
        let track = |n: u32, title: &str| {
            serde_json::json!([format!("00000000-0000-7000-8000-0000000000{n:02}"), title, "Ann", [[ANN, "primary", null]], [], 1000,
                               format!("00000000-0000-7000-8000-0000000000f{n}"), null, null, null, null, "released", [], "", false, "flac"])
        };
        let body = serde_json::json!({
            "format": crate::catalog::model::FORMAT,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": [track(1, "Intro"), track(2, "Glass Harbor"), track(3, "Glass Harbor (Reprise)")]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [[LP, "Long Player", "album", "Ann", [[ANN, "primary", null]], null, null, null, null, "", 3, 3000, []],
                                [EP, "Other EP", "ep", "Bo", [[BO, "primary", null]], null, null, "00000000-0000-7000-8000-00000000d001", null, "", 0, 0, []]]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[ANN, "Ann", "", ["Annie"], 3, 1], [BO, "Bo", "", [], 0, 1], [CY1, "Cy", "", [], 0, 0], [CY2, "Cy", "UK", [], 0, 0]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"],
                          "rows": [[CREW, "collective", "The Crew", 0, 0, []]]},
            "playlists": []
        });
        Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap())
    }

    fn vocab() -> Vocab {
        let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        Vocab { credit_roles: strings(&["primary", "featured", "producer"]), album_kinds: strings(&["album", "ep", "single"]), ..Default::default() }
    }

    struct World {
        catalog: Catalog,
        vocab: Vocab,
        credits: Vec<crate::details::Credit>,
        covers: Vec<crate::details::Cover>,
        aliases: Vec<(Uuid, Vec<AliasInfo>)>,
    }

    fn world() -> World {
        let credits = serde_json::from_value(serde_json::json!([{ "artist_id": ANN, "artist_name": "Ann", "role": "primary", "alias_id": null, "alias": null }])).unwrap();
        let covers = serde_json::from_value(serde_json::json!([
            { "id": Uuid::from_u128(1), "blob_sha256": "", "width": 1200, "height": 1200, "source": "upload" },
            { "id": Uuid::from_u128(2), "blob_sha256": "", "source": "soundcloud" },
        ]))
        .unwrap();
        let aliases = vec![(id(ANN), vec![AliasInfo { id: Uuid::from_u128(9), name: "Annie".into() }])];
        World { catalog: catalog(), vocab: vocab(), credits, covers, aliases }
    }

    const TRACKS: [u32; 3] = [0, 1, 2];
    const SECTIONS: [Reveal; 2] = [Reveal::Credits, Reveal::Notes];

    fn here(w: &World, edit: bool) -> Here<'_> {
        Here {
            loaded: true,
            id: id(LP),
            catalog: &w.catalog,
            vocab: &w.vocab,
            tracks: &TRACKS,
            og_count: 1,
            sections: &SECTIONS,
            folds: &[(Reveal::Credits, true)],
            starred: false,
            rating: 0,
            edit_mode: edit,
            may_edit: edit,
            may_metadata: true,
            title: "Long Player",
            kind: "album",
            release_date: "2019",
            credits: &w.credits,
            aliases: &w.aliases,
            groupings: &[],
            covers: &w.covers,
            sources: &[],
            may_source: false,
            extras: &[],
            may_access: false,
            notes: &[],
        }
    }

    fn rows_for(text: &str, here: &Here, known: &Known) -> Vec<Row> {
        schema::rows(&SCHEMA, &schema::parse(&SCHEMA, text), text, &Lister { here, known })
    }

    #[test]
    fn every_state_leads_somewhere_valid() {
        let w = world();
        let edit = here(&w, true);
        let known = Known::default();
        let edit = Here { may_source: true, ..edit };
        schema::walk(&SCHEMA, &Lister { here: &edit, known: &known }, &|_| vec!["x".into()], 3).unwrap();
        let no_sources = Here { may_source: false, ..edit };
        let labels: Vec<String> = rows_for("extra:x ", &no_sources, &known).into_iter().map(|r| r.label).collect();
        assert!(labels.contains(&"source:".to_string()), "an extra's own source: isn't a source to add");
    }

    fn does(text: &str, here: &Here, known: &Known) -> Vec<Result<Does, String>> {
        resolve(text, here, known).into_iter().map(|r| r.map(|(_, d)| d)).collect()
    }

    #[test]
    fn listening_and_filters() {
        let w = world();
        let here = here(&w, false);
        let known = Known::default();
        let does = |text: &str| does(text, &here, &known);
        assert_eq!(does("play:"), [Ok(Does::Act(Act::Play(0)))]);
        assert!(does("play").is_empty(), "a word: the list's (Enter takes its row)");
        assert_eq!(does("play:2"), [Ok(Does::Act(Act::Play(1)))]);
        assert_eq!(does("play:reprise"), [Ok(Does::Act(Act::Play(2)))]);
        assert!(does("play:glass")[0].as_ref().unwrap_err().contains("choose one"), "two titles have it");
        assert!(does("play:4")[0].is_err());
        assert_eq!(does("queue:next | star"), [Ok(Does::Act(Act::Queue(true))), Ok(Does::Act(Act::Star(true)))]);
        assert_eq!(does("rating:3.5 | -star"), [Ok(Does::Act(Act::Rate(7))), Ok(Does::Act(Act::Star(false)))]);
        assert!(does("rating:3.3")[0].is_err());
        assert_eq!(does("-rating"), [Ok(Does::Act(Act::Rate(0)))]);
        assert_eq!(does("credits: | notes"), [Ok(Does::Act(Act::Reveal(Reveal::Credits))), Ok(Does::Act(Act::Reveal(Reveal::Notes)))]);
        assert!(does("covers:")[0].as_ref().unwrap_err().contains("no covers"));
        // A folded part that's open: closed.
        assert_eq!(resolve("credits:", &here, &known)[0].as_ref().unwrap().0, "Credits: close (currently opened)");
        assert_eq!(resolve("notes:", &here, &known)[0].as_ref().unwrap().0, "Notes: scrolled to");
        // The track editor: one in listen mode, several in edit mode.
        assert_eq!(does("edit:2"), [Ok(Does::Act(Act::EditTracks(vec![1])))]);
        assert!(does("edit:all")[0].as_ref().unwrap_err().contains("edit mode"));
        // Filters: live, as the commandline would leave them.
        let mut commands = AlbumCommands::default();
        commands.cl.text = "tracks:year:2019 glass | og:".into();
        let f = commands.preview(&here, &Filters::default());
        assert_eq!((f.words.as_str(), f.og_only), ("year:2019 glass", true), "a filter's own keys stay in the value");
        assert_eq!(f.text(), "year:2019 glass og:");
        commands.cl.text = "tracks:".into();
        assert_eq!(commands.preview(&here, &f).words, "");
        // Plain words find actions, its artists, and the keys.
        let labels = |typed: &str| actions(typed, &here).into_iter().map(|r| r.label).collect::<Vec<_>>();
        assert!(labels("ann").contains(&"Ann".to_string()));
        assert!(labels("next").contains(&"Play next".to_string()));
        assert!(labels("merge").is_empty(), "merging is edit mode's");
        assert!(labels("title").is_empty(), "changing it too");
    }

    #[test]
    fn the_albums_own_changes_are_edit_modes() {
        let w = world();
        let (listen, edit) = (here(&w, false), here(&w, true));
        let known = Known::default();
        assert!(does("title:New", &listen, &known)[0].as_ref().unwrap_err().contains("edit mode"));
        assert_eq!(does("name:A | b", &edit, &known), [Ok(Does::Edit(Edit::Title("A".into()))), Err("“b”: not a command".into())]);
        assert_eq!(does("title:\"A | b\"", &edit, &known), [Ok(Does::Edit(Edit::Title("A | b".into())))]);
        assert_eq!(does("kind:e | date:2020-05 | -date", &edit, &known), [Ok(Does::Edit(Edit::Kind("ep".into()))), Ok(Does::Edit(Edit::Date("2020-05".into()))), Ok(Does::Edit(Edit::Date(String::new())))]);
        assert!(does("kind:mixtape", &edit, &known)[0].is_err());
        assert!(does("-title:x", &edit, &known)[0].is_err());
        // `release:` isn't `released:` (exact names only).
        assert_eq!(does("release:2020", &edit, &known), [Err("“release:2020”: not a command".into())]);
    }

    #[test]
    fn credits() {
        let w = world();
        let edit = here(&w, true);
        let mut known = Known::default();
        let credit = |text: &str, known: &Known| match does(text, &edit, known).remove(0) {
            Ok(Does::Edit(Edit::Credit(c))) => c,
            other => panic!("{text}: {other:?}"),
        };
        let c = credit("artist:Bo role:prod n:1", &known);
        assert_eq!((c.artist, c.role.as_deref(), c.at), (Some(id(BO)), Some("producer"), Some(0)));
        assert_eq!(credit("feat:bo", &known).role.as_deref(), Some("featured"));
        // Credited already in another role: a credit of its own (primary and producer).
        let c = credit("artist:Ann role:producer", &known);
        assert_eq!((c.artist, c.role.as_deref()), (Some(id(ANN)), Some("producer")));
        assert!(resolve("artist:Ann role:producer", &edit, &known)[0].as_ref().unwrap().0.contains("+ Ann"));
        assert!(does("artist:Ann role:primary", &edit, &known)[0].as_ref().unwrap_err().contains("already"));
        // Credited already: what changes; under one of its names.
        assert_eq!(credit("artist:Ann as:annie", &known).alias, Some(Some((Uuid::from_u128(9), "Annie".into()))));
        assert!(does("artist:Ann", &edit, &known)[0].as_ref().unwrap_err().contains("say what changes"));
        // Two of that name: chosen in the list. None: a new one, chosen there too.
        assert!(does("artist:Cy", &edit, &known)[0].as_ref().unwrap_err().contains("choose one"));
        assert!(does("artist:Dee", &edit, &known)[0].as_ref().unwrap_err().contains("New artist"));
        known.pin(Pinned::Artist, "cy", id(CY2));
        known.pin(Pinned::Artist, "dee", pc::NEW);
        assert_eq!(credit("artist:Cy", &known).artist, Some(id(CY2)));
        let c = credit("artist:Dee", &known);
        assert_eq!((c.artist, c.name.as_str()), (None, "Dee"));
        assert!(does("artist:Dee as:D", &edit, &known)[0].as_ref().unwrap_err().contains("new artist"));
        // Off: one it credits, or all.
        assert_eq!(does("-artist:Ann | -artist", &edit, &known), [Ok(Does::Edit(Edit::Uncredit(id(ANN), None))), Ok(Does::Edit(Edit::NoCredits))]);
        assert_eq!(does("-artist:Ann role:primary", &edit, &known), [Ok(Does::Edit(Edit::Uncredit(id(ANN), Some("primary".into()))))]);
        assert!(does("-artist:Ann role:producer", &edit, &known)[0].as_ref().unwrap_err().contains("as producer"));
        assert!(does("-artist:Bo", &edit, &known)[0].is_err());
        // Onto the form's rows.
        let mut rows = credits::rows_from(&w.credits);
        album_rows::credit_into(&mut rows, credit("artist:Bo n:1", &known));
        album_rows::credit_into(&mut rows, credit("artist:Ann role:producer", &known));
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.role.as_str())).collect::<Vec<_>>(), [("Bo", "featured"), ("Ann", "primary"), ("Ann", "producer")], "another role: a credit of its own");
        // The list: matches, then a new one; once one is there, what can follow it.
        let rows = rows_for("artist:b", &edit, &known);
        assert!(matches!(rows.first().map(|r| &r.take), Some(schema::Take::Pin { name, id: a, .. }) if name == "Bo" && *a == id(BO)));
        assert!(rows.last().is_some_and(|r| r.label == "New artist “b”"));
        let labels: Vec<String> = rows_for("artist:Bo ", &edit, &known).into_iter().map(|r| r.label).collect();
        assert_eq!(labels, ["role:", "as:", "n:"]);
    }

    #[test]
    fn covers_groupings_and_moves() {
        let w = world();
        let edit = here(&w, true);
        let mut known = Known::default();
        let does = |text: &str, known: &Known| does(text, &edit, known);
        let cover = |c| Ok(Does::Edit(Edit::Cover(c)));
        assert_eq!(does("cover:2 | cover:2 n:1 | -cover:1", &known), [cover(CoverChange::Move(Uuid::from_u128(2), 0)), cover(CoverChange::Move(Uuid::from_u128(2), 0)), cover(CoverChange::Trash(Uuid::from_u128(1)))]);
        assert!(does("cover:1", &known)[0].as_ref().unwrap_err().contains("main cover already"));
        assert!(does("cover:3", &known)[0].is_err());
        assert_eq!(does("cover:paste", &known), [Ok(Does::PasteCover)]);
        assert!(does("cover:/no/such.png", &known)[0].as_ref().unwrap_err().contains("no file"));
        // Another's cover: the best match, or the one chosen.
        assert_eq!(does("cover:other ep", &known), [cover(CoverChange::Copy(id("00000000-0000-7000-8000-00000000d001")))]);
        assert!(does("cover:long player", &known)[0].as_ref().unwrap_err().contains("no cover"));
        // Groupings: one there is, a new one (with its kind), out of one it's in.
        assert_eq!(does("group:the crew", &known), [Ok(Does::Edit(Edit::Group(part_of::Change::Add(id(CREW)))))]);
        assert_eq!(does("group:Night kind:era", &known), [Ok(Does::Edit(Edit::Group(part_of::Change::New { kind: "era".into(), name: "Night".into() })))]);
        assert!(does("group:Night", &known)[0].as_ref().unwrap_err().contains("kind:"));
        assert!(does("-group:The Crew", &known)[0].as_ref().unwrap_err().contains("isn't in"));
        let groupings = [id(CREW)];
        let inside = Here { groupings: &groupings, ..here(&w, true) };
        assert_eq!(resolve("-group:the crew", &inside, &known).remove(0).map(|r| r.1), Ok(Does::Edit(Edit::Group(part_of::Change::Remove(id(CREW))))));
        // `kind:` after group: is the grouping's; alone, the album's.
        let line = schema::parse(&SCHEMA, "group:x kind:era | kind:ep");
        assert_eq!(line.commands.iter().map(|c| (c.spec.key, c.args.len())).collect::<Vec<_>>(), [("group", 1), ("kind", 0)]);
        // Moving a track: at once (the tracklist's draft).
        assert_eq!(does("move:3 n:1", &known), [Ok(Does::Act(Act::Move(2, 0)))]);
        assert_eq!(does("edit:all | edit:1 3", &known), [Ok(Does::Act(Act::EditTracks(vec![0, 1, 2]))), Ok(Does::Act(Act::EditTracks(vec![0, 2])))]);
        assert!(does("move:3", &known)[0].is_err());
        assert!(does("move:3 n:1", &Known::default()).len() == 1);
        assert!(resolve("move:3 n:1", &here(&w, false), &known)[0].as_ref().unwrap_err().contains("edit mode"));
        // The main cover on its tracks: found by words, staged as it is.
        known.pins.clear();
        assert!(actions("cover", &edit).iter().any(|r| matches!(&r.take, schema::Take::Page(Choice::Stage(_, _, Edit::Cover(CoverChange::OnTracks(c)))) if *c == Uuid::from_u128(1))));
    }

    /// The server's album (and artist) revisions have no kind or version: they read as
    /// revisions all the same.
    #[test]
    fn an_albums_history_reads() {
        let d: crate::details::AlbumDetail = serde_json::from_value(serde_json::json!({
            "id": LP, "title": "Long Player", "kind": "album", "release_date": null, "revision_id": Uuid::from_u128(2),
            "revisions": [{ "id": Uuid::from_u128(2), "created_at": "2026-10-05T12:34:56.789Z", "author": "someone", "source": "edit",
                            "reason": "the label's spelling", "data": { "title": "Long Player", "kind": "album" } }],
        }))
        .unwrap();
        let r = &d.revisions.unwrap()[0];
        assert_eq!((r.kind.as_str(), r.variant_id, r.reason.as_str()), ("", None, "the label's spelling"));
    }

    #[test]
    fn keys_on_the_fields_rows_fill_the_line_or_stage_the_change() {
        let w = world();
        let here = here(&w, true);
        let base = acetate_core::editor::forms::AlbumData {
            title: here.title.into(),
            kind: here.kind.into(),
            release_date: here.release_date.into(),
            credits: credits::rows_from(here.credits),
        };
        let rows = album_rows::rows(&base, [].iter());
        let mut commands = AlbumCommands::default();
        let row = |kind| rows.iter().find(|r| r.kind == kind).unwrap();

        // Enter on the title: the line, ready to change it.
        assert!(commands.row_key(row(album_rows::Field::Title), RowKey::Edit, 1, &here));
        assert_eq!(commands.cl.text, "title:Long Player");
        // `a` on the artist: the name it's credited under, to type.
        commands.row_key(row(album_rows::Field::Credit(0)), RowKey::Alias, 1, &here);
        assert_eq!(commands.cl.text, "artist:Ann role:primary as:", "that credit (the artist can have several)");
        // Delete on it, and on the date: staged at once.
        assert!(commands.row_key(row(album_rows::Field::Credit(0)), RowKey::Remove, 1, &here));
        assert!(commands.row_key(row(album_rows::Field::Date), RowKey::Remove, 1, &here));
        let staged: Vec<&Edit> = commands.staged.iter().map(|s| &s.edit).collect();
        assert_eq!(staged, [&Edit::Uncredit(id(ANN), Some("primary".into())), &Edit::Date(String::new())]);
        // `r` on it: another role, in its place.
        commands.row_key(row(album_rows::Field::Credit(0)), RowKey::Role, 1, &here);
        assert_eq!(commands.cl.text, "-artist:Ann role:primary | artist:Ann n:1 role:");
        // One artist can't move earlier or later.
        let before = commands.staged.len();
        assert!(!commands.row_key(row(album_rows::Field::Credit(0)), RowKey::Earlier, 1, &here));
        assert!(!commands.row_key(row(album_rows::Field::Credit(0)), RowKey::Later, 1, &here));
        assert_eq!(commands.staged.len(), before);
    }
}
