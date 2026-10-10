//! Edit mode's door: an `Editor` is one open edit (a track, later an album's tracks), kept
//! in the core so it survives turning the phone and leaving the screen. Kotlin sends lines
//! (`run`), reads one `EditorState` (`state`) and is told by `Change::Editor` when it moved.
//! What a line means, the server's preview and applying are `acetate_core::editor`.

use std::sync::{Arc, Mutex};

use acetate_core::editor::engine::{Env, Phase};
use acetate_core::editor::panel::{self, Flag, RowKind};
use acetate_core::editor::parse::quoted;
use acetate_core::editor::resolve::Candidate;
use acetate_core::editor::session::{Chooser, Finish, Session, Undoer};
use uuid::Uuid;

use crate::{Change, Core, CoreError, Listener, Shared, parse_id};

/// A staged change.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorChip {
    pub text: String,
    pub label: String,
    pub warning: Option<String>,
}

/// A flag on a field (or on the track), as it is now and after the edit.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorFlag {
    /// The server's name of its kind.
    pub kind: String,
    /// For people ("No cover").
    pub label: String,
    /// "open" (the preview hasn't said), "fixed", "still" or "new".
    pub state: String,
    /// A line that starts fixing it, if one does.
    pub fix: Option<String>,
}

/// One field of the track as the changes leave it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorRow {
    /// "title", "credit", "add_artist", "date", "status", "genres", "album", "add_album",
    /// "related", "add_related", "comment", "cover", "add_cover", "source", "add_source",
    /// "group", "add_group", "removed" or "other".
    pub kind: String,
    /// Which of its kind (the credit's place among the credits…).
    pub index: u32,
    /// Its key in the line (`title`, `artist`, `date`…).
    pub key: String,
    pub value: String,
    /// What it was, if the changes change it ("new": it wasn't there).
    pub was: Option<String>,
    pub flags: Vec<EditorFlag>,
    /// A credit's artist.
    pub artist: Option<String>,
    /// A cover's image (its hash).
    pub image: Option<String>,
}

/// What the changes do to one track, in words.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorTrack {
    pub id: Option<String>,
    pub title: String,
    /// "✓", "✗" or "".
    pub mark: String,
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub fixed: Vec<String>,
    pub still: Vec<String>,
    pub new_flags: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorState {
    /// "editing", "applying" or "finished".
    pub phase: String,
    pub done: u32,
    pub total: u32,
    /// The (first) track's title.
    pub title: String,
    pub loading: bool,
    /// The server hasn't checked the changes as they are now.
    pub checking: bool,
    pub changing: u32,
    pub refused: u32,
    pub can_apply: bool,
    /// Why there's nothing to apply, when there's something to say.
    pub blocked: Option<String>,
    pub preview_error: Option<String>,
    /// New things the changes make.
    pub makes: Vec<String>,
    pub tracks: Vec<EditorTrack>,
    /// One track's fields (empty with more).
    pub rows: Vec<EditorRow>,
    pub chips: Vec<EditorChip>,
}

/// A choice for the line being typed.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorChoice {
    pub label: String,
    pub sub: String,
    /// The line with this choice in (then `choose` it, so the name stands for it).
    pub line: String,
    /// The same, with what comes next after it (`role:`…) added: Tab's.
    pub line_next: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorChoices {
    pub choices: Vec<EditorChoice>,
    /// Asked for in the background and not there yet.
    pub waiting: bool,
}

/// How applying went.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorFinish {
    pub changed: u32,
    pub failed: u32,
    pub errors: Vec<String>,
    /// It can be put back (`undo`).
    pub can_undo: bool,
}

/// What the session's background work calls when something arrived: only tells Kotlin
/// (which reads `state`, and that keeps the session going). It takes no lock: the engine
/// calls it while holding its own, and `state` holds the session's while waiting for them.
struct EditorWake {
    listener: Arc<dyn Listener>,
}

impl acetate_core::Wake for EditorWake {
    fn changed(&self) {
        self.listener.changed(Change::Editor);
    }
}

#[derive(uniffi::Object)]
pub struct Editor {
    shared: Arc<Shared>,
    runtime: tokio::runtime::Handle,
    session: Arc<Mutex<Session>>,
    /// The list last made, for `choose`.
    listed: Mutex<Vec<(Candidate, usize, usize)>>,
    finish: Arc<Mutex<Option<Finish>>>,
    undoer: Mutex<Option<Undoer>>,
    /// The tracks written by the edit that finished.
    written: Mutex<Vec<Uuid>>,
}

impl Editor {
    /// Keeps the session going, and keeps how applying went for `take_finish`.
    fn pump(&self, session: &mut Session) {
        session.drive();
        if let Some(finish) = session.take_finish() {
            *self.finish.lock().unwrap() = Some(finish);
        }
    }
}

fn kind_of(kind: RowKind) -> (&'static str, u32) {
    match kind {
        RowKind::Title => ("title", 0),
        RowKind::Credit(n) => ("credit", n as u32),
        RowKind::AddArtist => ("add_artist", 0),
        RowKind::Date => ("date", 0),
        RowKind::Status => ("status", 0),
        RowKind::Genres => ("genres", 0),
        RowKind::Album(n) => ("album", n as u32),
        RowKind::AddAlbum => ("add_album", 0),
        RowKind::Related(n) => ("related", n as u32),
        RowKind::AddRelated => ("add_related", 0),
        RowKind::Comment => ("comment", 0),
        RowKind::Cover(n) => ("cover", n as u32),
        RowKind::AddCover => ("add_cover", 0),
        RowKind::Source(n) => ("source", n as u32),
        RowKind::AddSource => ("add_source", 0),
        RowKind::Group(n) => ("group", n as u32),
        RowKind::AddGroup => ("add_group", 0),
        RowKind::Removed => ("removed", 0),
        RowKind::Other => ("other", 0),
    }
}

#[uniffi::export]
impl Core {
    /// Whether this account may change the library's tracks (edit mode is for them).
    /// Whether editing is offered now: the user may, and the server is there.
    pub fn can_edit(&self) -> bool {
        self.may_edit() && !self.shared.api.connection().is_offline()
    }

    /// Whether the user may edit (whether the server is there or not).
    pub fn may_edit(&self) -> bool {
        self.shared.account.lock().unwrap().can("edit_metadata")
    }

    /// Whether it may put revisions back (the Undo after an edit).
    pub fn can_restore(&self) -> bool {
        self.shared.account.lock().unwrap().can("restore_revision")
    }

    /// Opens an edit of these tracks. Waits for nothing: the tracks load in the background.
    pub fn editor(&self, tracks: Vec<String>) -> Result<Arc<Editor>, CoreError> {
        let failed = |reason: &str| CoreError::Failed { code: "editor".into(), reason: reason.into() };
        let targets: Vec<Uuid> = tracks.iter().filter_map(|t| parse_id(t)).collect();
        if targets.is_empty() {
            return Err(failed("There are no tracks to edit."));
        }
        if self.shared.api.connection().is_offline() {
            return Err(failed("Offline: editing needs the server."));
        }
        let catalog = self.shared.catalog().ok_or_else(|| failed("The library isn't loaded yet."))?;
        let (vocab, may_relate, may_source) = {
            let account = self.shared.account.lock().unwrap();
            if !account.can("edit_metadata") {
                return Err(failed("This account can't change the library."));
            }
            (account.vocab.clone().ok_or_else(|| failed("The server's lists haven't arrived yet; try again in a moment."))?, account.can("edit_relations"), account.can("edit_sources"))
        };
        let wake = Arc::new(EditorWake { listener: self.shared.listener.clone() });
        let env = Env { api: self.shared.api.clone(), runtime: self.runtime.handle().clone(), waker: wake, generation: 0 };
        let session = Arc::new(Mutex::new(Session::new(env, catalog, vocab, targets, may_relate, may_source)));
        let finish = Arc::new(Mutex::new(None));
        session.lock().unwrap().drive();
        Ok(Arc::new(Editor { shared: self.shared.clone(), runtime: self.runtime.handle().clone(), session, listed: Mutex::new(Vec::new()), finish, undoer: Mutex::new(None), written: Mutex::new(Vec::new()) }))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl Editor {
    /// Everything the screen shows.
    pub fn state(&self) -> EditorState {
        let mut session = self.session.lock().unwrap();
        // (Reading it keeps it going: details loaded, plans made, the preview asked for.)
        self.pump(&mut session);
        let report = session.report();
        let vocab = self.shared.account.lock().unwrap().vocab.clone().unwrap_or_default();
        let catalog = self.shared.catalog();
        let title = session
            .targets
            .first()
            .and_then(|t| catalog.as_ref().and_then(|c| c.track_index(*t).map(|i| c.track_at(i).title().to_string())))
            .unwrap_or_default();
        let title = if session.targets.len() > 1 { format!("{} tracks", session.targets.len()) } else { title };
        let rows = session
            .rows()
            .into_iter()
            .map(|r| {
                let (kind, index) = kind_of(r.kind);
                let title_of_track = title.clone();
                EditorRow {
                    kind: kind.into(),
                    index,
                    key: r.key.into(),
                    value: r.value,
                    was: r.was,
                    flags: r
                        .flags
                        .into_iter()
                        .map(|(k, f)| EditorFlag {
                            label: vocab.review_kind_name(&k).to_string(),
                            fix: panel::line_for(&k, &title_of_track),
                            state: match f {
                                Flag::Open => "open",
                                Flag::Fixed => "fixed",
                                Flag::Still => "still",
                                Flag::New => "new",
                            }
                            .into(),
                            kind: k,
                        })
                        .collect(),
                    artist: r.artist.map(|a| a.to_string()),
                    image: r.image,
                }
            })
            .collect();
        EditorState {
            phase: match report.phase {
                Phase::Editing => "editing",
                Phase::Applying => "applying",
                Phase::Finished => "finished",
            }
            .into(),
            done: report.done as u32,
            total: report.total as u32,
            title,
            loading: report.loading,
            checking: report.checking,
            changing: report.changing as u32,
            refused: report.refused as u32,
            can_apply: report.can_apply,
            blocked: report.blocked,
            preview_error: report.preview_error,
            makes: report.makes,
            tracks: report
                .tracks
                .into_iter()
                .map(|t| EditorTrack { id: t.track.map(|i| i.to_string()), title: t.title, mark: t.mark, lines: t.lines, warnings: t.warnings, error: t.error, fixed: t.fixed, still: t.still, new_flags: t.new })
                .collect(),
            rows,
            chips: session.chips().into_iter().map(|c| EditorChip { text: c.text, label: c.label, warning: c.warning }).collect(),
        }
    }

    /// Stages what the line says: all of it, or (something in it can't be) none of it.
    pub fn run(&self, line: String) -> Result<(), CoreError> {
        let mut session = self.session.lock().unwrap();
        session.stage(&line).map_err(|reason| CoreError::Failed { code: "line".into(), reason })?;
        self.pump(&mut session);
        Ok(())
    }

    /// Takes the staged change in this place back; its line, to type again.
    pub fn unstage(&self, place: u32) -> Option<String> {
        let mut session = self.session.lock().unwrap();
        let text = session.unstage(place as usize);
        self.pump(&mut session);
        text
    }

    /// Lists what could go where the line's last part is (found off the main thread).
    pub async fn choices(&self, line: String) -> EditorChoices {
        let chooser: Chooser = self.session.lock().unwrap().chooser();
        let text = line.clone();
        let found = self.runtime.spawn_blocking(move || chooser.choices(&text)).await.unwrap_or_default();
        let (from, to) = found.value;
        let (before, after) = (&line[..from.min(line.len())], &line[to.min(line.len())..]);
        let mut listed = Vec::new();
        let mut out = Vec::new();
        for c in &found.candidates {
            let value = if c.text == line[from.min(line.len())..to.min(line.len())] { c.text.clone() } else { quoted(&c.text) };
            let line = format!("{before}{value}{after}");
            let line_next = match &c.next {
                Some(next) => format!("{line} {next}"),
                None => line.clone(),
            };
            out.push(EditorChoice { label: c.label.clone(), sub: c.sub.clone(), line, line_next });
            listed.push((c.clone(), from, to));
        }
        for (label, text) in &found.actions {
            out.push(EditorChoice { label: label.clone(), sub: String::new(), line: text.clone(), line_next: text.clone() });
        }
        *self.listed.lock().unwrap() = listed;
        EditorChoices { choices: out, waiting: found.waiting }
    }

    /// A choice from the last list was taken: the name stands for it from now on (an
    /// artist chosen, or a new one to make).
    pub fn choose(&self, place: u32) {
        let listed = self.listed.lock().unwrap();
        if let Some((c, _, _)) = listed.get(place as usize) {
            self.session.lock().unwrap().take(c);
        }
    }

    /// An image stored on the phone, as a cover when the edit is applied (uploaded now).
    pub async fn add_cover(&self, bytes: Vec<u8>, name: String) -> Result<(), CoreError> {
        let api = self.session.lock().unwrap().env().api.clone();
        let label = name.clone();
        let sha = self
            .runtime
            .spawn(async move { acetate_core::editor::media::stage_cover(&api, bytes, &name).await })
            .await
            .map_err(|e| CoreError::Failed { code: "unexpected".into(), reason: e.to_string() })??;
        let mut session = self.session.lock().unwrap();
        session.add_staged_cover(sha, &label);
        self.pump(&mut session);
        Ok(())
    }

    /// Applies what the preview shows (once the preview is there, if it isn't yet).
    pub fn apply(&self) -> Result<(), CoreError> {
        let mut session = self.session.lock().unwrap();
        let result = session.apply().map_err(|reason| CoreError::Failed { code: "apply".into(), reason });
        self.pump(&mut session);
        result
    }

    /// How applying went, once. The library catches up (sync) and the tracks' pages are
    /// read again: the caller refreshes the catalog (`start_sync`).
    pub fn take_finish(&self) -> Option<EditorFinish> {
        let finish = self.finish.lock().unwrap().take()?;
        {
            let mut details = self.shared.track_details.lock().unwrap();
            for t in &finish.tracks {
                details.refresh(*t);
            }
        }
        let undoer = self.session.lock().unwrap().undoer();
        let can_undo = finish.failed == 0 && finish.changed > 0 && !undoer.is_empty() && self.shared.account.lock().unwrap().can("restore_revision");
        *self.undoer.lock().unwrap() = Some(undoer);
        *self.written.lock().unwrap() = finish.tracks.clone();
        Some(EditorFinish { changed: finish.changed as u32, failed: finish.failed as u32, errors: finish.errors, can_undo })
    }

    /// Puts back what the edit that finished changed (as new revisions).
    pub async fn undo(&self) -> Result<(), CoreError> {
        let undoer = self.undoer.lock().unwrap().take().ok_or_else(|| CoreError::Failed { code: "undo".into(), reason: "There's nothing to undo.".into() })?;
        let tracks = self.written.lock().unwrap().clone();
        let result = self.runtime.spawn(async move { undoer.restore(&tracks).await }).await;
        let tracks = self.written.lock().unwrap().clone();
        let mut details = self.shared.track_details.lock().unwrap();
        for t in &tracks {
            details.refresh(*t);
        }
        drop(details);
        self.shared.listener.changed(Change::Editor);
        match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(reason)) => Err(CoreError::Failed { code: "undo".into(), reason }),
            Err(e) => Err(CoreError::Failed { code: "undo".into(), reason: format!("the undo stopped: {e}") }),
        }
    }

    /// Done with the edit: whatever was staged is dropped.
    pub fn discard(&self) {
        self.session.lock().unwrap().clear();
    }

    /// Whether anything is staged (leaving would lose it).
    pub fn has_changes(&self) -> bool {
        self.session.lock().unwrap().has_changes()
    }
}
