//! The track page's commandline: the track editor's line (`ui::edit::editor`), with the
//! page's own keys added (`K::Page`). One language: the editor's keys (`title:`,
//! `artist:`, `cover:`, `source:`…) and these, which this module gives the meaning of
//! (what the pages share is in `page_commands`). In listen mode only the line is shown,
//! for listening and moving around; edit mode has the editor under it.
//!
//! * `play` plays it, then the rest of its album; `queue` puts it at the end of the queue,
//!   `queue:next` before what's queued. `star` (`star:no`, `-star`) and `rating:3.5`
//!   (`-rating` clears it) are yours, saved at once.
//! * `lyrics`, `credits`, `file`, `notes`, `extras`, `versions`, `tags`, `history` (and
//!   `merges` in edit mode) open that part of the page and scroll to it; one that's open is
//!   closed. Words find the rest (related tracks, sources, covers). `lyrics:English` (or
//!   `lyrics:#2`) shows that entry of its lyrics.
//! * Plain words find these too, its artists, albums, groupings and related tracks (to go
//!   to), its music video, its cover in full, and copying its link or a share link.
//!
//! Edit mode adds changes. They're staged as chips with the editor's and saved with them
//! (Ctrl+Enter), each on its own: one failing stays staged, saying why. Outside edit mode
//! they're not suggested, and typed they say why not.
//!
//! * `note:`, `-note:`, `extra:` (with `kind:`, `file:`, `description:`, `source:`,
//!   `visibility:`), `-extra:`: as on the artist and album pages.
//! * `og:` says its file is an original file (named as uploaded, or `og:Name`), `-og` that
//!   it isn't (`edit_files`). `file:#2` plays its second earlier file again, the file it
//!   plays now becoming an earlier one (`replace_file`).
//! * `unmerge:` undoes the newest merge into it (`replace_file`), `restore:#3` restores its
//!   third revision (`restore_revision`). The words *Duplicate this track* stage a copy of
//!   it (its page opens once it's made); *Undo the last origin change* puts back where its
//!   file came from.
//! * `version:Name` edits that version, or makes a new one by that name (a copy of the one
//!   edited) and edits it; `version:Name as:New name` renames one; the words *Show this
//!   version* make the one edited the track's shown one. These are done at once, as the
//!   versions bar does them, and wait for the editor's staged changes (they're for the
//!   version they were typed for).
//! * `merge`, `extra` (turn it into one), `edit lyrics` and `sync` (the lyrics' timing popup) open those tools.

use uuid::Uuid;

use super::album_commands::{place, rating, stars};
use super::page_commands::{self as pc, Arg, Change, Known, NoteDoes, Things, Whose};
use crate::account::Vocab;
use crate::catalog::Catalog;
use crate::commandline;
use crate::details::TrackDetail;
use crate::ui::edit::editor::K;
use crate::ui::Page;

type Part = commandline::Part<K>;

pub use acetate_core::editor::page_key::{PageKey, Reveal};

/// What the account may do here (besides edit mode).
#[derive(Debug, Clone, Copy, Default)]
pub struct May {
    /// `edit_metadata`: notes, extras, versions, a copy.
    pub metadata: bool,
    /// `edit_files`: whether its file is an original file.
    pub files: bool,
    /// `replace_file`: an earlier file played again, a merge undone.
    pub replace: bool,
    pub restore: bool,
    /// `upload` (and `edit_metadata`): a copy of it.
    pub upload: bool,
    /// `manage_access`: who sees an extra.
    pub access: bool,
    /// `edit_sources`: where its file came from.
    pub sources: bool,
}

/// What the page knows, for the commandline.
pub struct Here<'a> {
    pub id: Uuid,
    pub catalog: &'a Catalog,
    pub vocab: &'a Vocab,
    /// Its details (None: still loading).
    pub detail: Option<&'a TrackDetail>,
    pub edit_mode: bool,
    pub may: May,
    pub starred: bool,
    /// Yours, 0 to 10 (half stars).
    pub rating: i16,
    /// The parts of the page that are there now, and whether each was open.
    pub sections: &'a [Reveal],
    pub folds: &'a [(Reveal, bool)],
    /// Its music videos: (the video, the extra's name).
    pub videos: &'a [(Uuid, String)],
    /// The version the editor edits, and whether it has changes staged (versions wait).
    pub version: Option<Uuid>,
    pub staged: bool,
}

impl Here<'_> {
    fn track(&self) -> Option<crate::catalog::Track<'_>> {
        self.catalog.track(self.id)
    }

    fn loaded(&self) -> Result<&TrackDetail, String> {
        self.detail.ok_or_else(|| "its details are still loading…".to_string())
    }

    /// Its notes and extras, for what the pages share.
    fn things(&self) -> Things<'_> {
        Things {
            whose: Whose::Track(self.id),
            editing: editing(self).and_then(|_| if self.may.metadata { Ok(()) } else { Err("that needs the edit_metadata permission".into()) }),
            sources: &[],
            may_source: false,
            shapes: &self.vocab.source_shapes,
            extras: self.detail.map_or(&[][..], |d| &d.extras[..]),
            library_extras: &self.catalog.extras,
            may_access: self.may.access,
            notes: self.detail.map_or(&[][..], |d| &d.notes[..]),
        }
    }
}

/// What the page does for a command.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// Play it, then the rest of its album.
    Play,
    /// Queue it: at the end, or (true) next.
    Queue(bool),
    Star(bool),
    /// Your rating, 0 to 10.
    Rate(i16),
    CopyLink,
    ShareLink,
    Go(Page),
    /// Opened and scrolled to, or (open) closed.
    Reveal(Reveal),
    /// This entry of its lyrics shown.
    Lyrics(usize),
    /// A music video: (the video, its name).
    Watch(Uuid, String),
    /// Its cover, the original in full.
    Cover,
    Merge,
    /// The lyrics' form opens (a new entry if there are none) with the timing popup on.
    Sync,
    ToExtra,
    ToggleEdit,
    /// The note editor: for this note, or (None) a new one with this title.
    Note(Option<Uuid>, String),
    PasteNotes,
    /// The editor edits this version.
    EditVersion(Uuid),
    /// A new version by this name, a copy of the one edited (then edited).
    NewVersion(String),
    RenameVersion(Uuid, String),
    /// This version made the shown one.
    ShowVersion(Uuid),
}

/// A change staged with the editor's, sent on its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// A note or an extra.
    Thing(Change),
    /// Its file (this one) an original file named so, or (None) not one.
    Og(Uuid, Option<String>),
    /// This earlier file played again.
    UseFile(Uuid),
    /// This merge undone.
    Unmerge(Uuid),
    /// This revision restored.
    Restore(Uuid),
    /// A copy of the track (its page then).
    Duplicate,
    /// Its file (this one) from this snapshot again.
    UndoOrigin(Uuid, Uuid),
}

/// What a page command does.
#[derive(Debug, Clone, PartialEq)]
pub enum Does {
    Act(Act),
    Edit(Edit),
}

pub type Row = pc::Row<Act, Edit>;
type Choice = pc::Choice<Act, Edit>;

/// Whether it can be changed here (else why not).
fn editing(here: &Here) -> Result<(), String> {
    if here.edit_mode { Ok(()) } else { Err("changing the track is in edit mode: e".into()) }
}

/// Why one of the editor's keys does nothing in listen mode.
pub fn listening(key: K) -> String {
    format!("{}: changing the track is in edit mode: e", key.name())
}

/// An `extra:` argument's key, as the shared ones know it.
fn arg_of(key: K) -> Option<Arg> {
    Some(match key {
        K::Kind => Arg::Kind,
        K::Page(PageKey::File) => Arg::File,
        K::Page(PageKey::Description) => Arg::Description,
        K::Source => Arg::Source,
        K::Page(PageKey::Visibility) => Arg::Visibility,
        _ => return None,
    })
}

/// What showing a part of the page does: an open one closes, the rest open (scrolled to).
/// The lyrics' form always opens, the cursor in it.
fn reveal_label(here: &Here, r: Reveal) -> String {
    if r == Reveal::EditLyrics {
        return "Edit lyrics: the cursor in their text".into();
    }
    match here.folds.iter().find(|(f, _)| *f == r) {
        Some((_, true)) => format!("{}: close (currently opened)", r.title()),
        _ => format!("{}: open and scroll to it", r.title()),
    }
}

/// A lyrics entry as listed: "English · official".
fn lyrics_name(l: &crate::details::Lyrics, n: usize) -> String {
    let mut name = l.language.clone().unwrap_or_else(|| format!("Lyrics {}", n + 1));
    if l.is_official {
        name.push_str(" · official");
    }
    name
}

/// Its earlier files that can be played again: not purged, not the one it plays, each once.
fn earlier_files(d: &TrackDetail) -> Vec<&crate::details::FileChange> {
    let mut seen = std::collections::HashSet::new();
    d.file_history.iter().flatten().filter(|h| h.old_purged_at.is_none() && h.old_file_id != d.file.id && seen.insert(h.old_file_id)).collect()
}

/// An earlier file as listed: "original file “x.wav” · FLAC 900 kbps · replaced 2026-09-01".
fn earlier_label(h: &crate::details::FileChange) -> String {
    let name = h.old_original_filename.clone().map(|n| format!("original file “{n}”")).unwrap_or_else(|| h.old_uploaded_filename.clone());
    let codec = h.old_codec.clone().unwrap_or_default().to_uppercase();
    let bitrate = h.old_bitrate.map(|b| format!(" {b} kbps")).unwrap_or_default();
    let trash = if h.old_trashed_at.is_some() { " · in the trash" } else { "" };
    format!("{name} · {codec}{bitrate} · replaced {}{trash}", h.replaced_at.get(..10).unwrap_or(&h.replaced_at))
}

/// Its revisions (newest first), each with whether it's what's there now (the newest of
/// its kind and version: nothing to restore).
fn revisions(d: &TrackDetail) -> Vec<(&crate::details::Revision, bool)> {
    let all = d.revisions.as_deref().unwrap_or_default();
    all.iter().enumerate().map(|(n, r)| (r, !all[..n].iter().any(|o| o.kind == r.kind && o.variant_id == r.variant_id))).collect()
}

/// A revision as listed: "2026-09-01 12:00 · metadata · edit by ann".
fn revision_label(d: &TrackDetail, r: &crate::details::Revision) -> String {
    let what = match (r.kind.as_str(), d.variants.iter().find(|v| Some(v.id) == r.variant_id)) {
        ("external", _) => "status and albums".to_string(),
        (_, Some(v)) if d.variants.len() > 1 => format!("metadata ({})", v.label),
        _ => "metadata".to_string(),
    };
    let who = r.author.clone().unwrap_or_else(|| "the system".into());
    format!("{} · {what} · {} by {who}", r.created_at.get(..16).unwrap_or(&r.created_at).replace('T', " "), r.source)
}

/// The newest merge into it that isn't undone (only that one can be).
fn open_merge(d: &TrackDetail) -> Option<&crate::details::Merge> {
    d.merges.iter().find(|m| m.undone_at.is_none())
}

/// The last change of where its file came from, if it can be undone (the file still comes
/// from where it put it, and where it came from before is there): that snapshot.
fn origin_undo(d: &TrackDetail) -> Option<Uuid> {
    let c = d.origin_changes.as_deref()?.first()?;
    let now = d.file.origin.as_ref().map(|o| o.snapshot_id);
    (c.new.as_ref().map(|o| o.snapshot_id) == now).then(|| c.old.as_ref().map(|o| o.snapshot_id)).flatten()
}

/// What a command with one of the page's keys does, or why it can't.
pub fn resolve(command: &[Part], here: &Here, known: &Known) -> Result<(String, Does), String> {
    let main = &command[0];
    let K::Page(key) = main.key else { return Err(listening(main.key)) };
    let value = main.value.trim();
    let minus_ok = matches!(key, PageKey::Star | PageKey::Rating | PageKey::Note | PageKey::Extra | PageKey::Og);
    if main.minus && !minus_ok {
        return Err(format!("-{}: nothing to take off here", key.name()));
    }
    let act = |label: &str, act: Act| Ok((label.to_string(), Does::Act(act)));
    let edit = |label: String, edit: Edit| Ok((label, Does::Edit(edit)));
    let nothing_after = |label: &str, a: Act| if value.is_empty() && main.hashes.is_empty() { act(label, a) } else { Err(format!("{}: nothing after it", key.name())) };
    let reveal = |r: Reveal| if here.sections.contains(&r) { act(&reveal_label(here, r), Act::Reveal(r)) } else { Err(format!("no {} here", r.title().to_lowercase())) };
    match key {
        PageKey::Play if here.track().is_none() => Err("it isn't in the library you can see".into()),
        PageKey::Play => {
            let label = if here.track().is_some_and(|t| !t.albums().is_empty()) { "play it, then the rest of its album" } else { "play it" };
            nothing_after(label, Act::Play)
        }
        PageKey::Queue if here.track().is_none() => Err("it isn't in the library you can see".into()),
        PageKey::Queue => match value.to_lowercase().as_str() {
            "" | "end" | "last" => act("queued, at the end", Act::Queue(false)),
            "next" | "first" => act("queued, next", Act::Queue(true)),
            _ => Err("queue: next, or nothing (at the end)".into()),
        },
        PageKey::Star if main.minus => act("not starred", Act::Star(false)),
        PageKey::Star => match value.to_lowercase().as_str() {
            "" => act(if here.starred { "not starred" } else { "starred" }, Act::Star(!here.starred)),
            "yes" | "y" | "on" => act("starred", Act::Star(true)),
            "no" | "n" | "off" => act("not starred", Act::Star(false)),
            _ => Err("star: yes or no (or nothing: the other way)".into()),
        },
        PageKey::Rating if main.minus => act("no rating", Act::Rate(0)),
        PageKey::Rating if value.is_empty() => Err("rating: 0 to 5 stars (3.5 too)".into()),
        PageKey::Rating => rating(value).and_then(|r| act(&format!("your rating → {}", stars(r)), Act::Rate(r))),
        PageKey::Reveal(r) if !value.is_empty() || !main.hashes.is_empty() => Err(format!("{}: nothing after it", r.title().to_lowercase())),
        PageKey::Reveal(r) => reveal(r),
        PageKey::Lyrics if value.is_empty() && main.hashes.is_empty() => reveal(Reveal::Lyrics),
        PageKey::Lyrics => {
            let d = here.loaded()?;
            let n = match main.hashes.first() {
                Some(h) => place(h, d.lyrics.len(), "lyrics entries")?,
                None => {
                    let found: Vec<usize> = (0..d.lyrics.len()).filter(|&n| crate::catalog::query::loosely_in(value, &lyrics_name(&d.lyrics[n], n))).collect();
                    match found.as_slice() {
                        [n] => *n,
                        [] => return Err(format!("no lyrics like “{value}” (it has {})", d.lyrics.len())),
                        _ => return Err(format!("{} lyrics entries like “{value}”: choose one in the list", found.len())),
                    }
                }
            };
            act(&format!("its lyrics: {}", lyrics_name(&d.lyrics[n], n)), Act::Lyrics(n))
        }
        PageKey::File => match main.hashes.as_slice() {
            [] if value.is_empty() => reveal(Reveal::File),
            [h] if value.is_empty() => {
                editing(here)?;
                if !here.may.replace {
                    return Err("playing an earlier file needs the replace_file permission".into());
                }
                let d = here.loaded()?;
                let files = earlier_files(d);
                if files.is_empty() {
                    return Err("it has no earlier files to play again".into());
                }
                let n = place(h, files.len(), "earlier files")?;
                let label = format!("it plays earlier file #{} again ({}); the one it plays now becomes an earlier file", n + 1, earlier_label(files[n]));
                edit(label, Edit::UseFile(files[n].old_file_id))
            }
            _ => Err("file: opens the File part; file:#2 plays its second earlier file again (edit mode)".into()),
        },
        PageKey::Note => pc::note(value, main.minus, &here.things(), known).map(|(label, does)| {
            let does = match does {
                NoteDoes::Open(note, title) => Does::Act(Act::Note(note, title)),
                NoteDoes::Paste => Does::Act(Act::PasteNotes),
                NoteDoes::Hide(note) => Does::Edit(Edit::Thing(Change::HideNote(note))),
            };
            (label, does)
        }),
        PageKey::Extra => {
            let args: Vec<(Arg, &str)> = command[1..].iter().filter_map(|a| Some((arg_of(a.key)?, a.value.as_str()))).collect();
            pc::extra(value, main.minus, &args, &here.things(), known).map(|(label, change)| (label, Does::Edit(Edit::Thing(change))))
        }
        PageKey::Description | PageKey::Visibility => Err(format!("{}: goes after extra:Name", key.name())),
        PageKey::Og => {
            editing(here)?;
            if !here.may.files {
                return Err("whether its file is an original file: that needs the edit_files permission".into());
            }
            let f = &here.loaded()?.file;
            if main.minus {
                if f.original_filename.is_none() {
                    return Err("its file isn't an original file".into());
                }
                return edit("its file → not an original file".into(), Edit::Og(f.id, None));
            }
            let name = if value.is_empty() { f.original_filename.clone().unwrap_or_else(|| f.uploaded_filename.clone()) } else { value.to_string() };
            if f.original_filename.as_deref() == Some(name.as_str()) {
                return Err(format!("it's an original file named “{name}” already"));
            }
            edit(format!("its file → an original file named “{name}” (only if you know where it came from)"), Edit::Og(f.id, Some(name)))
        }
        PageKey::Sync => {
            editing(here)?;
            let new = here.detail.is_some_and(|d| d.lyrics.is_empty());
            nothing_after(if new { "time new lyrics (the sync popup)" } else { "time the lyrics (the sync popup)" }, Act::Sync)
        }
        PageKey::Version => version(command, here),
        PageKey::Unmerge => {
            editing(here)?;
            if !here.may.replace {
                return Err("undoing a merge changes which file a track plays: it needs the replace_file permission".into());
            }
            if !value.is_empty() {
                return Err("unmerge: alone (the newest merge into it is undone first)".into());
            }
            let m = open_merge(here.loaded()?).ok_or("nothing merged into it to undo")?;
            if !m.undo_blockers.is_empty() {
                return Err(format!("the merge of “{}” can't be undone: {}", m.from_title, m.undo_blockers.join("; ")));
            }
            edit(format!("undo the merge of “{}”: it's its own track again, as before the merge, and what the merge changed here is taken back", m.from_title), Edit::Unmerge(m.id))
        }
        PageKey::Restore => {
            editing(here)?;
            if !here.may.restore {
                return Err("restoring needs the restore_revision permission".into());
            }
            let d = here.loaded()?;
            let all = revisions(d);
            let [h] = main.hashes.as_slice() else { return Err("restore:#N restores its Nth revision (the list has them, newest first)".into()) };
            let n = place(h, all.len(), "revisions")?;
            let (r, now) = all[n];
            if now {
                return Err(format!("#{} is what's there now", n + 1));
            }
            edit(format!("restore #{} ({}): a new revision with its data", n + 1, revision_label(d, r)), Edit::Restore(r.id))
        }
    }
}

/// `version:Name` (edit it, or a new one), `version:Name as:New name` (renamed).
fn version(command: &[Part], here: &Here) -> Result<(String, Does), String> {
    editing(here)?;
    if !here.may.metadata {
        return Err("versions need the edit_metadata permission".into());
    }
    let d = here.loaded()?;
    if here.staged {
        return Err("apply or discard the changes first: they're for the version they were typed for".into());
    }
    let name = command[0].value.trim();
    if name.is_empty() {
        return Err("version: one of its versions (to edit it), or a new one's name".into());
    }
    let found = d.variants.iter().find(|v| v.label.eq_ignore_ascii_case(name));
    let edited = d.variants.iter().find(|v| Some(v.id) == here.version).map_or("the one edited", |v| v.label.as_str());
    let act = |label: String, a: Act| Ok((label, Does::Act(a)));
    match (found, command[1..].iter().find(|p| p.key == K::As)) {
        (Some(v), Some(p)) if p.value.trim().is_empty() => Err(format!("as: “{}”'s new name", v.label)),
        (Some(v), Some(p)) => act(format!("the version “{}” renamed “{}” (at once)", v.label, p.value.trim()), Act::RenameVersion(v.id, p.value.trim().to_string())),
        (None, Some(_)) => Err(format!("no version “{name}” to rename")),
        (Some(v), None) if Some(v.id) == here.version => Err(format!("“{}” is the one edited", v.label)),
        (Some(v), None) => act(format!("edit the version “{}”", v.label), Act::EditVersion(v.id)),
        (None, None) => act(format!("a new version “{name}”, a copy of “{edited}” (made at once), then edited"), Act::NewVersion(name.to_string())),
    }
}

/// The groups, asked for once `visibility:` is typed (it names them).
pub fn ask_groups(text: &str, here: &Here, shell: &crate::ui::Shell, known: &mut Known, groups: &crate::ui::edit::extras::Groups) {
    pc::ask_groups_if::<K>(text, |k| k == K::Page(PageKey::Visibility), &here.things(), shell, known, groups);
}

/// Each command of a line typed in listen mode: what it does, or why it can't (the
/// editor's keys can't, there).
pub fn resolve_line(text: &str, here: &Here, known: &Known) -> Vec<Result<(String, Does), String>> {
    commandline::parse::<K>(text).commands().map(|c| resolve(c, here, known)).collect()
}

/// Whether the part being typed is the page's (its key, or an argument of one of its keys).
pub fn is_page_part(part: &Part, group: &[Part]) -> bool {
    match group.first() {
        Some(owner) if part.arg => matches!(owner.key, K::Page(_)),
        _ => matches!(part.key, K::Page(_)),
    }
}

/// The choices for the value being typed (`group`: what it goes with, if it's an argument).
pub fn choices(part: &Part, group: &[Part], here: &Here, known: &Known) -> Vec<Row> {
    let value = part.value.trim();
    let typed = value.to_lowercase();
    let things = here.things();
    let can = editing(here).is_ok();
    let raw = |label: String, sub: &str, text: String| Row { label, sub: sub.into(), choice: Choice::Raw(text, None) };
    if let Some(arg) = arg_of(part.key).filter(|_| group.first().is_some_and(|g| g.key == K::Page(PageKey::Extra))) {
        return pc::extra_arg_rows(arg, value, &things, known);
    }
    let K::Page(key) = part.key else { return Vec::new() };
    let d = here.detail;
    match key {
        PageKey::Note => pc::note_rows(part.minus, value, &things),
        PageKey::Extra => pc::extra_rows(part.minus, value, &things),
        PageKey::Queue => [("end", "at the end of the queue"), ("next", "before what's queued")].into_iter().filter(|(w, _)| w.starts_with(&typed)).map(|(w, sub)| raw(w.into(), sub, w.into())).collect(),
        PageKey::Star if typed.is_empty() && !part.minus => [("yes", "starred"), ("no", "not starred")].into_iter().map(|(w, sub)| raw(w.into(), sub, w.into())).collect(),
        PageKey::Lyrics if part.hashes.is_empty() => d
            .map(|d| &d.lyrics[..])
            .unwrap_or_default()
            .iter()
            .enumerate()
            .filter(|(n, l)| typed.is_empty() || crate::catalog::query::loosely_in(value, &lyrics_name(l, *n)))
            .map(|(n, l)| raw(format!("#{} {}", n + 1, lyrics_name(l, n)), "show these", format!("#{}", n + 1)))
            .collect(),
        PageKey::File if can && here.may.replace && part.hashes.is_empty() && typed.is_empty() => d
            .map(earlier_files)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(n, h)| raw(format!("#{} {}", n + 1, earlier_label(h)), "plays it again", format!("#{}", n + 1)))
            .collect(),
        PageKey::Restore if can && here.may.restore && part.hashes.is_empty() => {
            let Some(d) = d else { return Vec::new() };
            revisions(d).into_iter().enumerate().filter(|(_, (_, now))| !now).map(|(n, (r, _))| raw(format!("#{} {}", n + 1, revision_label(d, r)), "restore it", format!("#{}", n + 1))).collect()
        }
        PageKey::Og if can && here.may.files && typed.is_empty() && !part.minus => {
            let Some(f) = d.map(|d| &d.file) else { return Vec::new() };
            let name = f.original_filename.clone().unwrap_or_else(|| f.uploaded_filename.clone());
            vec![Row { label: name.clone(), sub: "its name (as uploaded, or another)".into(), choice: Choice::Value(name) }]
        }
        PageKey::Version if can && here.may.metadata => {
            let Some(d) = d else { return Vec::new() };
            let mut rows: Vec<Row> = d
                .variants
                .iter()
                .filter(|v| typed.is_empty() || v.label.to_lowercase().contains(&typed))
                .map(|v| {
                    let mut sub = if Some(v.id) == here.version { "edited now".to_string() } else { "edit it".to_string() };
                    if d.primary_variant_id == Some(v.id) {
                        sub.push_str(" · shown");
                    }
                    Row { label: v.label.clone(), sub: format!("{sub} · Tab, as: renames it"), choice: Choice::Value(v.label.clone()) }
                })
                .collect();
            if !value.is_empty() && !d.variants.iter().any(|v| v.label.eq_ignore_ascii_case(value)) {
                rows.push(Row { label: format!("New version “{value}”"), sub: "a copy of the one edited".into(), choice: Choice::Value(value.to_string()) });
            }
            rows
        }
        _ => Vec::new(),
    }
}

/// The actions plain words find (and the keys to type, for the rest).
pub fn actions(typed: &str, here: &Here) -> Vec<Row> {
    let mut rows = Vec::new();
    let found = |label: &str, also: &[&str]| crate::catalog::query::loosely_in(typed, label) || also.iter().any(|a| a.starts_with(&typed.to_lowercase()));
    let act = |label: &str, sub: &str, act: Act| Row { label: label.into(), sub: sub.into(), choice: Choice::Act(act) };
    let insert = |label: &str, sub: &str, key: &str| Row { label: label.into(), sub: sub.into(), choice: Choice::Insert(key.into()) };
    let track = here.track();
    if track.is_some() {
        if found("Play", &[]) {
            let sub = if track.is_some_and(|t| !t.albums().is_empty()) { "it, then the rest of its album" } else { "it" };
            rows.push(act("Play", sub, Act::Play));
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
    for (video, name) in here.videos {
        let label = if here.videos.len() > 1 { format!("Watch “{name}”") } else { "Watch the video".to_string() };
        if found(&label, &["video", "watch"]) {
            rows.push(act(&label, "its music video", Act::Watch(*video, name.clone())));
        }
    }
    if track.is_some_and(|t| t.cover().is_some()) && found("Cover in full", &["cover", "original", "image"]) {
        rows.push(act("Cover in full", "the original image", Act::Cover));
    }
    for &section in here.sections {
        if found(section.title(), &[section.title().split(' ').next().unwrap_or("")]) {
            let label = reveal_label(here, section);
            let sub = label.rsplit(": ").next().unwrap_or("");
            rows.push(act(section.title(), sub, Act::Reveal(section)));
        }
    }
    if let Some(d) = here.detail.filter(|d| d.lyrics.len() > 1)
        && found("Lyrics: another entry", &["lyrics", "language"])
    {
        let names: Vec<String> = d.lyrics.iter().enumerate().map(|(n, l)| lyrics_name(l, n)).collect();
        rows.push(insert("Lyrics: another entry (lyrics:)", &names.join(", "), "lyrics:"));
    }
    // Its artists, albums, groupings and related tracks: their pages.
    if !typed.trim().is_empty() {
        let mut go = |label: &str, sub: &str, page: Page| {
            if found(label, &[]) && !rows.iter().any(|r| matches!(&r.choice, Choice::Act(Act::Go(p)) if *p == page)) {
                rows.push(act(label, sub, Act::Go(page)));
            }
        };
        if let Some(t) = track {
            for c in t.credits() {
                if let Some(a) = here.catalog.artist(c.0) {
                    go(&a.name, "its page", Page::Artist(a.id));
                    if let Some(alias) = &c.2 {
                        go(alias, &format!("{}: its page", a.name), Page::Artist(a.id));
                    }
                }
            }
            for m in t.albums() {
                if let Some(a) = here.catalog.album(m.0) {
                    go(a.title(), &format!("{}: its page", a.kind()), Page::Album(a.id()));
                }
            }
            for g in t.groupings().iter().filter_map(|id| here.catalog.grouping(*id)) {
                go(&g.name, &format!("{}: its page", g.kind), Page::Grouping(g.id));
            }
        }
        if let Some(d) = here.detail {
            for r in &d.relations {
                go(&r.title, &format!("{}: related", r.artist_display), Page::Track(r.track_id));
            }
            for s in &d.file_shared_with {
                go(&s.title, &format!("{}: plays the same file", s.artist_display), Page::Track(s.id));
            }
        }
    }
    let edit_label = if here.edit_mode { "Edit mode off" } else { "Edit mode on" };
    if found(edit_label, &["edit"]) {
        rows.push(act(edit_label, "Alt+E, anywhere", Act::ToggleEdit));
    }
    if editing(here).is_err() {
        return rows;
    }
    let d = here.detail;
    if found("Merge with another track", &["merge"]) {
        rows.push(act("Merge with another track…", "Ctrl+M", Act::Merge));
    }
    if d.is_some() && found("Sync the lyrics", &["sync", "time", "lyrics"]) {
        rows.push(act("Sync the lyrics", "time the lines while listening (sync)", Act::Sync));
    }
    if found("Turn into an extra", &["extra"]) {
        rows.push(act("Turn into an extra…", "vocals, a stem, a clip of another song", Act::ToExtra));
    }
    let mut keys = vec![
        ("note:", "Note (note:)", "a new one, or one of its notes to edit", here.may.metadata),
        ("extra:", "Extra (extra:)", "a new one, one of its own to change, or one in the library", here.may.metadata),
        ("version:", "Version (version:)", "edit another, or a new one by that name", here.may.metadata && d.is_some()),
        ("og:", "Original file (og:)", "its file is the artist's own (-og: it isn't)", here.may.files && d.is_some()),
        ("file:#", "Play an earlier file again (file:)", "#N: the file it plays now becomes an earlier one", here.may.replace && d.is_some_and(|d| !earlier_files(d).is_empty())),
        ("restore:#", "Restore a revision (restore:)", "#N, from its history", here.may.restore && d.is_some_and(|d| revisions(d).iter().any(|r| !r.1))),
    ];
    if d.and_then(open_merge).is_some_and(|m| here.may.replace && m.undo_blockers.is_empty()) {
        keys.push(("unmerge:", "Undo the merge (unmerge:)", "the newest merge into it", true));
    }
    for (key, label, sub, may) in keys {
        if may && found(label, &[key.trim_end_matches(['#', ':'])]) {
            rows.push(insert(label, sub, key));
        }
    }
    let Some(d) = d else { return rows };
    if here.may.metadata && !here.staged {
        let edited = d.variants.iter().find(|v| Some(v.id) == here.version);
        if let Some(v) = edited.filter(|v| d.primary_variant_id != Some(v.id))
            && found("Show this version", &["version", "shown"])
        {
            rows.push(act("Show this version", &format!("“{}”: the track takes its title, artists and the rest (at once)", v.label), Act::ShowVersion(v.id)));
        }
        if let Some(v) = edited
            && found("Rename this version", &["version", "rename"])
        {
            rows.push(insert("Rename this version", &format!("“{}”", v.label), &format!("version:{} as:", crate::ui::edit::editor::quoted(&v.label))));
        }
    }
    if here.may.upload && found("Duplicate this track", &["copy", "duplicate"]) {
        let label = format!(
            "a new track: “{}” with this metadata and status, playing the same file, with the same earlier files, copies of the covers and the same access; not its albums, relations, lyrics, sources or plays (its page opens once it's made)",
            d.title
        );
        rows.push(Row { label: "Duplicate this track".into(), sub: "staged".into(), choice: Choice::Stage("Duplicate this track".into(), label, Edit::Duplicate) });
    }
    if here.may.sources
        && let Some(snapshot) = origin_undo(d)
        && found("Undo the last origin change", &["origin", "undo"])
    {
        let label = "where its file came from: back to where it was before the last change".to_string();
        rows.push(Row { label: "Undo the last origin change".into(), sub: "staged".into(), choice: Choice::Stage("Undo the last origin change".into(), label, Edit::UndoOrigin(d.file.id, snapshot)) });
    }
    rows
}

/// A change sent on its own; `copied`: where a copy made goes (its page is opened). Ok:
/// what's worth saying, if anything.
pub async fn send(api: &crate::api::Api, ctx: &eframe::egui::Context, track: Uuid, edit: Edit, copied: &std::sync::Mutex<Option<Uuid>>) -> Result<Option<String>, String> {
    use reqwest::Method;
    use serde_json::{json, Value};
    let call = |method: Method, path: String, body: Value| async move { api.send_json::<Value>(method, &path, Some(body)).await.map_err(|e| e.message) };
    match edit {
        Edit::Thing(change) => pc::send(api, ctx, Whose::Track(track), &change).await,
        Edit::Og(file, name) => call(Method::PUT, format!("/api/files/{file}/original-filename"), json!({ "original_filename": name, "reason": "" })).await.map(|_| None),
        Edit::UseFile(file) => call(Method::POST, format!("/api/tracks/{track}/use-file"), json!({ "previous_file": file, "reason": "" })).await.map(|_| None),
        Edit::Unmerge(merge) => call(Method::POST, format!("/api/track-merges/{merge}/undo"), json!({ "reason": "" })).await.map(|_| Some("Undone: the merged track is its own again.".into())),
        Edit::Restore(revision) => call(Method::POST, format!("/api/revisions/{revision}/restore"), json!({ "reason": "" })).await.map(|_| Some("Restored.".into())),
        Edit::UndoOrigin(file, snapshot) => call(Method::PUT, format!("/api/files/{file}/origin"), json!({ "snapshot_id": snapshot, "reason": "undo" })).await.map(|_| None),
        Edit::Duplicate => {
            let made = call(Method::POST, format!("/api/tracks/{track}/duplicate"), json!({ "reason": "" })).await?;
            *copied.lock().unwrap() = made["track_id"].as_str().and_then(|id| id.parse().ok());
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACK: &str = "00000000-0000-7000-8000-000000000001";
    const ANN: &str = "00000000-0000-7000-8000-00000000a001";
    const LP: &str = "00000000-0000-7000-8000-00000000b001";

    fn id(s: &str) -> Uuid {
        s.parse().unwrap()
    }

    fn catalog() -> Catalog {
        let body = serde_json::json!({
            "format": 10,
            "tracks": {"columns": ["id", "title", "artist_display", "credits", "albums", "duration_ms", "file_id", "cover_id", "cover_key",
                                    "release_date", "year", "status", "genres", "created_at", "is_restricted", "codec", "groupings", "platforms", "original_filename", "bitrate", "sample_rate", "sha256"],
                       "rows": [[TRACK, "Glass Harbor", "Ann", [[ANN, "primary", null]], [[LP, 1, 2]], 1000,
                                 "00000000-0000-7000-8000-0000000000f1", null, null, null, null, "released", [], "", false, "flac"]]},
            "albums": {"columns": ["id", "title", "kind", "artist_display", "credits", "release_date", "year", "cover_id", "cover_key", "created_at", "track_count", "duration_ms", "groupings"],
                       "rows": [[LP, "Long Player", "album", "Ann", [[ANN, "primary", null]], null, null, null, null, "", 1, 1000, []]]},
            "artists": {"columns": ["id", "name", "disambiguation", "aliases", "track_count", "album_count"],
                        "rows": [[ANN, "Ann", "", [], 1, 1]]},
            "annotations": {"columns": ["kind", "id", "play_count", "last_played_at", "rating", "starred_at"], "rows": []},
            "issues": {"columns": ["track_id", "flags"], "rows": []},
            "groupings": {"columns": ["id", "kind", "name", "track_count", "album_count", "artists"], "rows": []},
            "playlists": []
        });
        Catalog::build(crate::catalog::model::parse(body.to_string().as_bytes()).unwrap())
    }

    /// Two versions (the first shown), two lyrics entries, an earlier file, a merge, three
    /// revisions (the newest of each kind is now).
    fn detail() -> TrackDetail {
        let u = |n: u128| Uuid::from_u128(n);
        serde_json::from_value(serde_json::json!({
            "id": TRACK, "title": "Glass Harbor", "artist_display": "Ann", "status": "released",
            "primary_variant_id": u(2), "external_revision_id": u(11),
            "file": { "id": u(3), "sha256": "", "uploaded_filename": "glass.flac", "size_bytes": 0, "uploaded_at": "",
                      "origin": { "snapshot_id": u(60), "source_id": u(61), "platform": "soundcloud", "external_id": null } },
            "file_history": [
                { "old_file_id": u(30), "replaced_at": "2026-09-01T00:00:00Z", "reason": "", "old_original_filename": "Glass Harbor.wav",
                  "old_uploaded_filename": "x.wav", "old_codec": "wav", "old_bitrate": null, "old_size_bytes": 0, "old_trashed_at": null, "old_purged_at": null },
                { "old_file_id": u(31), "replaced_at": "2026-08-01T00:00:00Z", "reason": "", "old_original_filename": null,
                  "old_uploaded_filename": "gone.mp3", "old_codec": "mp3", "old_bitrate": 128, "old_size_bytes": 0, "old_trashed_at": null, "old_purged_at": "2026-09-02" }
            ],
            "origin_changes": [{ "changed_at": "2026-09-03", "changed_by": null, "reason": "",
                                 "old": { "snapshot_id": u(50), "platform": "youtube", "external_id": null },
                                 "new": { "snapshot_id": u(60), "platform": "soundcloud", "external_id": null } }],
            "variants": [
                { "id": u(2), "label": "File tags", "revision_id": u(10), "data": {} },
                { "id": u(4), "label": "Leak title", "revision_id": u(20), "data": {} },
            ],
            "lyrics": [
                { "id": u(70), "is_official": true, "is_synced": false, "language": "English", "body": "", "revision_id": u(71) },
                { "id": u(72), "is_official": false, "is_synced": false, "language": "Finnish", "body": "", "revision_id": u(73) },
            ],
            "merges": [{ "id": u(80), "from_track_id": u(81), "from_title": "Glass Harbour", "merged_at": "2026-09-04", "merged_by": null, "undone_at": null }],
            "revisions": [
                { "id": u(90), "kind": "internal", "variant_id": u(2), "created_at": "2026-09-05T10:00:00Z", "author": "ann", "source": "edit" },
                { "id": u(91), "kind": "external", "variant_id": null, "created_at": "2026-09-04T10:00:00Z", "author": null, "source": "import" },
                { "id": u(92), "kind": "internal", "variant_id": u(2), "created_at": "2026-09-03T10:00:00Z", "author": null, "source": "import" },
            ],
        }))
        .unwrap()
    }

    fn may() -> May {
        May { metadata: true, files: true, replace: true, restore: true, upload: true, access: true, sources: true }
    }

    fn here<'a>(catalog: &'a Catalog, vocab: &'a Vocab, d: &'a TrackDetail, edit_mode: bool) -> Here<'a> {
        Here {
            id: id(TRACK),
            catalog,
            vocab,
            detail: Some(d),
            edit_mode,
            may: may(),
            starred: false,
            rating: 0,
            sections: &[Reveal::Lyrics, Reveal::Credits, Reveal::File, Reveal::History],
            folds: &[(Reveal::Lyrics, true), (Reveal::File, false)],
            videos: &[],
            version: Some(Uuid::from_u128(2)),
            staged: false,
        }
    }

    fn does(text: &str, here: &Here) -> Vec<Result<Does, String>> {
        resolve_line(text, here, &Known::default()).into_iter().map(|r| r.map(|d| d.1)).collect()
    }

    #[test]
    fn page_keys_are_the_editors_too() {
        use crate::ui::edit::editor::parse_line;
        let line = parse_line("play: | title:Intro: Part 2 | extra:Stems kind:stems source:a friend | -og");
        let keys: Vec<(K, bool)> = line.parts.iter().map(|p| (p.key, p.arg)).collect();
        assert_eq!(
            keys,
            [
                (K::Page(PageKey::Play), false),
                (K::Title, false),
                (K::Page(PageKey::Extra), false),
                (K::Kind, true),
                (K::Source, true),
                (K::Page(PageKey::Og), false)
            ]
        );
        assert_eq!(line.parts[1].value, "Intro: Part 2");
        // Without a colon, a word (plain words find actions).
        assert_eq!(parse_line("play").words, ["play"]);
        // `rate` is the page's here (in a filter it's the sample rate).
        assert_eq!(parse_line("rate:4").parts[0].key, K::Page(PageKey::Rating));
        assert_eq!(parse_line("cover:paste").parts[0].key, K::Cover, "the editor's own come first");
    }

    #[test]
    fn listening_keys_work_in_listen_mode() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let h = here(&c, &v, &d, false);
        assert_eq!(does("play:", &h), [Ok(Does::Act(Act::Play))]);
        assert_eq!(does("queue:next | star: | rating:3.5", &h), [Ok(Does::Act(Act::Queue(true))), Ok(Does::Act(Act::Star(true))), Ok(Does::Act(Act::Rate(7)))]);
        assert_eq!(does("-rating", &h), [Ok(Does::Act(Act::Rate(0)))]);
        assert_eq!(does("lyrics:finn", &h), [Ok(Does::Act(Act::Lyrics(1)))]);
        assert_eq!(does("lyrics:#1", &h), [Ok(Does::Act(Act::Lyrics(0)))]);
        assert_eq!(does("file:", &h), [Ok(Does::Act(Act::Reveal(Reveal::File)))]);
        assert!(does("tags:", &h)[0].as_ref().is_err_and(|e| e.contains("no tags in the file")));
        // An open one is closed, and says so.
        let (label, _) = resolve(&parse("lyrics:").parts, &h, &Known::default()).unwrap();
        assert!(label.contains("close"), "{label}");
    }

    fn parse(text: &str) -> commandline::Line<K> {
        crate::ui::edit::editor::parse_line(text)
    }

    #[test]
    fn sync_is_in_edit_mode() {
        let (catalog, vocab, d) = (catalog(), Vocab::default(), detail());
        let h = here(&catalog, &vocab, &d, false);
        assert!(does("sync:", &h)[0].is_err());
        let h = here(&catalog, &vocab, &d, true);
        assert_eq!(does("sync:", &h), [Ok(Does::Act(Act::Sync))]);
    }

    #[test]
    fn changes_say_edit_mode_outside_it() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let h = here(&c, &v, &d, false);
        for text in ["title:New", "og:", "file:#1", "unmerge:", "restore:#3", "note:Hi", "extra:Stems", "version:Other"] {
            let out = does(text, &h);
            assert!(out[0].as_ref().is_err_and(|e| e.contains("edit mode: e")), "{text}: {out:?}");
        }
        // Not suggested either.
        let rows = actions("merge", &h);
        assert!(rows.iter().all(|r| !r.label.starts_with("Merge")));
        assert!(choices(&parse("file:").parts[0], &[], &h, &Known::default()).is_empty());
    }

    #[test]
    fn changes_in_edit_mode() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let h = here(&c, &v, &d, true);
        let u = Uuid::from_u128;
        assert_eq!(does("og:", &h), [Ok(Does::Edit(Edit::Og(u(3), Some("glass.flac".into()))))]);
        assert!(does("-og", &h)[0].is_err(), "not an original file already");
        // The purged earlier file isn't one to play again.
        assert_eq!(does("file:#1", &h), [Ok(Does::Edit(Edit::UseFile(u(30))))]);
        assert!(does("file:#2", &h)[0].as_ref().is_err_and(|e| e.contains("it has 1")));
        assert_eq!(does("unmerge:", &h), [Ok(Does::Edit(Edit::Unmerge(u(80))))]);
        // The newest of its kind is what's there now.
        assert!(does("restore:#1", &h)[0].as_ref().is_err_and(|e| e.contains("there now")));
        assert!(does("restore:#2", &h)[0].as_ref().is_err_and(|e| e.contains("there now")));
        assert_eq!(does("restore:#3", &h), [Ok(Does::Edit(Edit::Restore(u(92))))]);
        assert_eq!(does("note:Hi", &h), [Ok(Does::Act(Act::Note(None, "Hi".into())))]);
        assert!(matches!(&does("extra:Stems kind:stems", &h)[0], Ok(Does::Edit(Edit::Thing(Change::Extra(e)))) if e.name == "Stems" && e.about.as_ref().is_some_and(|a| a.0 == "stems")));
        assert!(does("description:x", &h)[0].as_ref().is_err_and(|e| e.contains("after extra:")));
    }

    #[test]
    fn versions_wait_for_staged_changes() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let mut h = here(&c, &v, &d, true);
        let u = Uuid::from_u128;
        assert_eq!(does("version:leak title", &h), [Ok(Does::Act(Act::EditVersion(u(4))))]);
        assert_eq!(does("version:Single edit", &h), [Ok(Does::Act(Act::NewVersion("Single edit".into())))]);
        assert_eq!(does("version:Leak title as:Leaked", &h), [Ok(Does::Act(Act::RenameVersion(u(4), "Leaked".into())))]);
        assert!(does("version:File tags", &h)[0].is_err(), "edited already");
        h.staged = true;
        assert!(does("version:Leak title", &h)[0].as_ref().is_err_and(|e| e.contains("apply or discard")));
    }

    #[test]
    fn words_find_actions_and_pages() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let h = here(&c, &v, &d, true);
        let labels = |typed: &str| actions(typed, &h).into_iter().map(|r| r.label).collect::<Vec<_>>();
        assert!(labels("ann").contains(&"Ann".to_string()));
        assert!(labels("long").contains(&"Long Player".to_string()));
        assert!(labels("dup").contains(&"Duplicate this track".to_string()));
        assert!(labels("origin").contains(&"Undo the last origin change".to_string()));
        assert!(labels("merge").contains(&"Merge with another track…".to_string()));
        assert!(labels("unmerge").contains(&"Undo the merge (unmerge:)".to_string()));
        assert!(labels("history").contains(&"History".to_string()));
        // The version edited is shown already: nothing to show.
        assert!(!labels("show").contains(&"Show this version".to_string()));
        let rename = actions("rename", &h).into_iter().find(|r| r.label == "Rename this version").unwrap();
        assert!(matches!(rename.choice, Choice::Insert(ref t) if t == "version:File tags as:"));
    }

    #[test]
    fn choices_for_page_keys() {
        let (c, v, d) = (catalog(), Vocab::default(), detail());
        let h = here(&c, &v, &d, true);
        let rows = |text: &str| {
            let line = parse(text);
            choices(line.parts.last().unwrap(), line.last_goes_with(), &h, &Known::default()).into_iter().map(|r| r.label).collect::<Vec<_>>()
        };
        assert_eq!(rows("lyrics:"), ["#1 English · official", "#2 Finnish"]);
        assert_eq!(rows("file:").len(), 1);
        assert_eq!(rows("restore:").len(), 1);
        assert_eq!(rows("version:"), ["File tags", "Leak title"]);
        assert_eq!(rows("version:Demo"), ["New version “Demo”"]);
        assert!(rows("extra:Stems kind:st").contains(&"stems".to_string()));
        assert!(is_page_part(&parse("extra:x kind:").parts[1], parse("extra:x kind:").last_goes_with()));
        assert!(!is_page_part(&parse("album:x kind:").parts[1], parse("album:x kind:").last_goes_with()));
    }
}
