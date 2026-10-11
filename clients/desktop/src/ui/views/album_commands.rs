//! The album page's commandline (a `ui::edit::commandline`; the language is
//! `crate::commandline`'s, and what it shares with the artist page's is in `page_commands`).
//! It's there in listen mode too, for listening and moving around.
//!
//! * `play` plays the album, `play:#3` (or a title) from its third track; `queue` puts it at
//!   the end of the queue, `queue:next` before what's queued. `star` (`star:no`, `-star`)
//!   and `rating:3.5` (`-rating` clears it) are yours, saved at once.
//! * `tracks:words` filters its tracks (as the library's filter does: `tracks:year:2019`),
//!   `tracks:` shows them all; `og:` / `og:yes` / `og:no` sets the Original files chip. Live
//!   while they're typed, kept with Enter; Esc puts back what they were.
//! * `credits`, `covers`, `sources`, `notes`, `extras`, `history` (and `merges` in edit
//!   mode) open that part of the page and scroll to it; a folded one that's open is closed.
//! * `edit:#3` opens the track editor for its third track (a quick fix, listen mode too, as
//!   Ctrl+E); `edit:all` or `edit:#2 #5` for several, marked (edit mode).
//! * Plain words find these too, its artists and groupings (to go to), and copying its link
//!   or a share link; merging it in edit mode.
//!
//! Edit mode (and `edit_metadata`) adds changes, staged as chips and saved together with
//! Ctrl+Enter. Outside edit mode they're not suggested, and typed they say why not.
//!
//! * `title:`, `kind:`, `date:` (`-date` clears it), `why:` (kept with those and the
//!   artists: alone it has nothing to go with, and saving says so), and its artists:
//!   `artist:Name role:… as:… #2` credits one (or, credited already, changes its role, the
//!   name it's credited under, its place), `feat:Name` as featured, `-artist:Name` takes one
//!   off, `-artist` all of them. Saved through the album's form: one revision, the form
//!   says how it went.
//! * `cover:paste` (an image or an image file copied), `cover:choose` (the file dialog: its
//!   path goes in the line), `cover:/path`, `cover:<album or track>` (a copy of its cover)
//!   add one; `cover:#3` makes the third the main one, `cover:#3 #2` moves it; `-cover:#2`
//!   puts one in the trash. "Main cover on all its tracks" (found by words) puts it on
//!   those that haven't got it.
//! * `group:Name` puts it in a grouping (a new one: `group:Name kind:era`), `-group:Name`
//!   takes it out.
//! * `source:`, `-source:#N`, `extra:`, `-extra:`, `note:`, `-note:`: as on the artist page.
//! * `move:#5 #2` moves its fifth track to the second place, at once, onto the tracklist's
//!   draft (as Alt+↑/↓ does: the bar above the tracks saves it, Ctrl+Enter too).
//!
//! The album's own changes go through its form (one revision); the rest are sent one after
//! another, in order: one failing stays staged, saying why, and the rest are sent anyway.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use uuid::Uuid;

use super::page_commands::{self as pc, Arg, Change, Chosen, Known, NoteDoes, Pinned, Sending, Taken, Things, Whose};
use crate::account::Vocab;
use crate::catalog::search::{Part as SearchPart, Search};
use crate::catalog::Catalog;
use crate::commandline::{self, Keys};
use crate::details::AliasInfo;
use crate::ui::edit::album_form::AlbumForm;
use crate::ui::edit::commandline::{self as cl, CommandLine};
use crate::ui::edit::credits::{self, CreditRow};
use crate::ui::edit::extras::Groups;
use crate::ui::edit::part_of;
use crate::ui::{Page, Shell};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Og,
    Tracks,
    Play,
    Queue,
    Star,
    Rating,
    Reveal(Reveal),
    Title,
    Kind,
    Date,
    Why,
    Artist,
    Featured,
    Role,
    As,
    Cover,
    Source,
    /// One of a source's fields (after `source:<platform>`): its name is the part's.
    Field,
    Extra,
    File,
    Description,
    Visibility,
    Note,
    Group,
    Move,
    /// Its tracks in the track editor.
    Edit,
}

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

/// Every key's names: its own first. Exact names only (no starts of them).
const KEYS: &[(K, &[&str])] = &[
    (K::Og, &["og", "original"]),
    (K::Tracks, &["tracks", "track", "filter"]),
    (K::Play, &["play"]),
    (K::Queue, &["queue", "enqueue"]),
    (K::Star, &["star", "starred"]),
    (K::Rating, &["rating", "rate", "stars"]),
    (K::Reveal(Reveal::Credits), &["credits"]),
    (K::Reveal(Reveal::Covers), &["covers"]),
    (K::Reveal(Reveal::Sources), &["sources"]),
    (K::Reveal(Reveal::Notes), &["notes"]),
    (K::Reveal(Reveal::Extras), &["extras"]),
    (K::Reveal(Reveal::Merges), &["merges", "merged"]),
    (K::Reveal(Reveal::History), &["history", "revisions"]),
    (K::Title, &["title", "name", "rename"]),
    (K::Kind, &["kind", "type"]),
    (K::Date, &["date", "released", "year"]),
    (K::Why, &["why", "reason"]),
    (K::Artist, &["artist", "artists"]),
    (K::Featured, &["feat", "featured", "featuring"]),
    (K::Role, &["role"]),
    (K::As, &["as"]),
    (K::Cover, &["cover"]),
    (K::Source, &["source"]),
    (K::Extra, &["extra"]),
    (K::File, &["file", "files"]),
    (K::Description, &["description", "desc"]),
    (K::Visibility, &["visibility", "vis", "seen"]),
    (K::Note, &["note"]),
    (K::Group, &["group", "grouping"]),
    (K::Move, &["move"]),
    (K::Edit, &["edit"]),
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
        matches!(self, K::Date | K::Artist | K::Star | K::Rating)
    }

    fn takes_hash(self) -> bool {
        matches!(self, K::Cover | K::Source | K::Play | K::Move | K::Artist | K::Featured | K::Role | K::As | K::Edit)
    }

    fn takes(main: K, arg: K) -> bool {
        match main {
            K::Extra => matches!(arg, K::Kind | K::File | K::Description | K::Source | K::Visibility),
            K::Artist | K::Featured => matches!(arg, K::Role | K::As),
            K::Group => arg == K::Kind,
            _ => false,
        }
    }

    fn field(main: K) -> Option<K> {
        (main == K::Source).then_some(K::Field)
    }
}

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
    /// An artist's credits taken off.
    Uncredit(Uuid),
    NoCredits,
    /// A source, an extra or a note.
    Thing(Change),
    Cover(CoverChange),
    Group(part_of::Change),
}

/// An artist credited: one in the library (or None: a new one by `name`, made when it's
/// saved), with what's said of it.
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
        matches!(self, Edit::Title(_) | Edit::Kind(_) | Edit::Date(_) | Edit::Why(_) | Edit::Credit(_) | Edit::Uncredit(_) | Edit::NoCredits)
    }
}

type Staged = pc::Staged<Edit>;
type Row = pc::Row<Act, Edit>;
type Choice = pc::Choice<Act, Edit>;

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

/// A source, an extra or a note, as the album's command.
fn thing(done: Result<(String, Change), String>) -> Result<(String, Does), String> {
    done.map(|(label, change)| (label, Does::Edit(Edit::Thing(change))))
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

/// `edit:#3` (one: listen mode too), `edit:#2 #5`, `edit:all`, or a title.
fn edit_tracks(part: &commandline::Part<K>, here: &Here) -> Result<(String, Does), String> {
    if !here.may_metadata {
        return Err("changing tracks needs the edit_metadata permission".into());
    }
    let value = part.value.trim();
    let places: Vec<usize> = if value.eq_ignore_ascii_case("all") && part.hashes.is_empty() {
        (0..here.tracks.len()).collect()
    } else if !part.hashes.is_empty() && value.is_empty() {
        part.hashes.iter().map(|h| place(h, here.tracks.len(), "tracks")).collect::<Result<_, _>>()?
    } else if value.is_empty() {
        return Err("edit: #N (a track), several (#2 #5), all, or a title".into());
    } else {
        vec![track_at(here, part)?]
    };
    let title = |n: usize| here.catalog.track_at(here.tracks[n]).title();
    match places.as_slice() {
        [] => Err("it has no tracks you can see".into()),
        [n] => Ok((format!("the track editor: #{} “{}”", n + 1, title(*n)), Does::Act(Act::EditTracks(places)))),
        _ if !here.edit_mode => Err("several tracks at once is in edit mode: e (one alone works here: edit:#3)".into()),
        _ => Ok((format!("the track editor: {} tracks, marked", places.len()), Does::Act(Act::EditTracks(places)))),
    }
}

/// `#N` as a place from 0 (`what`: what it's the place of, for saying it's wrong).
pub(super) fn place(hash: &str, count: usize, what: &str) -> Result<usize, String> {
    match hash.parse::<usize>() {
        Ok(n) if n >= 1 && n <= count => Ok(n - 1),
        Ok(n) if n >= 1 => Err(format!("#{n}: it has {count} {what}")),
        _ => Err(format!("#{hash}: a place is a number from 1")),
    }
}

/// The track a `play:` or `move:` value means: `#N`, else the one whose title has the words.
fn track_at(here: &Here, part: &commandline::Part<K>) -> Result<usize, String> {
    if let Some(h) = part.hashes.first() {
        return place(h, here.tracks.len(), "tracks");
    }
    let typed = part.value.trim();
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

/// `artist:Name role:… as:… #N`, `feat:Name`, `-artist:Name` or `-artist`.
fn credit(command: &[commandline::Part<K>], here: &Here, known: &Known) -> Result<(String, Edit), String> {
    editing(here)?;
    let main = &command[0];
    let modifier = |k: K| command[1..].iter().find(|p| p.key == k);
    if main.minus && !main.colon {
        if main.key == K::Featured {
            return Err("-feat alone isn't anything: -artist takes every artist off".into());
        }
        return Ok(("artists → none".into(), Edit::NoCredits));
    }
    let value = main.value.trim();
    if value.is_empty() {
        return Err(format!("{}: an artist's name (the list has them)", main.key.name()));
    }
    let (id, name) = artist_named(here.catalog, here.credits, known, value)?;
    if main.minus {
        let Some(id) = id.filter(|id| here.credits.iter().any(|c| c.artist_id == *id)) else { return Err(format!("it doesn't credit “{value}”")) };
        return Ok((format!("artists: − {name}"), Edit::Uncredit(id)));
    }
    let role = match (modifier(K::Role), main.key) {
        (Some(p), _) => Some(crate::ui::edit::editor::find_in(&here.vocab.credit_roles, &p.value).ok_or_else(|| format!("no role like “{}”", p.value.trim()))?),
        (None, K::Featured) => Some("featured".to_string()),
        _ => None,
    };
    let alias = match modifier(K::As) {
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
    let hash = command.iter().rev().find_map(|p| p.hash.clone());
    let at = match hash {
        Some(h) => Some(place(&h, here.credits.len() + 1, "places for artists")?),
        None => None,
    };
    let credited = id.is_some_and(|id| here.credits.iter().any(|c| c.artist_id == id));
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
        label.push_str(&format!(", #{}", at + 1));
    }
    if credited && role.is_none() && alias.is_none() && at.is_none() {
        return Err(format!("it credits {name} already: say what changes (role:, as:, #place)"));
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

/// `cover:…` (an image added, or one moved) or `-cover:#N`.
fn cover(main: &commandline::Part<K>, here: &Here, known: &Known) -> Result<(String, Does), String> {
    editing(here)?;
    let value = main.value.trim();
    let edit = |label: String, change: CoverChange| Ok((label, Does::Edit(Edit::Cover(change))));
    let which = |h: &str| place(h, here.covers.len(), "covers").map(|n| (n, &here.covers[n]));
    if main.minus {
        return match (main.hashes.as_slice(), value.is_empty()) {
            ([h], true) => {
                let (n, c) = which(h)?;
                edit(format!("cover #{} ({}) → the trash", n + 1, cover_label(c)), CoverChange::Trash(c.id))
            }
            _ => Err("-cover:#N puts its Nth cover in the trash (the list says which is which)".into()),
        };
    }
    match (main.hashes.as_slice(), value) {
        ([h], "") => {
            let (n, c) = which(h)?;
            if n == 0 {
                return Err(format!("#{h} is the main cover already"));
            }
            edit(format!("cover #{} → the main one", n + 1), CoverChange::Move(c.id, 0))
        }
        ([h, to], "") => {
            let (n, c) = which(h)?;
            let to = place(to, here.covers.len(), "covers")?;
            if n == to {
                return Err(format!("#{h} is there already"));
            }
            edit(format!("cover #{} → #{}", n + 1, to + 1), CoverChange::Move(c.id, to))
        }
        ([], "") => Err("cover: paste, choose, a path, or an album or track (a copy of its cover); cover:#N makes one the main one".into()),
        ([], "paste") => Ok(("the image copied, as a new cover (read on Enter)".into(), Does::PasteCover)),
        ([], "choose") => Ok(("an image chosen in the dialog (on Enter: its path goes in the commandline)".into(), Does::OnEnter)),
        ([], v) if v.starts_with('/') || v.starts_with('~') => {
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
        ([], v) => {
            let (cover_id, what) = cover_of(here, known, v)?;
            edit(format!("+ a copy of {what}'s cover"), CoverChange::Copy(cover_id))
        }
        _ => Err("cover:#N (made the main one), cover:#N #M (moved), or something to add".into()),
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
fn group(command: &[commandline::Part<K>], here: &Here, known: &Known) -> Result<(String, Edit), String> {
    editing(here)?;
    let main = &command[0];
    let name = main.value.trim();
    if name.is_empty() {
        return Err(if main.minus { "-group:Name takes it out of that grouping".into() } else { "group: a grouping's name (the list has them), or a new one's".into() });
    }
    let named = |g: &&crate::catalog::model::Grouping| known.pinned(Pinned::Grouping, name) == Some(g.id) || g.name.eq_ignore_ascii_case(name);
    if main.minus {
        let g = here.groupings.iter().filter_map(|id| here.catalog.grouping(*id)).find(named).ok_or_else(|| format!("it isn't in “{name}”"))?;
        return Ok((format!("out of {} “{}”", g.kind, g.name), Edit::Group(part_of::Change::Remove(g.id))));
    }
    let kind = command[1..].iter().find(|p| p.key == K::Kind).map(|p| p.value.trim().to_string());
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

/// Each command typed: its text, and what it does (or why it can't).
fn resolve(text: &str, here: &Here, known: &Known) -> Vec<Result<(String, Does), String>> {
    let line = commandline::parse::<K>(text);
    let mut out = Vec::new();
    for command in line.commands() {
        let main = &command[0];
        let value = main.value.trim();
        let minus_ok = matches!(main.key, K::Date | K::Artist | K::Featured | K::Cover | K::Source | K::Extra | K::Note | K::Group | K::Star | K::Rating);
        if main.minus && !minus_ok {
            out.push(Err(format!("-{}: nothing to take off here", main.key.name())));
            continue;
        }
        let edit = |label: String, edit: Edit| editing(here).map(|_| (label, Does::Edit(edit)));
        let nothing_after = |what: &str, does: Does| if value.is_empty() { Ok((what.to_string(), does)) } else { Err(format!("{}: nothing after it", main.key.name())) };
        out.push(match main.key {
            K::Og if here.og_count == 0 => Err("it has no original files".into()),
            K::Og => match value.to_lowercase().as_str() {
                "" | "yes" | "y" | "on" => Ok(("only original files".into(), Does::Og(true))),
                "no" | "n" | "off" => Ok(("all files, not only original ones".into(), Does::Og(false))),
                _ => Err("og: yes or no".into()),
            },
            K::Tracks if value.is_empty() => Ok(("all its tracks (no filter)".into(), Does::Words(String::new()))),
            K::Tracks => Ok((format!("its tracks, filtered: {value}"), Does::Words(value.to_string()))),
            K::Play if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
            K::Play if value.is_empty() && main.hashes.is_empty() => Ok(("play it".into(), Does::Act(Act::Play(0)))),
            K::Play => track_at(here, main).map(|n| (format!("play it from #{} “{}”", n + 1, here.catalog.track_at(here.tracks[n]).title()), Does::Act(Act::Play(n)))),
            K::Queue if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
            K::Queue => match value.to_lowercase().as_str() {
                "" | "end" | "last" => Ok(("queued, at the end".into(), Does::Act(Act::Queue(false)))),
                "next" | "first" => Ok(("queued, next".into(), Does::Act(Act::Queue(true)))),
                _ => Err("queue: next, or nothing (at the end)".into()),
            },
            K::Star if main.minus => Ok(("not starred".into(), Does::Act(Act::Star(false)))),
            K::Star => match value.to_lowercase().as_str() {
                "" => Ok((if here.starred { "not starred" } else { "starred" }.into(), Does::Act(Act::Star(!here.starred)))),
                "yes" | "y" | "on" => Ok(("starred".into(), Does::Act(Act::Star(true)))),
                "no" | "n" | "off" => Ok(("not starred".into(), Does::Act(Act::Star(false)))),
                _ => Err("star: yes or no (or nothing: the other way)".into()),
            },
            K::Rating if main.minus => Ok(("no rating".into(), Does::Act(Act::Rate(0)))),
            K::Rating if value.is_empty() => Err("rating: 0 to 5 stars (3.5 too)".into()),
            K::Rating => rating(value).map(|r| (format!("your rating → {}", stars(r)), Does::Act(Act::Rate(r)))),
            K::Reveal(r) if !here.sections.contains(&r) => Err(format!("no {} here", r.title().to_lowercase())),
            K::Reveal(r) => nothing_after(&reveal_label(here, r), Does::Act(Act::Reveal(r))),
            K::Title if value.is_empty() => Err("title: its new title".into()),
            K::Title => edit(format!("title → {value}"), Edit::Title(value.to_string())),
            K::Kind if value.is_empty() => Err(format!("kind: {}", here.vocab.album_kinds.join(", "))),
            K::Kind => match crate::ui::edit::editor::find_in(&here.vocab.album_kinds, value) {
                Some(kind) => edit(format!("kind → {kind}"), Edit::Kind(kind)),
                None => Err(format!("no kind like “{value}” ({})", here.vocab.album_kinds.join(", "))),
            },
            K::Date if main.minus || value.is_empty() => edit("released → none".into(), Edit::Date(String::new())),
            K::Date => edit(format!("released → {value}"), Edit::Date(value.to_string())),
            K::Why => edit(format!("why: {value} (kept with a change to its title, kind, date or artists)"), Edit::Why(value.to_string())),
            K::Edit if here.tracks.is_empty() => Err("it has no tracks you can see".into()),
            K::Edit => edit_tracks(main, here),
            K::Artist | K::Featured => credit(command, here, known).map(|(label, edit)| (label, Does::Edit(edit))),
            K::Role | K::As => Err(format!("{}: goes after artist:Name", main.key.name())),
            K::Cover => cover(main, here, known),
            K::Source => {
                let fields: Vec<(&str, &str)> = command[1..].iter().map(|p| (p.name(), p.value.trim())).collect();
                thing(pc::source(main.minus, value, &main.hashes, &fields, &here.things()))
            }
            K::Field => Err(format!("{}: goes after source: and its platform (source:bandcamp url:…)", main.name())),
            K::Extra => {
                let args: Vec<(Arg, &str)> = command[1..].iter().filter_map(|a| Some((arg_of(a.key)?, a.value.as_str()))).collect();
                thing(pc::extra(value, main.minus, &args, &here.things(), known))
            }
            K::File | K::Description | K::Visibility => Err(format!("{}: goes after extra:Name", main.key.name())),
            K::Note => pc::note(value, main.minus, &here.things(), known).map(|(label, does)| {
                let does = match does {
                    NoteDoes::Open(note, title) => Does::Act(Act::Note(note, title)),
                    NoteDoes::Paste => Does::Act(Act::PasteNotes),
                    NoteDoes::Hide(note) => Does::Edit(Edit::Thing(Change::HideNote(note))),
                };
                (label, does)
            }),
            K::Group => group(command, here, known).map(|(label, edit)| (label, Does::Edit(edit))),
            K::Move => match editing(here) {
                Err(err) => Err(err),
                Ok(()) => match main.hashes.as_slice() {
                    [from, to] => match (place(from, here.tracks.len(), "tracks"), place(to, here.tracks.len(), "tracks")) {
                        (Ok(a), Ok(b)) if a == b => Err(format!("#{from} is there already")),
                        (Ok(a), Ok(b)) => Ok((format!("track #{} “{}” → #{} (the tracklist's draft)", a + 1, here.catalog.track_at(here.tracks[a]).title(), b + 1), Does::Act(Act::Move(a, b)))),
                        (Err(err), _) | (_, Err(err)) => Err(err),
                    },
                    _ => Err("move:#5 #2 moves the fifth track to the second place".into()),
                },
            },
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

/// The actions plain words find (and the keys to type, for the rest).
fn actions(typed: &str, here: &Here) -> Vec<Row> {
    let mut rows = Vec::new();
    let found = |label: &str, also: &[&str]| crate::catalog::query::loosely_in(typed, label) || also.iter().any(|a| a.starts_with(&typed.to_lowercase()));
    let act = |label: &str, sub: &str, act: Act| Row { label: label.into(), sub: sub.into(), choice: Choice::Act(act) };
    let insert = |label: &str, sub: &str, key: &str| Row { label: label.into(), sub: sub.into(), choice: Choice::Insert(key.into()) };
    if !here.tracks.is_empty() {
        if found("Play", &[]) {
            rows.push(act("Play", "the album", Act::Play(0)));
        }
        if found("Play from a track", &["play"]) {
            rows.push(insert("Play from a track (play:)", "#N or its title", "play:"));
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
    if found("Rate", &["rating", "stars"]) {
        let now = if here.rating > 0 { format!("now {}", stars(here.rating)) } else { "0 to 5 stars".into() };
        rows.push(insert("Rate (rating:)", &now, "rating:"));
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
    if here.may_metadata && !here.tracks.is_empty() && found("Edit its tracks", &["edit"]) {
        rows.push(insert("Edit its tracks (edit:)", "#3: one (Ctrl+E) · all, or #2 #5: marked, in edit mode", "edit:"));
    }
    if !here.tracks.is_empty() && found("Tracks", &["filter"]) {
        rows.push(insert("Filter its tracks (tracks:)", "words, or year:… status:… (as the library's filter)", "tracks:"));
    }
    if here.og_count > 0 && found("Original files", &["og"]) {
        rows.push(insert("og:", "only original files", "og:"));
    }
    // Its artists and groupings: their pages.
    if !typed.trim().is_empty() {
        if let Some(album) = here.catalog.album(here.id) {
            let mut seen = Vec::new();
            for c in album.credits() {
                let Some(a) = here.catalog.artist(c.0).filter(|_| !seen.contains(&c.0)) else { continue };
                seen.push(c.0);
                if found(&a.name, &[]) || c.2.as_deref().is_some_and(|alias| found(alias, &[])) {
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
    if editing(here).is_ok() {
        for (key, label, sub) in [
            ("title:", "Title (title:)", "its title"),
            ("kind:", "Kind (kind:)", "album, EP, single…"),
            ("date:", "Released (date:)", "as the source wrote it: 2019, 2019-05-12…"),
            ("artist:", "Artist (artist:)", "credit one, or change one's role:, as:, #place"),
            ("-artist:", "Take an artist off (-artist:)", "its credit on the album"),
            ("why:", "Why (why:)", "kept with the album's changes"),
            ("cover:", "Cover (cover:)", "paste, choose, a path, an album or track; #N: the main one"),
            ("-cover:", "Cover to the trash (-cover:)", "#N"),
            ("group:", "Grouping (group:)", "put it in one, or a new one (kind:)"),
            ("-group:", "Out of a grouping (-group:)", "one it's in"),
            ("move:", "Move a track (move:)", "#5 #2: the fifth to the second place"),
            ("note:", "Note (note:)", "a new one, or one of its notes to edit"),
            ("extra:", "Extra (extra:)", "a new one, one of its own to change, or one in the library"),
            ("source:", "Source (source:)", "a link, a platform and its fields, or custom"),
        ] {
            if found(label, &[key.trim_end_matches(':').trim_start_matches('-')]) {
                rows.push(insert(label, sub, key));
            }
        }
        if let Some(main) = here.covers.first()
            && found("Main cover on all its tracks", &["cover"])
        {
            let label = "the main cover on each of its tracks that hasn't got it";
            rows.push(Row { label: "Main cover on all its tracks".into(), sub: "staged".into(), choice: Choice::Stage("main cover on its tracks".into(), label.into(), Edit::Cover(CoverChange::OnTracks(main.id))) });
        }
    }
    if here.edit_mode && found("Merge into another album", &[]) {
        rows.push(act("Merge into another album…", "Ctrl+M", Act::Merge));
    }
    rows
}

/// Its tracks as rows: "#3 Title", the value `#3` (on Tab, `next` after it).
fn track_rows(here: &Here, typed: &str, sub: &str, next: Option<&str>) -> Vec<Row> {
    here.tracks
        .iter()
        .enumerate()
        .filter(|&(_, &t)| typed.is_empty() || crate::catalog::query::loosely_in(typed, here.catalog.track_at(t).title()))
        .map(|(n, &t)| Row { label: format!("#{} {}", n + 1, here.catalog.track_at(t).title()), sub: sub.into(), choice: Choice::Raw(format!("#{}", n + 1), next.map(str::to_string)) })
        .collect()
}

/// Rows for a list of words: each as it is (on Tab, `next` after it).
fn word_rows(words: &[String], typed: &str, sub: &str, next: Option<&str>) -> Vec<Row> {
    words
        .iter()
        .filter(|w| w.to_lowercase().starts_with(&typed.to_lowercase()))
        .map(|w| Row { label: w.clone(), sub: sub.into(), choice: Choice::Raw(w.clone(), next.map(str::to_string)) })
        .collect()
}

/// The choices for the value being typed (`group`: what it goes with, if it's an argument
/// or a field).
fn choices(part: &commandline::Part<K>, group: &[commandline::Part<K>], here: &Here, known: &Known) -> Vec<Row> {
    let value = part.value.trim();
    let typed = value.to_lowercase();
    let things = here.things();
    let owner = group.first().map(|g| g.key);
    let can = editing(here).is_ok();
    if let Some(arg) = arg_of(part.key).filter(|_| owner == Some(K::Extra)) {
        return pc::extra_arg_rows(arg, value, &things, known);
    }
    let covers = |sub: &str| -> Vec<Row> { here.covers.iter().enumerate().map(|(n, c)| Row { label: format!("#{} {}{}", n + 1, cover_label(c), if n == 0 { " · main" } else { "" }), sub: sub.into(), choice: Choice::Raw(format!("#{}", n + 1), None) }).collect() };
    match part.key {
        K::Source => pc::source_rows(part.minus, value, part.hash.is_some(), &things),
        K::Note => pc::note_rows(part.minus, value, &things),
        K::Extra => pc::extra_rows(part.minus, value, &things),
        K::Field => {
            let Some(owner) = group.first().filter(|o| o.key == K::Source) else { return Vec::new() };
            let given: Vec<&str> = group.iter().skip(1).chain([part]).map(|p| p.name()).collect();
            pc::field_rows(&owner.value, part.name(), value, &given, &things)
        }
        K::Og if typed.is_empty() => [("yes", "only original files"), ("no", "all files")].into_iter().map(|(v, sub)| Row { label: v.into(), sub: sub.into(), choice: Choice::Value(v.into()) }).collect(),
        K::Play if part.hashes.is_empty() => track_rows(here, value, "play from here", None),
        K::Edit if here.may_metadata && part.hashes.is_empty() => {
            let mut rows = Vec::new();
            if here.edit_mode && "all".starts_with(&typed) {
                rows.push(Row { label: "all".into(), sub: "every track, marked".into(), choice: Choice::Raw("all".into(), None) });
            }
            rows.extend(track_rows(here, value, "in the track editor", None));
            rows
        }
        K::Move if can && part.hashes.is_empty() => track_rows(here, value, "to move (Tab: then its new place, #N)", Some("#")),
        K::Queue => [("end", "at the end of the queue"), ("next", "before what's queued")].into_iter().filter(|(w, _)| w.starts_with(&typed)).map(|(w, sub)| Row { label: w.into(), sub: sub.into(), choice: Choice::Raw(w.into(), None) }).collect(),
        K::Star if typed.is_empty() && !part.minus => [("yes", "starred"), ("no", "not starred")].into_iter().map(|(v, sub)| Row { label: v.into(), sub: sub.into(), choice: Choice::Raw(v.into(), None) }).collect(),
        K::Kind if owner == Some(K::Group) => {
            let kinds: Vec<String> = crate::ui::views::grouping::KINDS.iter().map(|k| k.to_string()).collect();
            word_rows(&kinds, value, "a new grouping's kind (any word works)", None)
        }
        K::Kind if can => word_rows(&here.vocab.album_kinds, value, if here.kind.is_empty() { "" } else { "a kind" }, None),
        K::Title if can && typed.is_empty() => vec![Row { label: here.title.to_string(), sub: "its title now".into(), choice: Choice::Value(here.title.to_string()) }],
        K::Date if can && typed.is_empty() && !here.release_date.is_empty() && !part.minus => {
            vec![Row { label: here.release_date.to_string(), sub: "released, now".into(), choice: Choice::Value(here.release_date.to_string()) }]
        }
        K::Artist | K::Featured if can && part.minus => here
            .credits
            .iter()
            .filter(|c| c.artist_name.to_lowercase().contains(&typed))
            .map(|c| Row { label: format!("− {}", c.artist_name), sub: format!("{}: taken off", c.role), choice: Choice::Pin(Pinned::Artist, c.artist_name.clone(), c.artist_id, None) })
            .collect(),
        K::Artist | K::Featured if can => {
            let mut rows: Vec<Row> = Vec::new();
            if typed.is_empty() {
                rows.extend(here.credits.iter().map(|c| Row { label: c.artist_name.clone(), sub: format!("{}: credited, change it", c.role), choice: Choice::Pin(Pinned::Artist, c.artist_name.clone(), c.artist_id, Some("role:".into())) }));
                return rows;
            }
            for (i, _) in Search::default().run_part(here.catalog, value, SearchPart::Artists, 8) {
                let (name, sub) = crate::ui::edit::inline_search::describe(here.catalog, SearchPart::Artists, i);
                let a = &here.catalog.artists[i as usize];
                let sub = if here.credits.iter().any(|c| c.artist_id == a.id) { format!("{sub} · credited") } else { sub };
                rows.push(Row { label: name.clone(), sub, choice: Choice::Pin(Pinned::Artist, name, a.id, Some("role:".into())) });
            }
            rows.push(Row { label: format!("New artist “{value}”"), sub: "made when it's saved · Tab: its role".into(), choice: Choice::Pin(Pinned::Artist, value.to_string(), pc::NEW, Some("role:".into())) });
            rows
        }
        K::Role if can => word_rows(&here.vocab.credit_roles, value, "a role", Some("as:")),
        K::As if can => {
            let Some(owner) = group.first() else { return Vec::new() };
            let Ok((Some(id), name)) = artist_named(here.catalog, here.credits, known, &owner.value) else { return Vec::new() };
            let mut names = vec![name.clone()];
            names.extend(here.aliases.iter().find(|(a, _)| *a == id).map(|(_, l)| l.iter().map(|a| a.name.clone()).collect::<Vec<_>>()).unwrap_or_default());
            word_rows(&names, value, "a name it goes by", None)
        }
        K::Cover if can && part.minus => covers("to the trash"),
        K::Cover if can && !part.hashes.is_empty() => covers("its place"),
        K::Cover if can => {
            let mut rows = if typed.is_empty() { covers("made the main one") } else { Vec::new() };
            rows.extend(pc::file_words(&typed, "the image (or image file) copied", "the file dialog"));
            rows.extend(pc::path_rows(value));
            if !typed.is_empty() && !value.starts_with('/') && !value.starts_with('~') {
                for (part, kind) in [(SearchPart::Albums, Pinned::Album), (SearchPart::Tracks, Pinned::Track)] {
                    for (i, _) in Search::default().run_part(here.catalog, value, part, 5) {
                        let (title, sub) = crate::ui::edit::inline_search::describe(here.catalog, part, i);
                        let id = match kind {
                            Pinned::Album => here.catalog.album_at(i).id(),
                            _ => here.catalog.track_at(i).id(),
                        };
                        rows.push(Row { label: title.clone(), sub: format!("{sub} · a copy of its cover"), choice: Choice::Pin(kind, title, id, None) });
                    }
                }
            }
            rows
        }
        K::Group if can && part.minus => here
            .groupings
            .iter()
            .filter_map(|id| here.catalog.grouping(*id))
            .filter(|g| g.name.to_lowercase().contains(&typed))
            .map(|g| Row { label: format!("− {}", g.name), sub: format!("{}: taken out of it", g.kind), choice: Choice::Pin(Pinned::Grouping, g.name.clone(), g.id, None) })
            .collect(),
        K::Group if can => {
            let mut rows: Vec<Row> = here
                .catalog
                .groupings
                .iter()
                .filter(|g| !here.groupings.contains(&g.id) && g.name.to_lowercase().contains(&typed))
                .take(12)
                .map(|g| Row { label: g.name.clone(), sub: format!("{}: put it in", g.kind), choice: Choice::Pin(Pinned::Grouping, g.name.clone(), g.id, None) })
                .collect();
            if !value.is_empty() && !here.catalog.groupings.iter().any(|g| g.name.eq_ignore_ascii_case(value)) {
                rows.push(Row { label: format!("New grouping “{value}”"), sub: "made when it's saved · Tab: its kind".into(), choice: Choice::Raw(value.to_string(), Some("kind:".into())) });
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
        let line = commandline::parse::<K>(&self.cl.text);
        line.commands()
            .filter(|c| matches!(c[0].key, K::Artist | K::Featured) && c[1..].iter().any(|p| p.key == K::As))
            .filter_map(|c| artist_named(catalog, credits, &self.known, &c[0].value).ok()?.0)
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
        pc::ask_groups_if::<K>(&self.cl.text, |k| k == K::Visibility, &here.things(), shell, &mut self.known, &self.groups);
        self.chosen.put_in_line(&mut self.cl);
        self.chosen_cover.put_in_line(&mut self.cl);
        pc::arrive(&mut self.cl, &ctx, shell, &mut self.last_pass);
        let focused = self.cl.hold_focus(&ctx);
        let line = commandline::parse::<K>(&self.cl.text);
        let words = line.parts.is_empty() && !line.words.is_empty();
        let rows: Vec<Row> = match line.parts.last() {
            _ if words => actions(&line.words.join(" "), here),
            Some(last) => choices(last, line.last_goes_with(), here, &self.known),
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
            "/  title:  artist:  cover:  group:  move:  play  tracks:  (a word finds the rest · | between commands · Ctrl+Enter saves)"
        } else {
            "/  play  queue  star  rating:  tracks:  og:  credits  notes  (a word finds the rest · | between commands)"
        };
        if self.cl.field(ui, true, hint).changed() {
            self.notice = None;
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
        // `file:paste`, `file:choose`, `cover:choose`: the paths in the line first (what's
        // staged is what the line says).
        if enter && let Err(err) = self.expand_files(shell) {
            self.notice = Some(err);
            enter = false;
            save = false;
        }
        if enter && !self.cl.text.trim().is_empty() {
            let done = resolve(&self.cl.text, here, &self.known);
            let texts = pc::commands_text::<K>(&self.cl.text);
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
        if let Some(n) = pc::status(ui, self.notice.as_deref(), self.sending.count, &self.staged, can_edit) {
            self.staged.remove(n);
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

    /// `file:paste` becomes the copied files' paths; `file:choose` and `cover:choose` open the
    /// dialog and come out of the line (the paths are put in when chosen: Enter again).
    fn expand_files(&mut self, shell: &Shell) -> Result<(), String> {
        let dialog = pc::Dialog { title: "Files for the extra", kinds: ("Any file", &[]), many: true };
        pc::expand_files::<K>(&mut self.cl.text, shell, |p| p.key == K::File && p.arg, true, dialog, &self.chosen)?;
        let dialog = pc::Dialog { title: "A cover", kinds: ("Images", crate::ui::files::IMAGES), many: false };
        pc::expand_files::<K>(&mut self.cl.text, shell, |p| p.key == K::Cover && !p.minus, false, dialog, &self.chosen_cover)
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
                            Edit::Title(title) => form.data.title = title,
                            Edit::Kind(kind) => form.data.kind = kind,
                            Edit::Date(date) => form.data.release_date = date,
                            Edit::Why(why) => form.set_reason(&why),
                            Edit::Credit(c) => credit_into(&mut form.data.credits, c),
                            Edit::Uncredit(artist) => form.data.credits.retain(|r| r.artist_id != Some(artist)),
                            Edit::NoCredits => form.data.credits.clear(),
                            Edit::Thing(_) | Edit::Cover(_) | Edit::Group(_) => {}
                        }
                    }
                    if form.dirty() {
                        form.save(shell);
                        notices.push("Saving… (the album's form below says how it went)");
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

/// A credit onto the form's rows: one there already changes (its role, name, place), else
/// it's added (as primary if there's none yet, else featured, unless its role is said).
fn credit_into(rows: &mut Vec<CreditRow>, c: Credit) {
    let at = match c.artist.and_then(|id| rows.iter().position(|r| r.artist_id == Some(id))) {
        Some(at) => at,
        None => {
            credits::add_artist(rows, c.artist, c.name, c.role.as_deref() == Some("featured"));
            rows.len() - 1
        }
    };
    let row = &mut rows[at];
    if let Some(role) = c.role {
        row.role = role;
    }
    if let Some(alias) = c.alias {
        row.alias_id = alias.as_ref().map(|a| a.0);
        row.alias = alias.map(|a| a.1);
    }
    if let Some(to) = c.at {
        let row = rows.remove(at);
        rows.insert(to.min(rows.len()), row);
    }
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
        assert_eq!(does("play:#2"), [Ok(Does::Act(Act::Play(1)))]);
        assert_eq!(does("play:reprise"), [Ok(Does::Act(Act::Play(2)))]);
        assert!(does("play:glass")[0].as_ref().unwrap_err().contains("choose one"), "two titles have it");
        assert!(does("play:#4")[0].is_err());
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
        assert_eq!(does("edit:#2"), [Ok(Does::Act(Act::EditTracks(vec![1])))]);
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
        assert!(does("release:2020", &edit, &known).is_empty());
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
        let c = credit("artist:Bo role:prod #1", &known);
        assert_eq!((c.artist, c.role.as_deref(), c.at), (Some(id(BO)), Some("producer"), Some(0)));
        assert_eq!(credit("feat:bo", &known).role.as_deref(), Some("featured"));
        // Credited already: what changes; under one of its names.
        assert_eq!(credit("artist:Ann as:annie", &known).alias, Some(Some((Uuid::from_u128(9), "Annie".into()))));
        assert!(does("artist:Ann", &edit, &known)[0].as_ref().unwrap_err().contains("say what changes"));
        // Two of that name: chosen in the list. None: a new one, chosen there too.
        assert!(does("artist:Cy", &edit, &known)[0].as_ref().unwrap_err().contains("choose one"));
        assert!(does("artist:Dee", &edit, &known)[0].as_ref().unwrap_err().contains("New artist"));
        known.pins.insert((Pinned::Artist, "cy".into()), id(CY2));
        known.pins.insert((Pinned::Artist, "dee".into()), pc::NEW);
        assert_eq!(credit("artist:Cy", &known).artist, Some(id(CY2)));
        let c = credit("artist:Dee", &known);
        assert_eq!((c.artist, c.name.as_str()), (None, "Dee"));
        assert!(does("artist:Dee as:D", &edit, &known)[0].as_ref().unwrap_err().contains("new artist"));
        // Off: one it credits, or all.
        assert_eq!(does("-artist:Ann | -artist", &edit, &known), [Ok(Does::Edit(Edit::Uncredit(id(ANN)))), Ok(Does::Edit(Edit::NoCredits))]);
        assert!(does("-artist:Bo", &edit, &known)[0].is_err());
        // Onto the form's rows.
        let mut rows = credits::rows_from(&w.credits);
        credit_into(&mut rows, credit("artist:Bo #1", &known));
        credit_into(&mut rows, credit("artist:Ann role:producer", &known));
        assert_eq!(rows.iter().map(|r| (r.name.as_str(), r.role.as_str())).collect::<Vec<_>>(), [("Bo", "featured"), ("Ann", "producer")]);
        // The list: matches, then a new one; Tab goes on to its role.
        let part = commandline::parse::<K>("artist:b").parts.remove(0);
        let rows = choices(&part, &[], &edit, &known);
        assert!(matches!(rows.first().map(|r| &r.choice), Some(Choice::Pin(Pinned::Artist, name, a, Some(next))) if name == "Bo" && *a == id(BO) && next == "role:"));
        assert!(rows.last().is_some_and(|r| r.label == "New artist “b”"));
    }

    #[test]
    fn covers_groupings_and_moves() {
        let w = world();
        let edit = here(&w, true);
        let mut known = Known::default();
        let does = |text: &str, known: &Known| does(text, &edit, known);
        let cover = |c| Ok(Does::Edit(Edit::Cover(c)));
        assert_eq!(does("cover:#2 | cover:#2 #1 | -cover:#1", &known), [cover(CoverChange::Move(Uuid::from_u128(2), 0)), cover(CoverChange::Move(Uuid::from_u128(2), 0)), cover(CoverChange::Trash(Uuid::from_u128(1)))]);
        assert!(does("cover:#1", &known)[0].as_ref().unwrap_err().contains("main cover already"));
        assert!(does("cover:#3", &known)[0].is_err());
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
        let line = commandline::parse::<K>("group:x kind:era | kind:ep");
        assert_eq!(line.parts.iter().map(|p| (p.key, p.arg)).collect::<Vec<_>>(), [(K::Group, false), (K::Kind, true), (K::Kind, false)]);
        // Moving a track: at once (the tracklist's draft).
        assert_eq!(does("move:#3 #1", &known), [Ok(Does::Act(Act::Move(2, 0)))]);
        assert_eq!(does("edit:all | edit:#1 #3", &known), [Ok(Does::Act(Act::EditTracks(vec![0, 1, 2]))), Ok(Does::Act(Act::EditTracks(vec![0, 2])))]);
        assert!(does("move:#3", &known)[0].is_err());
        assert!(does("move:#3 #1", &Known::default()).len() == 1);
        assert!(resolve("move:#3 #1", &here(&w, false), &known)[0].as_ref().unwrap_err().contains("edit mode"));
        // The main cover on its tracks: found by words, staged as it is.
        known.pins.clear();
        assert!(actions("cover", &edit).iter().any(|r| matches!(&r.choice, Choice::Stage(_, _, Edit::Cover(CoverChange::OnTracks(c))) if *c == Uuid::from_u128(1))));
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
}
