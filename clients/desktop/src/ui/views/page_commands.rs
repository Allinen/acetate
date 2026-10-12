//! What the page commandlines share (`artist_commands`, `album_commands`,
//! `track_commands`): a page's
//! sources, extras and notes (`source:`, `extra:`, `note:`, and taking them off), the paths
//! and files typed for them, the rows of the list and what taking one does, the changes
//! staged, and sending the ones saved on their own. Each page has its own keys (a
//! `commandline::Keys` table) and hands its parts' values to these.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};
use uuid::Uuid;

use crate::account::SourceShape;
use crate::commandline::schema::{self, Node, Spec};
use crate::details::Source;
use crate::ui::edit::commandline::{self as cl, CommandLine};
use crate::ui::edit::extras::GroupChoice;
use crate::ui::theme::PALETTE as P;
use crate::ui::Shell;

/// Whose page it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whose {
    Artist(Uuid),
    Album(Uuid),
    Track(Uuid),
}

impl Whose {
    /// "artist", "album", "track": in what's said, and the kind of its sources.
    pub fn word(self) -> &'static str {
        match self {
            Whose::Artist(_) => "artist",
            Whose::Album(_) => "album",
            Whose::Track(_) => "track",
        }
    }

    pub fn extras(self) -> crate::ui::edit::extras::Owner {
        match self {
            Whose::Artist(id) => crate::ui::edit::extras::Owner::Artist(id),
            Whose::Album(id) => crate::ui::edit::extras::Owner::Album(id),
            Whose::Track(id) => crate::ui::edit::extras::Owner::Track(id),
        }
    }

    fn sources_path(self) -> String {
        match self {
            Whose::Artist(id) => format!("/api/artists/{id}/sources"),
            Whose::Album(id) => format!("/api/albums/{id}/sources"),
            Whose::Track(id) => format!("/api/tracks/{id}/sources"),
        }
    }

    /// Who sees a new extra when it isn't said: an artist's, like the artist.
    fn default_seen(self) -> &'static str {
        match self {
            Whose::Artist(_) => "seen like the artist",
            Whose::Album(_) => "seen as by default (like its first track)",
            Whose::Track(_) => "seen like the track",
        }
    }
}

/// What the page has of these, and whether it can change them.
pub struct Things<'a> {
    pub whose: Whose,
    /// Ok: it can be changed here; else why not.
    pub editing: Result<(), String>,
    /// Its sources, and whether they can be changed here (edit mode, `edit_sources`).
    pub sources: &'a [Source],
    pub may_source: bool,
    /// Every platform's sources' fields (the page's are those of its kind).
    pub shapes: &'a [SourceShape],
    /// Its extras (taken off ones too), every extra in the library, and whether who sees
    /// one can be said here (`manage_access`).
    pub extras: &'a [crate::details::Extra],
    pub library_extras: &'a [crate::catalog::model::Extra],
    /// The kinds of extra the library has.
    pub extra_kinds: &'a [String],
    pub may_access: bool,
    /// Its notes (taken off ones too).
    pub notes: &'a [crate::details::Note],
}

impl Things<'_> {
    /// "this artist", "this album".
    fn this(&self) -> String {
        format!("this {}", self.whose.word())
    }
}

/// What a name chosen in the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pinned {
    Extra,
    Note,
    Artist,
    Grouping,
    /// Where a cover is copied from.
    Album,
    Track,
}

/// A new artist chosen in the list ("New artist “…”"): pinned as this.
pub const NEW: Uuid = Uuid::nil();

/// What the commandline knows besides the page: names chosen in its list (by `Pinned`'s
/// tag, shared with schema commandlines' pins), and the groups (to say who sees an extra;
/// None: not asked for yet).
#[derive(Default)]
pub struct Known {
    pub pins: schema::Pins,
    pub groups: Option<Vec<GroupChoice>>,
}

impl Known {
    pub fn pinned(&self, kind: Pinned, name: &str) -> Option<Uuid> {
        self.pins.get_tag(kind.tag(), name)
    }

    #[cfg(test)]
    pub fn pin(&mut self, kind: Pinned, name: &str, id: Uuid) {
        self.pins.set_tag(kind.tag(), name, id);
    }
}

impl Pinned {
    /// Its tag among the pins (an artist's is the same as `schema::Kind::Artist`'s).
    pub fn tag(self) -> &'static str {
        match self {
            Pinned::Extra => "extra",
            Pinned::Note => "note",
            Pinned::Artist => schema::Kind::Artist.pin_tag(),
            Pinned::Grouping => schema::Kind::Grouping.pin_tag(),
            Pinned::Album => schema::Kind::Album.pin_tag(),
            Pinned::Track => schema::Kind::Track.pin_tag(),
        }
    }
}

/// A change saved on its own (not through the page's form).
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A source added: its platform and data.
    AddSource(String, serde_json::Value),
    /// A source to the trash.
    TrashSource(Uuid),
    Extra(ExtraChange),
    /// An extra not on this page's artist or album anymore.
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
    /// Linked here already.
    pub linked: bool,
    pub files: Vec<PathBuf>,
    /// One there already: its name, kind, description and source as they'll be (its own,
    /// with what was typed on top), when any of them changes. A new one: all of them.
    pub about: Option<(String, String, String)>,
    pub seen: Option<Seen>,
}

/// Who sees an extra.
#[derive(Debug, Clone, PartialEq)]
pub enum Seen {
    Everyone,
    /// The server's default (a new one only): like the artist, or the first track.
    Default,
    /// Admins and these groups.
    Groups(Vec<Uuid>),
}

/// A staged change: what it was typed as, what it does, and why it wasn't saved (if it
/// was tried).
#[derive(Debug, Clone)]
pub struct Staged<E> {
    pub text: String,
    pub label: String,
    pub edit: E,
    pub failed: Option<String>,
}

/// A row of the list under the commandline: what it says, and what taking it does.
pub struct Row<A, E> {
    pub label: String,
    pub sub: String,
    pub choice: Choice<A, E>,
}

#[derive(Debug, Clone)]
pub enum Choice<A, E> {
    /// The value being typed becomes this.
    Value(String),
    /// The commandline becomes this (a key to go on typing after).
    Insert(String),
    Act(A),
    /// Staged as it is: (typed as, what it does, the change).
    Stage(String, String, E),
    /// The value being typed becomes this, as it is (a path).
    Raw(String),
    /// A name chosen: in the line, and that one from now on (an extra, a note, an artist…).
    Pin(Pinned, String, Uuid),
}

/// What taking a row leaves to the page.
pub enum Taken<A, E> {
    /// The value was filled in: Enter goes on to do the line.
    Filled,
    /// The line was replaced: nothing more this time.
    Inserted,
    Act(A),
    Stage(Staged<E>),
}

/// A page's own row taken on a schema commandline: an action (the line emptied), a
/// change staged (only its command comes out of the line), or the line replaced.
pub fn take_page<A: Clone, E: Clone>(cl: &mut CommandLine, ctx: &egui::Context, line: &schema::Line, choice: &Choice<A, E>) -> Taken<A, E> {
    match choice {
        Choice::Act(a) => {
            cl.text.clear();
            Taken::Act(a.clone())
        }
        Choice::Stage(text, label, edit) => {
            let start = line.current().map_or(cl.text.len(), |c| c.start);
            let kept = cl.text[..start].trim_end().trim_end_matches('|').trim_end().len();
            cl.text.truncate(kept);
            cl.cursor_to_end(ctx);
            Taken::Stage(Staged { text: text.clone(), label: label.clone(), edit: edit.clone(), failed: None })
        }
        Choice::Insert(text) => {
            cl.text = text.clone();
            cl.cursor_to_end(ctx);
            Taken::Inserted
        }
        Choice::Value(_) | Choice::Raw(..) | Choice::Pin(..) => Taken::Filled,
    }
}

/// The page just shown, or `/`: the cursor to the commandline (gently on arrival: not
/// from another field that has it). `last_pass`: the frame it was last shown in.
pub fn arrive(cl: &mut CommandLine, ctx: &egui::Context, shell: &mut Shell, last_pass: &mut u64) {
    // `/` is the app's key (by the character typed): it puts the cursor here.
    shell.slash_target = Some(cl.id());
    let pass = ctx.cumulative_pass_nr();
    if pass > *last_pass + 1 {
        cl.focus = true;
        cl.gently = true;
    }
    *last_pass = pass;
    if !ctx.egui_wants_keyboard_input() && !shell.palette_open && !shell.modal_was_open && ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == "/"))) {
        ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::NONE, egui::Key::Slash);
            i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
        });
        cl.focus = true;
    }
}

/// Something else gets the keys (a section, the note editor): the commandline doesn't take
/// the cursor back.
pub fn let_go(cl: &mut CommandLine, ctx: &egui::Context) {
    cl.focus = false;
    cl.place_cursor = false;
    ctx.memory_mut(|m| m.surrender_focus(cl.id()));
}

/// What's said under the commandline: the notice, what's being sent, the changes that
/// failed and those staged (chips; outside edit mode too: kept, saved in it). Returns the
/// chip whose × was clicked.
pub fn status<E>(ui: &mut egui::Ui, notice: Option<&str>, sending: usize, staged: &[Staged<E>], can_edit: bool) -> Option<usize> {
    if let Some(notice) = notice {
        ui.label(RichText::new(notice).color(P.muted).small());
    }
    if sending > 0 {
        ui.label(RichText::new(format!("Saving {sending} change(s)…")).color(P.faint).small());
    }
    for s in staged.iter().filter(|s| s.failed.is_some()) {
        ui.label(RichText::new(format!("Not saved: {}: {}", s.text, s.failed.as_deref().unwrap_or(""))).color(P.danger).small());
    }
    if staged.is_empty() {
        return None;
    }
    ui.add_space(4.0);
    let chips: Vec<cl::Chip> = staged.iter().map(|s| cl::Chip { text: &s.text, label: &s.label, warning: s.failed.as_deref() }).collect();
    let removed = cl::chips(ui, &chips, true);
    let text = if can_edit { "Ctrl+Enter saves them · Backspace on an empty commandline takes the last back" } else { "Not saved: they're saved in edit mode (e, then Ctrl+Enter)" };
    ui.label(RichText::new(text).color(P.faint).small());
    removed
}

/// Changes being saved on their own, and what the server said about each.
pub struct Sending<E> {
    sent: Arc<Mutex<Vec<(Staged<E>, Result<Option<String>, String>)>>>,
    pub count: usize,
}

impl<E> Default for Sending<E> {
    fn default() -> Self {
        Sending { sent: Arc::default(), count: 0 }
    }
}

impl<E: Clone + Send + 'static> Sending<E> {
    /// Sends one: `call` makes the request (Ok: what's worth saying, if anything).
    pub fn spawn<F>(&mut self, shell: &Shell, change: Staged<E>, call: impl FnOnce(crate::api::Api, egui::Context, E) -> F + Send + 'static)
    where
        F: std::future::Future<Output = Result<Option<String>, String>> + Send,
    {
        self.count += 1;
        let (api, ctx, sent) = (shell.api.clone(), shell.ctx.clone(), self.sent.clone());
        shell.runtime.spawn(async move {
            let result = call(api, ctx.clone(), change.edit.clone()).await;
            sent.lock().unwrap().push((change, result));
            ctx.request_repaint();
        });
    }

    /// Sends these one after another, in order (a cover added, then moved: the move
    /// finds it).
    pub fn spawn_in_order<F>(&mut self, shell: &Shell, changes: Vec<Staged<E>>, call: impl Fn(crate::api::Api, egui::Context, E) -> F + Send + 'static)
    where
        F: std::future::Future<Output = Result<Option<String>, String>> + Send,
    {
        self.count += changes.len();
        let (api, ctx, sent) = (shell.api.clone(), shell.ctx.clone(), self.sent.clone());
        shell.runtime.spawn(async move {
            for change in changes {
                let result = call(api.clone(), ctx.clone(), change.edit.clone()).await;
                sent.lock().unwrap().push((change, result));
                ctx.request_repaint();
            }
        });
    }

    /// The answers come in: one that failed is staged again (saying why). Some: something
    /// was answered, with what's worth saying.
    pub fn answers(&mut self, staged: &mut Vec<Staged<E>>) -> Option<Vec<String>> {
        let sent = std::mem::take(&mut *self.sent.lock().unwrap());
        if sent.is_empty() {
            return None;
        }
        self.count -= sent.len().min(self.count);
        let mut said = Vec::new();
        for (mut change, result) in sent {
            match result {
                Ok(Some(text)) => said.push(text),
                Ok(None) => {}
                Err(err) => {
                    change.failed = Some(err);
                    staged.push(change);
                }
            }
        }
        Some(said)
    }
}

/// A change of these sent (a source, an extra, a note). Ok: what's worth saying, if anything.
pub async fn send(api: &crate::api::Api, ctx: &egui::Context, whose: Whose, change: &Change) -> Result<Option<String>, String> {
    use reqwest::Method;
    use serde_json::{json, Value};
    let call = |method: Method, path: String, body: Option<Value>| async move { api.send_json::<Value>(method, &path, body).await.map_err(|e| e.message) };
    let link = whose.extras().link();
    match change {
        Change::AddSource(platform, data) => call(Method::POST, whose.sources_path(), Some(json!({ "platform": platform, "data": data }))).await.map(|_| None),
        Change::TrashSource(source) => call(Method::DELETE, format!("/api/sources/{source}"), None).await.map(|_| None),
        Change::UnlinkExtra(extra) => call(Method::POST, format!("/api/extras/{extra}/links"), Some(json!({ "link": link, "undo": true }))).await.map(|_| None),
        Change::HideNote(note) => call(Method::POST, format!("/api/notes/{note}/hidden"), Some(json!({ "hidden": true }))).await.map(|_| None),
        Change::Extra(c) => {
            let access = |seen: &Seen| match seen {
                Seen::Everyone => Some(json!({ "restricted": false })),
                Seen::Groups(groups) => Some(json!({ "restricted": true, "groups": groups })),
                Seen::Default => None,
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
    }
}

// ---- Files ----

/// Files chosen in the dialog, to put in the commandline as `key:path` each.
pub struct Chosen {
    key: &'static str,
    paths: Arc<Mutex<Vec<PathBuf>>>,
}

impl Chosen {
    pub fn new(key: &'static str) -> Chosen {
        Chosen { key, paths: Arc::default() }
    }

    /// Those chosen since: in the line (the cursor after them).
    pub fn put_in_line(&self, cl: &mut CommandLine) {
        let chosen = std::mem::take(&mut *self.paths.lock().unwrap());
        if chosen.is_empty() {
            return;
        }
        for path in chosen {
            cl.text = format!("{} {}:{}", cl.text.trim_end(), self.key, path.display());
        }
        cl.place_cursor = true;
        cl.focus = true;
    }
}

/// The file dialog for `key:choose`: its title, the kind of files, and whether several.
pub struct Dialog<'a> {
    pub title: &'a str,
    pub kinds: (&'a str, &'a [&'a str]),
    pub many: bool,
}

/// In `text`, the file parts at these spans (where `key:` starts, where it ends, its value;
/// in the order typed: `file:` after an extra, a cover): `paste` (if `paste`) becomes the
/// files copied in a file manager (`key:/…` each); `choose` opens the dialog and comes out
/// of the line (`chosen` puts its paths in when they're chosen: Enter again). Err: why not
/// now (no files copied; the dialog is open).
pub fn expand_file_spans(text: &mut String, shell: &Shell, spans: Vec<(usize, usize, String)>, paste: bool, dialog: Dialog, chosen: &Chosen) -> Result<(), String> {
    let mut choosing = false;
    for (start, end, value) in spans.into_iter().rev() {
        if !(value == "paste" && paste) && value != "choose" {
            continue;
        }
        let with = if value == "paste" {
            let files = crate::ui::files::clipboard_files();
            if files.is_empty() {
                return Err("There are no files in the clipboard (copy them in a file manager first).".into());
            }
            files.iter().map(|f| format!("{}:{}", chosen.key, f.display())).collect::<Vec<_>>().join(" ")
        } else {
            choosing = true;
            String::new()
        };
        text.replace_range(start..end, &with);
    }
    if choosing {
        crate::ui::files::choose(shell, dialog.title, dialog.kinds, dialog.many, &chosen.paths);
        return Err("Choose in the dialog: the paths go in the commandline, then Enter.".into());
    }
    Ok(())
}

/// A path typed: `~/` is the home folder.
pub fn path_of(typed: &str) -> PathBuf {
    match typed.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(typed),
    }
}

/// Paths starting with what's typed (a folder's entries; folders end in `/`).
pub fn paths_like(typed: &str) -> Vec<String> {
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

/// The rows for a path being typed (from `/` or `~`): its folder's entries.
pub fn path_rows<A, E>(value: &str) -> Vec<Row<A, E>> {
    if !value.starts_with('/') && !value.starts_with('~') {
        return Vec::new();
    }
    paths_like(value).into_iter().map(|p| Row { sub: if p.ends_with('/') { "a folder".into() } else { "a file".into() }, label: p.clone(), choice: Choice::Raw(p) }).collect()
}

/// `paste` and `choose`, as typed so far, with what each does.
pub fn file_words<A, E>(typed: &str, paste: &str, choose: &str) -> Vec<Row<A, E>> {
    [("paste", paste), ("choose", choose)]
        .into_iter()
        .filter(|(w, _)| w.starts_with(&typed.trim().to_lowercase()))
        .map(|(w, sub)| Row { label: w.into(), sub: sub.into(), choice: Choice::Raw(w.into()) })
        .collect()
}

// ---- Sources ----

/// A source as listed: "soundcloud someone", "custom “vk”".
pub fn source_label(s: &Source) -> String {
    let data = s.snapshots.first().map(|x| x.data.clone()).unwrap_or_default();
    match (s.platform.as_str(), crate::ui::edit::editor::model::source_name(s.external_id.as_deref(), &data).as_str()) {
        (p, "") => p.to_string(),
        ("custom", name) => format!("custom “{name}”"),
        (p, name) => format!("{p} {name}"),
    }
}

/// `source:…` (`fields`: those typed after it) or `-source:N` (`value`: its place).
pub fn source(minus: bool, value: &str, fields: &[(&str, &str)], t: &Things) -> Result<(String, Change), String> {
    t.editing.clone()?;
    if !t.may_source {
        return Err("sources need the edit_sources permission".into());
    }
    let value = value.trim();
    if minus {
        if value.is_empty() {
            return Err("-source:N puts its Nth source in the trash (the list says which is which)".into());
        }
        let n = value.parse::<usize>().ok().filter(|n| *n >= 1).ok_or_else(|| format!("{value}: a source's place is a number from 1"))?;
        let s = t.sources.get(n - 1).ok_or_else(|| format!("it has {} sources", t.sources.len()))?;
        return Ok((format!("source {n} ({}) → the trash", source_label(s)), Change::TrashSource(s.id)));
    }
    if value.is_empty() {
        return Err("source: a link, a platform and its fields, or custom and where it's from (custom vk, its page)".into());
    }
    let typed = crate::ui::edit::sources::typed(value, fields, t.whose.word(), t.shapes)?;
    Ok((format!("source: + {}", typed.shown), Change::AddSource(typed.platform, typed.data)))
}

/// The rows under `-source:` (its sources, by place) or `source:` (the platforms).
pub fn source_rows<A, E>(minus: bool, value: &str, t: &Things) -> Vec<Row<A, E>> {
    if t.editing.is_err() || !t.may_source {
        return Vec::new();
    }
    if minus {
        return t.sources.iter().enumerate().map(|(n, s)| Row { label: format!("{} {}", n + 1, source_label(s)), sub: "to the trash".into(), choice: Choice::Raw(format!("{}", n + 1)) }).collect();
    }
    crate::ui::edit::sources::platform_choices(value.trim(), "", t.whose.word(), t.shapes).into_iter().map(|c| Row { label: c.label, sub: c.sub, choice: Choice::Raw(c.text) }).collect()
}

// ---- Extras ----

/// An argument of `extra:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    Kind,
    File,
    Description,
    Source,
    Visibility,
}

/// An extra by name: one chosen in the list, else this page's, else the library's (one of
/// each name; several: chosen in the list). None: there's none (a new one).
fn extra_named(t: &Things, known: &Known, name: &str) -> Result<Option<(Uuid, bool)>, String> {
    let linked = |id: Uuid| t.extras.iter().any(|e| e.id == id && !e.hidden);
    if let Some(id) = known.pinned(Pinned::Extra, name) {
        return Ok(Some((id, linked(id))));
    }
    let own: Vec<Uuid> = t.extras.iter().filter(|e| !e.hidden && e.name.eq_ignore_ascii_case(name)).map(|e| e.id).collect();
    match own.as_slice() {
        [id] => return Ok(Some((*id, true))),
        [_, _, ..] => return Err(format!("several of its extras are called “{name}”: choose one in the list")),
        [] => {}
    }
    let library: Vec<Uuid> = t.library_extras.iter().filter(|e| !e.hidden && e.name.eq_ignore_ascii_case(name)).map(|e| e.id).collect();
    match library.as_slice() {
        [id] => Ok(Some((*id, linked(*id)))),
        [_, _, ..] => Err(format!("several extras in the library are called “{name}”: choose one in the list")),
        [] => Ok(None),
    }
}

/// Who sees an extra, as typed: everyone, the default (`new` only), or groups by name.
fn seen(value: &str, t: &Things, known: &Known, new: bool) -> Result<Seen, String> {
    if !t.may_access {
        return Err("who sees an extra: that needs the manage_access permission".into());
    }
    match value.to_lowercase().as_str() {
        "" => Err(format!("visibility: everyone, {} (the default), or groups (Group A, Group B)", t.whose.word())),
        "everyone" | "all" | "public" => Ok(Seen::Everyone),
        "artist" | "album" | "track" | "like artist" | "like the artist" | "default" if new => Ok(Seen::Default),
        "artist" | "album" | "track" | "like artist" | "like the artist" | "default" => Err("for one there already: everyone, or groups".into()),
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
pub fn extra(name: &str, minus: bool, args: &[(Arg, &str)], t: &Things, known: &Known) -> Result<(String, Change), String> {
    t.editing.clone()?;
    let name = name.trim();
    let this = t.this();
    if minus {
        let found = t.extras.iter().find(|e| !e.hidden && (known.pinned(Pinned::Extra, name) == Some(e.id) || e.name.eq_ignore_ascii_case(name)));
        let e = found.ok_or_else(|| if name.is_empty() { format!("-extra:Name: not on {this} anymore") } else { format!("it has no extra “{name}”") })?;
        return Ok((format!("“{}” not on {this} (it stays, wherever else it's linked)", e.name), Change::UnlinkExtra(e.id)));
    }
    if name.is_empty() {
        return Err(format!("extra: its name (the list has {this}'s, and the library's)"));
    }
    let found = extra_named(t, known, name)?;
    let (mut kind, mut description, mut source, mut visibility, mut files) = (None, None, None, None, Vec::new());
    let mut said = Vec::new();
    for &(arg, v) in args {
        let v = v.trim();
        match arg {
            Arg::Kind if v.is_empty() => return Err("kind: vocals, stems, sample… (any word)".into()),
            Arg::Kind => kind = Some(v.to_string()),
            Arg::File => match v {
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
            Arg::Description => description = Some(v.to_string()),
            Arg::Source => source = Some(v.to_string()),
            Arg::Visibility => visibility = Some(seen(v, t, known, found.is_none())?),
        }
    }
    if !files.is_empty() {
        said.push(crate::ui::edit::extras::files_text(files.len()));
    }
    let seen_text = |s: &Seen| match s {
        Seen::Everyone => "seen by everyone".to_string(),
        Seen::Default => t.whose.default_seen().to_string(),
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
            let own = t.extras.iter().find(|e| e.id == id);
            if !linked && (changes_about || visibility.is_some()) {
                return Err(format!("“{name}” isn't on {this}: link it first (extra:{name}), then change it; or on its own page"));
            }
            let about = match own {
                Some(e) if changes_about => Some((kind.clone().unwrap_or_else(|| e.kind.clone()), description.clone().unwrap_or_else(|| e.description.clone()), source.clone().unwrap_or_else(|| e.source.clone()))),
                _ => None,
            };
            let shown = own.map(|e| e.name.clone()).or_else(|| t.library_extras.iter().find(|e| e.id == id).map(|e| e.name.clone())).unwrap_or_else(|| name.to_string());
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
                return Err(format!("“{shown}” is on {this} already: say what changes (kind:, file:, description:, source:, visibility:)"));
            }
            (format!("extra “{shown}”: {}", what.join(" · ")), ExtraChange { id: Some(id), name: shown, linked, files, about, seen: visibility })
        }
    };
    Ok((change.0, Change::Extra(change.1)))
}

/// The rows under `extra:` (its own, then the library's, then a new one) or `-extra:`.
pub fn extra_rows<A, E>(minus: bool, value: &str, t: &Things) -> Vec<Row<A, E>> {
    if t.editing.is_err() {
        return Vec::new();
    }
    let typed = value.trim().to_lowercase();
    let this = t.this();
    if minus {
        return t
            .extras
            .iter()
            .filter(|e| !e.hidden && e.name.to_lowercase().contains(&typed))
            .map(|e| Row { label: format!("− {}", e.name), sub: format!("{} · not on {this} anymore", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) })
            .collect();
    }
    let mut rows: Vec<Row<A, E>> = t
        .extras
        .iter()
        .filter(|e| !e.hidden && e.name.to_lowercase().contains(&typed))
        .map(|e| Row { label: e.name.clone(), sub: format!("{} · on {this}: change it", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) })
        .collect();
    let linked = |id: Uuid| t.extras.iter().any(|e| e.id == id);
    rows.extend(
        t.library_extras
            .iter()
            .filter(|e| !e.hidden && !linked(e.id) && !typed.is_empty() && e.name.to_lowercase().contains(&typed))
            .take(12)
            .map(|e| Row { label: e.name.clone(), sub: format!("{} · in the library: linked here", e.kind), choice: Choice::Pin(Pinned::Extra, e.name.clone(), e.id) }),
    );
    let value = value.trim();
    if !value.is_empty() {
        rows.push(Row { label: format!("New extra “{value}”"), sub: "made when it's saved (then kind:)".into(), choice: Choice::Raw(value.to_string()) });
    }
    rows
}

/// The rows for one of `extra:`'s arguments being typed.
pub fn extra_arg_rows<A, E>(arg: Arg, value: &str, t: &Things, known: &Known) -> Vec<Row<A, E>> {
    if t.editing.is_err() {
        return Vec::new();
    }
    let typed = value.trim().to_lowercase();
    match arg {
        Arg::Kind => t.extra_kinds.iter().filter(|k| k.starts_with(&typed)).map(|k| Row { label: k.to_string(), sub: "a kind of extra here".into(), choice: Choice::Raw(k.to_string()) }).collect(),
        Arg::File => {
            let mut rows = file_words(&typed, "the files copied in a file manager", "the file dialog");
            rows.extend(path_rows(value.trim()));
            rows
        }
        Arg::Visibility if t.may_access => {
            let default = format!("{} (the default, a new one)", t.whose.word());
            let mut rows: Vec<Row<A, E>> = [("everyone", "seen by everyone".to_string()), (t.whose.word(), default)]
                .into_iter()
                .filter(|(w, _)| w.starts_with(&typed))
                .map(|(w, sub)| Row { label: w.into(), sub, choice: Choice::Raw(w.into()) })
                .collect();
            rows.extend(known.groups.iter().flatten().filter(|g| g.name != "admin" && g.name.to_lowercase().starts_with(&typed)).map(|g| Row { label: g.name.clone(), sub: "admins and this group (more: Group A, Group B)".into(), choice: Choice::Raw(g.name.clone()) }));
            rows
        }
        _ => Vec::new(),
    }
}

/// The groups, asked for once `typed` (`visibility:` is in the line).
pub fn ask_groups_when(typed: bool, t: &Things, shell: &Shell, known: &mut Known, groups: &crate::ui::edit::extras::Groups) {
    if !t.may_access || known.groups.is_some() || !typed {
        return;
    }
    crate::ui::edit::extras::ask_groups(shell, groups);
    if let Some(Ok(list)) = &*groups.lock().unwrap()
        && !list.is_empty()
    {
        known.groups = Some(list.clone());
    }
}

// ---- Notes ----

/// What `note:` does.
#[derive(Debug, Clone, PartialEq)]
pub enum NoteDoes {
    /// The note editor: for this note, or (None) a new one with this title.
    Open(Option<Uuid>, String),
    /// The text files copied in a file manager, listed to be made notes.
    Paste,
    /// Taken off (kept).
    Hide(Uuid),
}

/// A note's title as listed (no title: its first line).
pub fn note_title(n: &crate::details::Note) -> String {
    if n.title.trim().is_empty() { n.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string() } else { n.title.clone() }
}

/// `note:Title` (that note, else a new one), `note:paste`, or `-note:Title`.
pub fn note(title: &str, minus: bool, t: &Things, known: &Known) -> Result<(String, NoteDoes), String> {
    t.editing.clone()?;
    let title = title.trim();
    let found = t.notes.iter().filter(|n| !n.hidden).find(|n| known.pinned(Pinned::Note, title) == Some(n.id) || note_title(n).eq_ignore_ascii_case(title));
    if minus {
        let n = found.ok_or_else(|| if title.is_empty() { "-note:Title takes that note off (kept)".to_string() } else { format!("it has no note “{title}”") })?;
        return Ok((format!("note “{}” taken off (kept: “Taken off” brings it back)", note_title(n)), NoteDoes::Hide(n.id)));
    }
    match (title, found) {
        ("", _) => Err("note: a title (a new note), one of its notes (the list), or paste (copied text files)".into()),
        (t, None) if t.eq_ignore_ascii_case("paste") => Ok(("the text files copied: a note each (listed first)".into(), NoteDoes::Paste)),
        (_, Some(n)) => Ok((format!("the note “{}”, to edit", note_title(n)), NoteDoes::Open(Some(n.id), note_title(n)))),
        (t, None) => Ok((format!("a new note “{t}”"), NoteDoes::Open(None, t.to_string()))),
    }
}

/// The rows under `note:` (its notes, paste, a new one) or `-note:`.
pub fn note_rows<A, E>(minus: bool, value: &str, t: &Things) -> Vec<Row<A, E>> {
    if t.editing.is_err() {
        return Vec::new();
    }
    let typed = value.trim().to_lowercase();
    let mut rows: Vec<Row<A, E>> = t
        .notes
        .iter()
        .filter(|n| !n.hidden && note_title(n).to_lowercase().contains(&typed))
        .map(|n| Row { label: format!("{}{}", if minus { "− " } else { "" }, note_title(n)), sub: if minus { "taken off (kept)".into() } else { "edit it".into() }, choice: Choice::Pin(Pinned::Note, note_title(n), n.id) })
        .collect();
    if !minus {
        if "paste".starts_with(&typed) {
            rows.push(Row { label: "paste".into(), sub: "the text files copied in a file manager: a note each".into(), choice: Choice::Raw("paste".into()) });
        }
        let value = value.trim();
        if !value.is_empty() && !value.eq_ignore_ascii_case("paste") && !t.notes.iter().any(|n| !n.hidden && note_title(n).eq_ignore_ascii_case(value)) {
            rows.push(Row { label: format!("New note “{value}”"), sub: "its text next".into(), choice: Choice::Raw(value.to_string()) });
        }
    }
    rows
}

// ---- On a schema commandline ----

/// `source:`, `extra:` and `note:` as a page's commands (shared with the track editor's line).
pub use acetate_core::editor::page_key::{EXTRA, NOTE, SOURCE};

/// Whether a command is one of these (`source:`, `extra:`, `note:`).
pub fn is_thing(spec: &Spec) -> bool {
    matches!(spec.key, "source" | "extra" | "note")
}

/// A row of these lists, as a schema commandline's (a page's own: actions, stages).
pub fn to_schema<A, E>(row: Row<A, E>) -> schema::Row<Choice<A, E>> {
    let take = match row.choice {
        Choice::Value(v) => schema::Take::Value(v),
        Choice::Raw(v) => schema::Take::Raw(v),
        Choice::Pin(kind, name, id) => schema::Take::Pin { tag: kind.tag(), name, id },
        other => schema::Take::Page(other),
    };
    schema::Row { label: row.label, sub: row.sub, take }
}

/// What a source, an extra or a note command does.
#[derive(Debug, Clone, PartialEq)]
pub enum ThingDoes {
    Change(Change),
    Note(NoteDoes),
}

/// A command that is one of these: what it does (None: it isn't one).
pub fn resolve_thing(c: &Node, t: &Things, known: &Known) -> Option<Result<(String, ThingDoes), String>> {
    let change = |r: Result<(String, Change), String>| r.map(|(label, c)| (label, ThingDoes::Change(c)));
    Some(match c.spec.key {
        "source" => {
            let fields: Vec<(&str, &str)> = c.fields().map(|(n, v)| (n, v.trim())).collect();
            change(source(c.minus, &c.value, &fields, t))
        }
        "extra" => {
            let args: Vec<(Arg, &str)> = c.args.iter().filter_map(|a| Some((extra_arg(a.spec.key)?, a.value.as_str()))).collect();
            change(extra(&c.value, c.minus, &args, t, known))
        }
        "note" => note(&c.value, c.minus, t, known).map(|(label, d)| (label, ThingDoes::Note(d))),
        _ => return None,
    })
}

fn extra_arg(key: &str) -> Option<Arg> {
    Some(match key {
        "kind" => Arg::Kind,
        "file" => Arg::File,
        "description" => Arg::Description,
        "source" => Arg::Source,
        "visibility" => Arg::Visibility,
        _ => return None,
    })
}

/// The choices for a value of one of these being typed (None: `node` isn't one of theirs).
pub fn thing_choices<A, E>(node: &Node, command: &Node, t: &Things, known: &Known) -> Option<Vec<schema::Row<Choice<A, E>>>> {
    if !is_thing(command.spec) {
        return None;
    }
    let rows: Vec<Row<A, E>> = if std::ptr::eq(node, command) {
        match command.spec.key {
            "source" => source_rows(command.minus, &command.value, t),
            "extra" => extra_rows(command.minus, &command.value, t),
            _ => note_rows(command.minus, &command.value, t),
        }
    } else if node.field.is_some() {
        // A source's field: what it takes is said by its key row; any text.
        Vec::new()
    } else {
        extra_arg(node.spec.key).map(|arg| extra_arg_rows(arg, &node.value, t, known)).unwrap_or_default()
    };
    Some(rows.into_iter().map(to_schema).collect())
}

/// The fields a `source:` can be given after its platform, besides `given`.
pub fn thing_fields(node: &Node, given: &[&str], t: &Things) -> Vec<(String, String)> {
    if node.spec.key != "source" || t.editing.is_err() || !t.may_source {
        return Vec::new();
    }
    let Some(shape) = crate::ui::edit::sources::shape_of(&node.value, t.whose.word(), t.shapes) else { return Vec::new() };
    shape
        .fields
        .iter()
        .filter(|f| !given.iter().any(|g| f.name.eq_ignore_ascii_case(g)))
        .map(|f| {
            let what = match f.kind.as_str() {
                "int" => "a number",
                "strs" => "text; the field again for another",
                _ => "text",
            };
            (f.name.clone(), format!("{what}{}", if f.required { " (needed)" } else { "" }))
        })
        .collect()
}

/// The `file:` parts under `extra:` in a line, as `expand_file_spans` takes them.
pub fn file_spans(line: &schema::Line, under: &str, key: &str) -> Vec<(usize, usize, String)> {
    line.commands.iter().filter(|c| c.spec.key == under).flat_map(|c| c.args.iter().filter(|a| a.spec.key == key && a.field.is_none())).map(|a| (a.start, a.end, a.value.trim().to_string())).collect()
}
