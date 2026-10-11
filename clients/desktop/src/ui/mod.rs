//! The window. `App` holds the pages' states and a `Shell` with what every page uses:
//! the catalog, covers, playback. Pages don't change each other or start playback
//! themselves: they push `Action`s, which the app carries out after the frame. The
//! keyboard (and later the command palette) do the same, so there's one way to do each
//! thing.
//!
//! Keys, see `keys`: Ctrl+1–6 (anywhere) or 1–6 (outside text fields): albums, artists,
//! tracks, now playing, queue, review · q add to the queue · Shift+Q play next · Esc / Alt+← back · Alt+→ forward · / filter · Space pause · Ctrl+←/→
//! previous/next · Shift+←/→ seek 10 s · s star the selected one · Shift+S star what
//! plays · 9/0 volume down/up · m mute · i the selected track's page · +/− rate the selected one · r loop the track · z shuffle · F5 refresh the library · arrows or hjkl move ·
//! Enter open or play · Shift+Enter play.

pub mod covers;
pub mod files;
pub mod watch;
pub mod edit;
pub mod keymap;
pub mod messages;
pub mod modal;
pub mod selection;
pub mod share;
pub mod task;
pub mod theme;
pub mod widgets;
mod views;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Instant, SystemTime};

use eframe::egui::{self, Frame, Key, Margin, Stroke};
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::api::Api;
use crate::catalog::marks::{Kind, Mark, Marks};
use crate::catalog::model::Cover;
use crate::catalog::store::{self, Store, Update};
use crate::catalog::{Cached, Catalog};
use crate::playback::spectrum::Spectrum;
use crate::playback::{Controller, PlayerView, QueueItem};
use covers::{ease_out, Covers};
use theme::PALETTE as P;

/// A place in the app. Pages with an id show that album or artist.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Albums,
    Artists,
    Tracks,
    Album(Uuid),
    Artist(Uuid),
    NowPlaying,
    /// What plays, what's next, what played.
    Queue,
    /// The starred tracks (a playlist that's always there).
    Liked,
    Playlist(Uuid),
    /// Everything about one track.
    Track(Uuid),
    Settings,
    /// The workbench: fixing what's flagged or missing.
    Review,
    /// What's in the trash (to take back out, or purge), and old files to clean up.
    Trash,
    /// What was changed lately in the library's metadata, by anyone.
    Changes,
    /// A collective, an era, a project…: its tracks and releases.
    Grouping(Uuid),
    /// Every grouping, to browse.
    Groupings,
    /// An extra (vocals, stems, a sample…): its files, what it belongs to, who sees it.
    Extra(Uuid),
}

/// Something to do, asked for by a page, a key or a button.
pub enum Action {
    Go(Page),
    Back,
    Forward,
    /// Play these tracks (positions in the catalog), starting with `start`.
    Play { tracks: Vec<u32>, start: usize },
    /// Play this one track from this point (comparing two files).
    /// Plays `track` with `other` in step beside it (comparing: see `Controller::compare`).
    /// Opens `a` and `b` for comparing at `at`, ahead of the first play.
    Warm { a: u32, b: u32, at: std::time::Duration },
    Compare { track: u32, other: u32, at: std::time::Duration, follow: bool },
    /// Queue these tracks by hand: at the end of the queue, or (`next`) at its start.
    Enqueue { tracks: Vec<u32>, next: bool },
    /// Play this track of the queue or the list now.
    PlayPlace(crate::playback::queue::Place),
    /// Take it out of the queue or the list.
    RemovePlace(crate::playback::queue::Place),
    /// Move a queued track to another place in the queue.
    MoveQueued(usize, usize),
    ClearQueued,
    TogglePause,
    Next,
    Previous,
    /// Seek by this many seconds (negative: back).
    SeekBy(f32),
    /// Seek to this point of the current track.
    SeekTo(std::time::Duration),
    /// Change the volume slider by this much (0 to 1 is all of it).
    VolumeBy(f32),
    SetVolume(f32),
    ToggleMute,
    /// Play through this sound output (None: the system's default).
    SetOutput(Option<String>),
    /// Stream copies at this quality (kbps); None: as the server decides.
    SetStreamQuality(Option<u32>),
    /// The equalizer: on or off, and each band's gain (dB).
    SetEqualizer(bool, [f32; 10]),
    /// Loop the current track (or stop looping it).
    ToggleRepeat,
    ToggleShuffle,
    /// Star it, or unstar it if it's starred.
    ToggleStar(Kind, Uuid),
    /// Set its rating (0 to 10, half stars).
    Rate(Kind, Uuid, i16),
    /// Change its rating by this many half stars.
    RateBy(Kind, Uuid, i16),
    /// Choose a playlist to add these tracks to (or make one).
    AddToPlaylist(Vec<Uuid>),
    /// Choose which of your playlists to take these tracks out of.
    RemoveFromPlaylist(Vec<Uuid>),
    NewPlaylist,
    RenamePlaylist(Uuid),
    RefreshLibrary,
    /// Switch edit mode on or off (pages become editable in place).
    ToggleEditMode,
    /// Show every key (the `?` overlay).
    ShowKeys,
    ShowMessages,
    /// Copy a link to this track, album or artist (to send to someone with an account).
    CopyLink(crate::links::Target),
    /// Copy a link to the page shown (if it's a track's, album's or artist's).
    CopyPageLink,
    /// Make a share link (a guest can open it, without an account) to this track or
    /// album, and copy it. Someone logged in who opens it gets the app, as for a link.
    CopyShareLink(crate::links::Target),
    /// Like `CopyShareLink`, for the page shown.
    CopyPageShareLink,
    /// Choose an artist, album or track for the form that asked (the palette; the
    /// choice arrives through `Shell::picker`).
    Pick { kind: edit::PickKind, except: Option<Uuid> },
    LoggedIn,
    LogOut,
}

/// What every page uses.
pub struct Shell {
    /// For waking the UI from background work.
    pub ctx: egui::Context,
    /// Tells the window background work finished (a repaint), for what the core runs.
    pub waker: acetate_core::Waker,
    /// The logo, as a texture.
    pub logo: egui::TextureHandle,
    /// Volume and sound output, kept between runs.
    pub settings: crate::settings::Settings,
    /// Who's logged in, what they may do, and the server's fixed lists.
    pub account: crate::account::Account,
    /// The version warning already shown (a toast), so it isn't repeated at every refresh.
    warned_version: Option<String>,
    /// Whether the server could be reached, as of the last frame (see `api::Connection`).
    pub connection: acetate_core::api::Connection,
    /// Edit mode was on when the server went away: it comes back on with the server.
    edit_mode_when_back: bool,
    /// Pages show their editors (only with `edit_metadata`).
    pub edit_mode: bool,
    /// Open review flags (the Review page's list).
    pub flags: crate::review::Flags,
    /// Where picks from the palette arrive, for the form that asked.
    pub picker: edit::Picker,
    /// Track editors, per track: changes staged and not applied are kept while you look
    /// elsewhere.
    pub track_editors: std::collections::HashMap<Uuid, edit::editor::TrackEditor>,
    /// Album forms, per album, likewise.
    pub album_forms: std::collections::HashMap<Uuid, edit::album_form::AlbumForm>,
    /// Artist forms, per artist, likewise.
    pub artist_forms: std::collections::HashMap<Uuid, edit::artist_form::ArtistForm>,
    /// The form shown has changes not saved (set by the form each frame): Esc doesn't
    /// leave the page then, it asks to discard them.
    pub form_dirty: bool,
    /// The page uses Space itself (a merge view: Space plays the left file), so it
    /// doesn't pause. Set by the page each frame.
    pub page_takes_space: bool,
    /// The page uses Esc and Backspace itself (an artist's section shown alone: they go
    /// back to the whole page), so they don't leave it. Set by the page each frame.
    pub page_takes_back: bool,
    /// The page's own keys are on (the track editor's rows): the app's one-key shortcuts
    /// (e, r, m…) wait.
    pub page_takes_keys: bool,
    /// Where `/` puts the cursor: the page's commandline (else the list's filter). Set by
    /// the page each frame.
    pub slash_target: Option<egui::Id>,
    /// A popup is open (see `modal`): the app's keys wait. Set by the page each frame.
    pub modal_open: bool,
    /// What Ctrl+L links to on a list page (not a track's, album's or artist's): the
    /// selected track or album, as the list said last frame.
    pub list_target: Option<crate::links::Target>,
    /// A popup was open last frame. A page's forms are drawn before the popup (a track
    /// list's, below them), so they leave Ctrl+Enter and Esc to it by this.
    pub modal_was_open: bool,
    /// The palette is open (it takes Esc, before the page's form could).
    pub palette_open: bool,
    /// The toasts so far, to read again.
    pub messages: messages::Messages,
    pub runtime: Runtime,
    pub api: Api,
    pub controller: Arc<Controller>,
    pub covers: Covers,
    /// A cover's original, shown on its own in a popup (a click on a big cover).
    pub original: Option<covers::Original>,
    /// A video being watched (over everything).
    pub watching: Option<watch::Watch>,
    pub catalog: Cached<Catalog>,
    /// Counts catalog replacements: a page's lists are rebuilt when it changes.
    pub generation: u64,
    /// The catalog was replaced: freed memory is given back next frame (`memory::trim`).
    pub trim_asked: bool,
    pub actions: Vec<Action>,
    /// The root: where everything's kept (see `root`).
    pub data_dir: PathBuf,
    /// Where copies of things are kept (scaled covers).
    pub cache_dir: PathBuf,
    /// The mirror of the server's files (admins), kept up to date while the app is open.
    pub mirror: crate::mirror::in_app::InApp,
    /// The app holds its lock (see `root::hold_app_lock`): the mirror service has been
    /// told to step aside.
    pub app_lock: Arc<std::sync::atomic::AtomicBool>,
    /// Music kept on this machine to play without the server (set at login).
    pub offline: Option<Arc<acetate_core::offline::Offline>>,
    /// When what the downloads want was last worked out.
    downloads_checked: Instant,
    /// Stars, ratings and plays as shown (the catalog's, with this session's changes).
    pub marks: Marks,
    /// The queue kept between runs (set at login), and whether it was brought back yet.
    resume: Option<crate::resume::Resume>,
    resumed: bool,
    /// What each changing cover place shows (see `cover_crossfade`).
    cover_slots: std::collections::HashMap<&'static str, CoverSlot>,
    pub playlists: crate::playlists::Playlists,
    /// Everything about one track, album or artist, for their pages.
    pub track_details: crate::details::Details<crate::details::TrackDetail>,
    pub album_details: crate::details::Details<crate::details::AlbumDetail>,
    pub artist_details: crate::details::Details<crate::details::ArtistDetail>,
    pub grouping_details: crate::details::Details<crate::details::GroupingDetail>,
    updates: (mpsc::Sender<Update>, mpsc::Receiver<Update>),
    store: Option<Arc<Store>>,
    /// When the catalog being loaded was asked for.
    refresh_asked: Option<Instant>,
    /// A refresh was asked for while one was running (after an edit): it runs again right
    /// after, so the edit isn't missed until the next one.
    refresh_again: bool,
    /// The server's answers to mark changes: (id, the mark it has, or an error with the
    /// mark to go back to).
    /// Stars, ratings, plays and playlist additions on their way to the server.
    pub outbox: Option<Arc<acetate_core::outbox::Outbox>>,
}

impl Shell {
    /// What the track editor's engine works with.
    pub fn env(&self) -> edit::editor::engine::Env {
        edit::editor::engine::Env { api: self.api.clone(), runtime: self.runtime.handle().clone(), waker: self.waker.clone(), generation: self.generation }
    }

    pub fn act(&mut self, action: Action) {
        self.actions.push(action);
    }

    /// For something only the server can do (`what`: "editing", "share links"…): whether
    /// it can be done now. Offline it can't, and the user is told so.
    pub fn needs_server(&mut self, what: &str) -> bool {
        if !self.connection.is_offline() {
            return true;
        }
        self.playlists.notice = Some((format!("Offline: {what} needs the server."), Instant::now()));
        false
    }

    /// Whether editing is offered: the user may, and the server is there.
    pub fn can_edit(&self) -> bool {
        self.account.can("edit_metadata") && !self.connection.is_offline()
    }

    /// The user's star, rating and play count on a track, album or artist.
    pub fn mark(&self, id: Uuid) -> Mark {
        self.marks.get(self.catalog.value.as_deref(), id)
    }

    /// Shows `mark` at once and sends the change to the server (through the outbox: now,
    /// or when it's back).
    fn change_mark(&mut self, kind: Kind, id: Uuid, mark: Mark) {
        let before = self.mark(id);
        let Some(outbox) = self.outbox.clone() else { return };
        if mark == before {
            return;
        }
        self.marks.set(id, mark);
        let catalog = self.catalog.value.as_deref();
        let name = match kind {
            Kind::Track => catalog.and_then(|c| Some(c.track(id)?.title().to_string())),
            Kind::Album => catalog.and_then(|c| Some(c.album(id)?.title().to_string())),
            Kind::Artist => catalog.and_then(|c| Some(c.artist(id)?.name.clone())),
        }
        .unwrap_or_default();
        let kind = kind.path().to_string();
        outbox.push(if mark.starred != before.starred {
            acetate_core::outbox::Change::Star { kind, id, name, starred: mark.starred, from: before.starred }
        } else {
            acetate_core::outbox::Change::Rate { kind, id, name, rating: mark.rating, from: before.rating }
        });
    }

    /// Whether something is kept on this machine (`kind`: "track", "album", "playlist" or
    /// "liked", the last with an empty id).
    pub fn is_pinned(&self, kind: &str, id: &str) -> bool {
        self.offline.as_ref().is_some_and(|o| o.is_pinned(kind, id))
    }

    /// Keeps something on this machine (or stops), and fetches what that makes missing.
    pub fn pin(&mut self, kind: &str, id: &str, name: &str, on: bool) {
        let Some(offline) = &self.offline else { return };
        offline.pin(acetate_core::offline::Pin { kind: kind.into(), id: id.into(), name: name.into() }, on);
        self.refresh_downloads();
    }

    /// Works out which files the pins want now and tells the downloads (a playlist got a
    /// track, a star was added…).
    pub fn refresh_downloads(&mut self) {
        self.downloads_checked = Instant::now();
        let (Some(offline), Some(catalog)) = (self.offline.clone(), self.catalog.value.clone()) else { return };
        let pins = offline.pins();
        if pins.is_empty() && offline.status().files == 0 {
            return;
        }
        let playlists = &mut self.playlists;
        let liked = self.marks.liked(&catalog);
        let (wants, complete) = acetate_core::offline::wanted(
            &pins,
            &catalog,
            &mut |id| playlists.detail(id).value.as_ref().map(|d| d.entries.iter().map(|e| e.track_id).collect()),
            &|| liked.clone(),
        );
        offline.set_wanted(wants, complete);
    }

    /// Takes in what came of the changes in the outbox.
    fn take_sent(&mut self) {
        use acetate_core::outbox::Sent;
        let Some(outbox) = self.outbox.clone() else { return };
        for sent in outbox.take() {
            match sent {
                Sent::Played(track) => {
                    let mark = self.mark(track);
                    self.marks.set(track, Mark { play_count: mark.play_count + 1, ..mark });
                }
                Sent::Marked(id) => self.marks.touch(id),
                Sent::Unstar { id, starred } => {
                    let mark = self.mark(id);
                    self.marks.set(id, Mark { starred, ..mark });
                }
                Sent::Unrate { id, rating } => {
                    let mark = self.mark(id);
                    self.marks.set(id, Mark { rating, ..mark });
                }
                Sent::Playlist(id) => self.playlists.fetch(id),
                Sent::Notice(text) => self.playlists.notice = Some((text, Instant::now())),
            }
        }
    }

    /// A cover, with a placeholder (a small disc on a color made from `seed`) until it
    /// has loaded, fading in. `lift`: 0 to 1, how raised it looks (its shadow).
    pub fn cover(&mut self, ui: &egui::Ui, cover: Option<Cover>, seed: Uuid, rect: egui::Rect, radius: u8, lift: f32) {
        let painter = ui.painter();
        painter.add(
            egui::Shadow {
                offset: [0, (4.0 + lift * 10.0) as i8],
                blur: (10.0 + lift * 22.0) as u8,
                spread: 0,
                color: egui::Color32::from_black_alpha((90.0 + lift * 70.0) as u8),
            }
            .as_shape(rect, radius),
        );
        let size = covers::CoverSize::for_rect(ui.ctx(), rect);
        let loaded = cover.and_then(|c| self.covers.get(ui.ctx(), c, size));
        let fade = loaded.as_ref().map(|(_, f)| *f).unwrap_or(0.0);
        if fade < 1.0 {
            widgets::placeholder(ui.painter(), rect, radius, seed);
        }
        if let Some((texture, fade)) = loaded {
            egui::Image::from_texture(&texture)
                .corner_radius(radius)
                .tint(egui::Color32::from_white_alpha((fade * 255.0) as u8))
                .paint_at(ui, rect);
        }
    }

    /// A larger copy of a row's cover, grown out of its thumbnail `thumb` below it (above, if
    /// there's no room) while the pointer rests on it, and shrunk back into it after.
    /// Call every frame for each row's cover.
    pub fn cover_preview(&mut self, ui: &egui::Ui, cover: Option<Cover>, seed: Uuid, thumb: egui::Rect) {
        const SIZE: f32 = 240.0;
        const DWELL: f64 = 0.3;
        const GRACE: f64 = 0.25;
        let ctx = ui.ctx();
        let id = egui::Id::new("cover-preview");
        let now = ctx.input(|i| i.time);
        let over = ctx.input(|i| i.pointer.hover_pos().is_some_and(|p| thumb.contains(p) && ui.clip_rect().contains(p) && !i.pointer.any_down()));
        let mut open = false;
        if over && let Some(cover) = cover.filter(|_| !self.modal_open) {
            let (at, since) = ctx.data(|d| d.get_temp::<(Uuid, f64)>(id)).unwrap_or((seed, now));
            let since = if at == seed { since } else { now };
            ctx.data_mut(|d| d.insert_temp(id, (seed, since)));
            // Both copies are asked for from the first moment, so they're there when it opens.
            let size = covers::CoverSize::for_rect(ctx, egui::Rect::from_min_size(thumb.min, egui::Vec2::splat(SIZE)));
            let loaded = self.covers.get(ctx, cover, size).is_some() || self.covers.get(ctx, cover, covers::CoverSize::Small).is_some();
            // It opens once the cover is there; if it's slow, the placeholder shows after a grace
            // period (not a flash of it before the image).
            open = now - since >= DWELL && (loaded || now - since >= DWELL + GRACE);
            if now - since < DWELL + GRACE {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        } else if ctx.data(|d| d.get_temp::<(Uuid, f64)>(id)).is_some_and(|(at, _)| at == seed) {
            ctx.data_mut(|d| d.remove::<(Uuid, f64)>(id));
        }
        let t = covers::ease_out(ctx.animate_bool_with_time(id.with(seed), open, 0.16));
        let Some(cover) = cover else { return };
        if t <= 0.0 {
            return;
        }
        let screen = ctx.content_rect();
        let left = (thumb.center().x - SIZE / 2.0).clamp(screen.left() + 8.0, (screen.right() - SIZE - 8.0).max(screen.left() + 8.0));
        let top = if thumb.bottom() + 8.0 + SIZE <= screen.bottom() - 8.0 { thumb.bottom() + 8.0 } else { thumb.top() - 8.0 - SIZE };
        let full = egui::Rect::from_min_size(egui::pos2(left, top), egui::Vec2::splat(SIZE));
        let rect = egui::Rect::from_min_max(thumb.min.lerp(full.min, t), thumb.max.lerp(full.max, t));
        let (radius, size) = (4.0 + 6.0 * t, covers::CoverSize::for_rect(ctx, full));
        let big = self.covers.get(ctx, cover, size);
        let small = self.covers.get(ctx, cover, covers::CoverSize::Small);
        egui::Area::new(id.with("area"))
            .order(egui::Order::Tooltip)
            .fixed_pos(screen.min)
            .constrain(false)
            .interactable(false)
            .show(ctx, |ui| {
                ui.painter().add(
                    egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: egui::Color32::from_black_alpha((150.0 * t) as u8) }
                        .as_shape(rect, radius as u8),
                );
                // The images are drawn at full strength all the way (the preview's own growing
                // and shrinking is its size; fading them would show what's under them). The
                // placeholder only when there's no image at all.
                if small.is_none() && big.is_none() {
                    widgets::placeholder(ui.painter(), rect, radius as u8, seed);
                }
                // The small copy is there at once; the large one fades in over it.
                for (texture, fade) in [small, big].into_iter().flatten() {
                    egui::Image::from_texture(&texture)
                        .corner_radius(radius as u8)
                        .tint(egui::Color32::from_white_alpha((fade * 255.0) as u8))
                        .paint_at(ui, rect);
                }
            });
    }

    /// A big cover: like `cover`, and a click shows its original (see `open_original`).
    #[allow(clippy::too_many_arguments)]
    pub fn cover_with_original(&mut self, ui: &egui::Ui, cover: Option<Cover>, seed: Uuid, rect: egui::Rect, radius: u8, lift: f32) {
        self.cover(ui, cover, seed, rect, radius, lift);
        self.original_on_click(ui, cover, rect);
    }

    /// A click on `rect` (where `cover` is drawn) shows its original.
    pub fn original_on_click(&mut self, ui: &egui::Ui, cover: Option<Cover>, rect: egui::Rect) {
        let Some(cover) = cover else { return };
        let response = ui.interact(rect, ui.id().with(("original", cover.key.0)), egui::Sense::click());
        if response.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text("Click: the original").clicked() {
            self.open_original(ui.ctx(), cover);
        }
    }

    /// Shows a cover's original (unchanged, full size) in a popup. It's fetched now, and
    /// let go when the popup closes.
    pub fn open_original(&mut self, ctx: &egui::Context, cover: Cover) {
        self.original = Some(covers::Original::open(&self.api, self.runtime.handle(), ctx, cover.key, self.covers.here()));
    }

    /// The original's popup, while one is open: as big as fits (never bigger than it is),
    /// with its size in pixels and bytes.
    /// Watches an extra's video file: over everything, the music paused.
    pub fn watch(&mut self, file: Uuid, title: &str) {
        let view = self.controller.view.lock().unwrap().clone();
        if view.playing && !view.paused {
            self.controller.set_paused(true);
        }
        let volume = if view.muted { 0.0 } else { (view.volume as f64 * 100.0).clamp(0.0, 100.0) };
        self.watching = Some(watch::Watch::open(&self.ctx, &self.api, file, title, volume));
    }

    /// The video being watched, while there's one.
    fn show_watch(&mut self, ctx: &egui::Context) {
        let Some(watching) = self.watching.as_mut() else { return };
        self.modal_open = true;
        if watching.show(ctx) {
            self.watching = None;
        }
    }

    fn show_original(&mut self, ctx: &egui::Context) {
        let Some(original) = self.original.as_mut() else { return };
        self.modal_open = true;
        original.poll(ctx);
        let title = match &original.loaded {
            Some(Ok((_, [w, h], bytes))) => format!("Original · {w}×{h} · {:.1} MB", *bytes as f64 / 1_000_000.0),
            _ => "Original".to_string(),
        };
        let screen = ctx.content_rect().size();
        let closed = modal::show(ctx, "original-cover", &title, screen - egui::Vec2::splat(48.0), |ui| match &original.loaded {
            None => {
                ui.label(egui::RichText::new("Loading the original…").color(P.muted));
            }
            Some(Err(err)) => {
                ui.label(egui::RichText::new(format!("Couldn't load it: {err}")).color(P.danger));
            }
            Some(Ok((texture, [w, h], _))) => {
                let room = ui.available_size();
                // Pixels on the screen: 1:1 at most (bigger would only blur it).
                let ppp = ui.ctx().pixels_per_point();
                let scale = (room.x / *w as f32).min(room.y / *h as f32).min(1.0 / ppp);
                let size = egui::vec2(*w as f32, *h as f32) * scale;
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::from_texture(texture).fit_to_exact_size(size));
                });
            }
        });
        if closed {
            self.original = None;
        }
    }

    /// Keeps the queue for next time, and brings back last time's once the library is
    /// here (paused, where it was; tracks no longer in the library are left out).
    fn resume_queue(&mut self, player: &PlayerView) {
        let Some(resume) = self.resume.as_mut() else { return };
        if self.resumed {
            resume.save(player, self.runtime.handle());
            return;
        }
        let Some(catalog) = self.catalog.value.clone() else { return };
        self.resumed = true;
        if player.current.is_some() {
            return;
        }
        let Some((list, saved, position)) = resume.load() else { return };
        let current = saved.current_queued.or_else(|| list.tracks.get(saved.index).copied());
        let at = position
            .filter(|p| Some(p.track) == current)
            .map(|p| std::time::Duration::from_millis(p.position_ms))
            .unwrap_or_default();
        let controller = self.controller.clone();
        // Built in the background: it can be the whole library.
        self.runtime.spawn(async move {
            let items = |ids: &[Uuid]| -> Vec<QueueItem> {
                ids.iter().filter_map(|id| catalog.track_index(*id)).map(|i| queue_item(&catalog, i)).collect()
            };
            let list_track = list.tracks.get(saved.index).copied();
            let list = items(&list.tracks);
            let index = list_track.and_then(|id| list.iter().position(|i| i.track_id == id)).unwrap_or(0);
            let current_queued = saved.current_queued.and_then(|id| catalog.track_index(id)).map(|i| queue_item(&catalog, i));
            let queued = items(&saved.queued);
            controller.restore(list, index, queued, current_queued, at, saved.repeat, saved.shuffle).await
        });
    }

    /// Like `cover`, for a place whose cover changes (what plays): the old one fades out
    /// as the new one fades in, and stays until the new one has loaded. `slot` names the
    /// place ("player-bar"), each remembering its own.
    #[allow(clippy::too_many_arguments)]
    pub fn cover_crossfade(&mut self, ui: &egui::Ui, slot: &'static str, cover: Option<Cover>, seed: Uuid, rect: egui::Rect, radius: u8, lift: f32) {
        let now = ui.input(|i| i.time);
        let state = self.cover_slots.entry(slot).or_insert(CoverSlot { shown: cover, seed, previous: None, since: now });
        if state.shown.map(|c| c.key) != cover.map(|c| c.key) {
            // The old one fades out from the moment the new one is there.
            state.previous = state.shown.map(|c| (c, state.seed));
            state.shown = cover;
            state.seed = seed;
            state.since = f64::INFINITY;
        }
        let state = *state;
        let size = covers::CoverSize::for_rect(ui.ctx(), rect);
        let new = cover.and_then(|c| self.covers.get(ui.ctx(), c, size));
        let old = state.previous.and_then(|(c, _)| self.covers.get(ui.ctx(), c, size));
        // No cover (or one that couldn't be loaded) is there at once: the placeholder.
        let none = cover.is_none_or(|c| self.covers.failed(c.key, size));
        let ready = new.is_some() || none;
        if ready && state.since.is_infinite() {
            self.cover_slots.get_mut(slot).unwrap().since = now;
        }
        let t = if ready { ease_out((((now - state.since.min(now)) as f32) / 0.4).clamp(0.0, 1.0)) } else { 0.0 };
        if t < 1.0 {
            ui.ctx().request_repaint();
        }
        let painter = ui.painter();
        painter.add(
            egui::Shadow {
                offset: [0, (4.0 + lift * 10.0) as i8],
                blur: (10.0 + lift * 22.0) as u8,
                spread: 0,
                color: egui::Color32::from_black_alpha((90.0 + lift * 70.0) as u8),
            }
            .as_shape(rect, radius),
        );
        let image = |texture: &egui::TextureHandle, alpha: f32| {
            egui::Image::from_texture(texture)
                .corner_radius(radius)
                .tint(egui::Color32::from_white_alpha((alpha * 255.0) as u8))
                .paint_at(ui, rect);
        };
        match (new, none) {
            // The new cover fades in over the old one (or the placeholder).
            (Some((texture, _)), _) => {
                match &old {
                    Some((old, _)) if t < 1.0 => image(old, 1.0),
                    _ if t < 1.0 => widgets::placeholder(ui.painter(), rect, radius, seed),
                    _ => {}
                }
                image(&texture, t);
            }
            // No cover: the old one fades out, showing the placeholder under it.
            (None, true) => {
                widgets::placeholder(ui.painter(), rect, radius, seed);
                if let Some((old, _)) = &old {
                    image(old, 1.0 - t);
                }
            }
            // Still loading: the old one stays until the new one is there.
            (None, false) => match &old {
                Some((old, _)) => image(old, 1.0),
                None => widgets::placeholder(ui.painter(), rect, radius, seed),
            },
        }
    }

    /// Starts loading the library: the copy on disk first, then the server's.
    fn load_library(&mut self, ctx: &egui::Context) {
        // A session from before servers had an identity learns it first (what's kept moves
        // under it); the server being away only delays that to the next time.
        if self.api.session().is_some_and(|s| s.instance.is_none()) {
            let api = self.api.clone();
            let _ = self.runtime.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(3), api.learn_instance()).await });
        }
        let Some(session) = self.api.session() else { return };
        let store = Arc::new(Store::new(&self.data_dir, &session));
        self.store = Some(store.clone());
        // Changes on their way to the server (some kept from when it was away).
        let outbox = acetate_core::outbox::Outbox::open(self.data_dir.join(format!("outbox-{}.json", store::key(&session))), self.api.clone(), self.runtime.handle().clone(), self.waker.clone());
        self.controller.send_listens_to(Some(outbox.clone()));
        self.playlists.reset(Some(self.data_dir.join(format!("playlists-{}", store::key(&session)))), Some(outbox.clone()));
        self.outbox = Some(outbox);
        self.resume = Some(crate::resume::Resume::new(&self.data_dir, &store::key(&session)));
        self.resumed = false;
        // What's downloaded plays from the disk.
        match acetate_core::offline::Offline::open(&self.data_dir.join(format!("offline-{}", store::key(&session))), self.api.clone(), self.runtime.handle().clone(), self.waker.clone()) {
            Ok(offline) => {
                offline.set_quality(self.settings.stream_kbps);
                // Downloads, and what the mirror (admins) keeps: blobs by their hash, under the root.
                let finder = offline.clone();
                let mirror_blobs = self.data_dir.join("blobs");
                self.controller.set_local(Arc::new(move |sha| {
                    finder.local_path(sha).or_else(|| {
                        let path = mirror_blobs.join(sha.get(0..2)?).join(sha.get(2..4)?).join(sha);
                        (acetate_core::blobs::is_sha256(sha) && path.is_file()).then_some(path)
                    })
                }));
                self.covers.set_here(covers::Here { downloads: Some(offline.clone()), mirror: Some(self.data_dir.clone()) });
                self.offline = Some(offline);
            }
            Err(err) => eprintln!("couldn't open the downloads: {err:#}"),
        }
        let details = self.data_dir.join(format!("details-{}", store::key(&session)));
        self.track_details.reset(Some(details.clone()));
        self.album_details.reset(Some(details.clone()));
        self.artist_details.reset(Some(details.clone()));
        self.grouping_details.reset(Some(details));
        self.catalog = Cached { refreshing: true, ..Cached::default() };
        self.refresh_asked = Some(Instant::now());
        // What the account may do, as kept (it may be offline), then as the server says.
        self.account = crate::account::Account::open(Some(self.data_dir.join(format!("me-{}.json", store::key(&session)))));
        self.account.fetch(&self.api, self.runtime.handle(), &self.waker);
        self.edit_mode = false;
        self.track_editors.clear();
        self.album_forms.clear();
        self.artist_forms.clear();
        self.flags.reset(Some(self.data_dir.join(format!("review-{}.json", store::key(&session)))));
        let (sender, api, ctx) = (self.updates.0.clone(), self.api.clone(), ctx.clone());
        self.runtime.spawn(store::load_and_refresh(store, api, move |update| {
            let _ = sender.send(update);
            ctx.request_repaint();
        }));
    }

    /// Notices the server going away or coming back. Away: only what's on this computer
    /// plays (worked out in the background). Back: everything plays again, the library is
    /// asked what changed, the account what it may do, and the mirror tries again.
    fn follow_connection(&mut self) {
        let now = self.api.connection();
        let (was, is) = (self.connection.is_offline(), now.is_offline());
        self.connection = now;
        if is && !was {
            // Editing needs the server: edit mode goes off (what's unsaved in open forms
            // stays), and comes back on with the server.
            if self.edit_mode {
                self.edit_mode = false;
                self.edit_mode_when_back = true;
                self.playlists.notice = Some(("Offline: edit mode is off until the server is back (nothing unsaved is lost).".into(), Instant::now()));
            }
            let kept = self.offline.as_ref().map(|o| o.sources()).unwrap_or_default();
            let blobs = self.data_dir.join("blobs");
            let (controller, api) = (self.controller.clone(), self.api.clone());
            self.runtime.spawn(async move {
                let here = tokio::task::spawn_blocking(move || acetate_core::playback::on_disk::gather(kept, Some(&blobs))).await.unwrap_or_default();
                if api.connection().is_offline() {
                    controller.on_disk.set(Some(here));
                    controller.playable_changed().await;
                }
            });
        }
        if was && !is {
            if std::mem::take(&mut self.edit_mode_when_back) && self.account.can("edit_metadata") {
                self.edit_mode = true;
                self.playlists.notice = Some(("Back online: edit mode is on again.".into(), Instant::now()));
            }
            // What waited goes first, so the library asked for next has it.
            if let Some(outbox) = &self.outbox {
                outbox.kick();
            }
            self.controller.on_disk.set(None);
            self.covers.forget_failed();
            let controller = self.controller.clone();
            self.runtime.spawn(async move { controller.playable_changed().await });
            self.act(Action::RefreshLibrary);
            self.account.fetch(&self.api, self.runtime.handle(), &self.waker);
            self.mirror.server_back();
        }
    }

    fn refresh_library(&mut self, ctx: &egui::Context) {
        let Some(store) = self.store.clone() else { return };
        if self.catalog.refreshing {
            self.refresh_again = true;
            return;
        }
        self.catalog.refreshing = true;
        self.refresh_asked = Some(Instant::now());
        let current = self.catalog.value.clone();
        let (sender, api, ctx) = (self.updates.0.clone(), self.api.clone(), ctx.clone());
        self.runtime.spawn(async move {
            store::refresh(&store, &api, current, &move |update| {
                let _ = sender.send(update);
                ctx.request_repaint();
            })
            .await;
        });
    }

    /// Takes in what the library loading reported (and starts the refresh asked for
    /// meanwhile, if one was).
    fn take_updates(&mut self) {
        self.take_reports();
        if self.refresh_again && !self.catalog.refreshing {
            self.refresh_again = false;
            let ctx = self.ctx.clone();
            self.refresh_library(&ctx);
        }
    }

    fn take_reports(&mut self) {
        while let Ok(update) = self.updates.1.try_recv() {
            match update {
                Update::Loaded { catalog, from_server } => {
                    if from_server {
                        // Every playlist kept on disk, as the server has it (for offline).
                        self.playlists.keep_all_fresh(&catalog);
                    }
                    self.catalog.value = Some(catalog);
                    self.downloads_checked = Instant::now().checked_sub(std::time::Duration::from_secs(3600)).unwrap_or_else(Instant::now);
                    // The old one's memory, back to the system (it held about as much),
                    // once this frame's copies of it are gone too.
                    self.trim_asked = true;
                    self.generation += 1;
                    if from_server {
                        if let Some(asked) = self.refresh_asked {
                            let waiting = self.outbox.as_ref().map(|o| o.waiting_marks()).unwrap_or_default();
                            self.marks.catalog_arrived(asked, &waiting);
                            self.playlists.catalog_arrived(asked);
                        }
                        self.catalog.refreshing = false;
                        self.catalog.confirmed_at = Some(SystemTime::now());
                        self.catalog.error = None;
                    }
                }
                Update::Unchanged => {
                    self.catalog.refreshing = false;
                    self.catalog.confirmed_at = Some(SystemTime::now());
                    self.catalog.error = None;
                }
                Update::Failed(err) => {
                    self.catalog.refreshing = false;
                    self.catalog.error = Some(err);
                }
            }
        }
    }

    /// A line about the library for the sidebar.
    pub fn library_status(&self) -> String {
        let count = self.catalog.value.as_ref().map(|c| format!("{} tracks", thousands(c.track_count())));
        let state = if self.catalog.refreshing {
            "checking for changes…".to_string()
        } else if let Some(err) = &self.catalog.error {
            format!("the copy kept here (couldn't check for changes: {err})")
        } else if self.catalog.confirmed_at.is_some() {
            "up to date".to_string()
        } else {
            String::new()
        };
        match count {
            Some(count) if !state.is_empty() => format!("{count} · {state}"),
            Some(count) => count,
            None if self.catalog.refreshing => "Loading the library…".into(),
            None => state,
        }
    }
}

/// A place whose cover changes: what it shows, what it showed before (fading out), and
/// since when the new one fades in (infinite: not loaded yet).
#[derive(Clone, Copy)]
struct CoverSlot {
    shown: Option<Cover>,
    seed: Uuid,
    previous: Option<(Cover, Uuid)>,
    since: f64,
}

pub struct App {
    shell: Shell,
    /// When the settings last changed (they're saved a moment after: a slider drag
    /// changes them many times).
    settings_changed: Option<Instant>,
    /// The `?` overlay (every key) is open.
    keys_open: bool,
    spectrum: Spectrum,
    last_time: f64,
    /// When freed memory was last given back (see `memory::trim`).
    last_trim: f64,
    pages: views::Pages,

    page: Page,
    back: Vec<Page>,
    forward: Vec<Page>,
    page_since: f64,
    /// Links handed over by later starts of the app (see `links`), and one waiting to be
    /// opened (once logged in and the library is here).
    links: Option<mpsc::Receiver<String>>,
    pending_link: Option<crate::links::Link>,
    /// A share link being made: the server's answer (the link, or why not).
    share_answer: Arc<Mutex<Option<Result<String, String>>>>,
    /// The share link popup, while it's open (see `share`).
    share_dialog: Option<share::ShareDialog>,
}

impl App {
    /// `link`: the `acetate://` link the app was started for, if any.
    pub fn new(cc: &eframe::CreationContext, link: Option<String>) -> App {
        theme::apply(&cc.egui_ctx);
        let runtime = Runtime::new().expect("a tokio runtime");
        let data_dir = crate::root::root();
        let cache_dir = crate::root::cache_dir();
        let app_lock = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::root::hold_app_lock(app_lock.clone());
        let api = Api::new(data_dir.clone());
        let controller = runtime.block_on(async { Controller::new(api.clone(), cc.egui_ctx.clone()) });
        runtime.spawn(controller.clone().monitor());
        // The server going away or coming back is shown at once.
        let (mut connection, repaint) = (api.watch_connection(), cc.egui_ctx.clone());
        runtime.spawn(async move {
            while connection.changed().await.is_ok() {
                repaint.request_repaint();
            }
        });
        controller.attach_media_keys(runtime.handle().clone(), cache_dir.clone(), window_handle(cc));
        let covers = Covers::new(api.clone(), runtime.handle().clone(), cache_dir.clone());
        let waker: acetate_core::Waker = Arc::new(Repaint(cc.egui_ctx.clone()));
        let playlists = crate::playlists::Playlists::new(api.clone(), runtime.handle().clone(), waker.clone());
        let details = |kind| (kind, api.clone(), runtime.handle().clone(), waker.clone());
        let (k, a, r, c) = details("tracks");
        let track_details = crate::details::Details::new(k, a, r, c);
        let (k, a, r, c) = details("albums");
        let album_details = crate::details::Details::new(k, a, r, c);
        let (k, a, r, c) = details("artists");
        let artist_details = crate::details::Details::new(k, a, r, c);
        let (k, a, r, c) = details("groupings");
        let grouping_details = crate::details::Details::new(k, a, r, c);
        let mut shell = Shell {
            ctx: cc.egui_ctx.clone(),
            waker,
            logo: widgets::logo_texture(&cc.egui_ctx),
            settings: crate::settings::Settings::load(&data_dir),
            account: crate::account::Account::default(),
            warned_version: None,
            connection: acetate_core::api::Connection::Online,
            edit_mode_when_back: false,
            edit_mode: false,
            picker: edit::Picker::default(),
            flags: Default::default(),
            track_editors: Default::default(),
            album_forms: Default::default(),
            artist_forms: Default::default(),
            form_dirty: false,
            page_takes_space: false,
            modal_open: false,
            modal_was_open: false,
            page_takes_back: false,
            page_takes_keys: false,
            slash_target: None,
            list_target: None,
            palette_open: false,
            messages: Default::default(),
            runtime,
            api,
            controller,
            covers,
            original: None,
            watching: None,
            catalog: Cached::default(),
            generation: 0,
            trim_asked: false,
            actions: Vec::new(),
            data_dir,
            cache_dir,
            mirror: Default::default(),
            app_lock,
            offline: None,
            downloads_checked: Instant::now(),
            marks: Marks::default(),
            cover_slots: Default::default(),
            resume: None,
            resumed: false,
            playlists,
            track_details,
            album_details,
            artist_details,
            grouping_details,
            updates: mpsc::channel(),
            store: None,
            refresh_asked: None,
            refresh_again: false,
            outbox: None,
        };
        if shell.api.session().is_some() {
            shell.load_library(&cc.egui_ctx);
        }
        shell.controller.set_volume(shell.settings.volume, shell.settings.muted);
        shell.runtime.block_on(shell.controller.set_quality(shell.settings.stream_kbps));
        shell.controller.set_equalizer(shell.settings.eq_enabled, shell.settings.eq_gains);
        if let Some(output) = shell.settings.output.clone() {
            let c = shell.controller.clone();
            shell.runtime.spawn(async move { c.set_output(Some(output)).await });
        }
        App {
            shell,
            settings_changed: None,
            keys_open: false,
            spectrum: Spectrum::new(64),
            last_time: 0.0,
            last_trim: 0.0,
            pages: views::Pages::default(),
            page: Page::Albums,
            back: Vec::new(),
            forward: Vec::new(),
            page_since: 0.0,
            links: crate::links::listen(cc.egui_ctx.clone()),
            pending_link: link.as_deref().and_then(crate::links::parse),
            share_answer: Arc::default(),
            share_dialog: None,
        }
    }

    /// What Ctrl+L links to: a track's, album's or artist's page itself; on a list, the
    /// track or album selected in it.
    fn page_target(&self) -> Option<crate::links::Target> {
        match self.page {
            Page::Track(id) => Some(crate::links::Target::Track(id)),
            Page::Album(id) => Some(crate::links::Target::Album(id)),
            Page::Artist(id) => Some(crate::links::Target::Artist(id)),
            _ => self.shell.list_target,
        }
    }

    /// Opens the share link popup for a track or album (`share`): its options, then the
    /// link is made (`make_share`).
    fn open_share_dialog(&mut self, target: crate::links::Target) {
        use crate::links::Target;
        match target {
            Target::Track(_) | Target::Album(_) => {}
            Target::Artist(_) => {
                self.shell.playlists.notice = Some(("Share links are to a track or an album (an artist has its plain link: Ctrl+L).".into(), Instant::now()));
                return;
            }
        }
        if !self.shell.account.can("share") {
            self.shell.playlists.notice = Some(("Making share links needs the share permission (the plain link, Ctrl+L, works for anyone with an account).".into(), Instant::now()));
            return;
        }
        if !self.shell.needs_server("making a share link") {
            return;
        }
        // The qualities the server makes copies at: asked now, they arrive in the popup.
        let answer = Arc::new(Mutex::new(None));
        self.share_dialog = Some(share::ShareDialog::new(target, self.shell.account.can("download_original"), answer.clone()));
        let (api, ctx) = (self.shell.api.clone(), self.shell.ctx.clone());
        self.shell.runtime.spawn(async move {
            #[derive(serde::Deserialize)]
            struct Qualities {
                kbps: Vec<u32>,
            }
            if let Ok(q) = api.send_json::<Qualities>(reqwest::Method::GET, "/api/qualities", None).await {
                *answer.lock().unwrap() = Some(q.kbps);
                ctx.request_repaint();
            }
        });
    }

    /// The popup, while it's open; makes the link when it asks.
    fn show_share(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.share_dialog.as_mut() else { return };
        self.shell.modal_open = true;
        match dialog.show(ctx) {
            share::Outcome::Open => {}
            share::Outcome::Close => self.share_dialog = None,
            share::Outcome::Create { target, original, days, kbps } => {
                self.share_dialog = None;
                self.make_share(target, original, days, kbps);
            }
        }
    }

    /// Makes a share link (`POST /api/shares`) to a track or album; it's copied when the
    /// server answers (`take_share`). `original`: it plays the original file, not a
    /// lower-quality copy; `days`: it expires after that many (None: never).
    fn make_share(&mut self, target: crate::links::Target, original: bool, days: Option<u32>, kbps: Option<u32>) {
        use crate::links::Target;
        let mut body = match target {
            Target::Track(id) => serde_json::json!({ "track_id": id }),
            Target::Album(id) => serde_json::json!({ "album_id": id }),
            Target::Artist(_) => return,
        };
        body["original"] = original.into();
        if let Some(kbps) = kbps {
            body["kbps"] = kbps.into();
        }
        if let Some(days) = days {
            body["expires_at"] = share::expires_at(days).into();
        }
        let Some(session) = self.shell.api.session() else { return };
        #[derive(serde::Deserialize)]
        struct Created {
            token: String,
        }
        let (api, answer, ctx) = (self.shell.api.clone(), self.share_answer.clone(), self.shell.ctx.clone());
        self.shell.runtime.spawn(async move {
            let result = api
                .send_json::<Created>(reqwest::Method::POST, "/api/shares", Some(body))
                .await
                .map(|c| format!("{}/s/{}", session.server.trim_end_matches('/'), c.token))
                .map_err(|err| err.message);
            *answer.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }

    /// A share link made: copied, and said so.
    fn take_share(&mut self, ctx: &egui::Context) {
        let Some(result) = self.share_answer.lock().unwrap().take() else { return };
        let text = match result {
            Ok(link) => {
                ctx.copy_text(link.clone());
                format!("Share link copied (anyone can open it; revoke it in the web UI's Shares): {link}")
            }
            Err(err) => format!("Couldn't make a share link: {err}"),
        };
        self.shell.playlists.notice = Some((text, Instant::now()));
    }

    /// Opens the link waiting (one handed over, or the one the app started for), once
    /// there's a library to open it in. A link to another server says so instead.
    fn open_links(&mut self) {
        if let Some(links) = &self.links {
            while let Ok(text) = links.try_recv() {
                match crate::links::parse(&text) {
                    Some(link) => self.pending_link = Some(link),
                    None => self.shell.playlists.notice = Some((format!("That isn't a link this app opens: {text}"), Instant::now())),
                }
            }
        }
        let (Some(session), Some(_)) = (self.shell.api.session(), self.shell.catalog.value.as_ref()) else { return };
        let Some(link) = self.pending_link.take() else { return };
        if let Some(server) = link.server.as_deref()
            && !crate::links::same_server(server, &session.server)
        {
            let text = format!("That link is to another server ({server}); you're logged in to {}.", session.server);
            self.shell.playlists.notice = Some((text, Instant::now()));
            return;
        }
        let page = match link.target {
            crate::links::Target::Track(id) => Page::Track(id),
            crate::links::Target::Album(id) => Page::Album(id),
            crate::links::Target::Artist(id) => Page::Artist(id),
        };
        self.shell.act(Action::Go(page));
    }

    fn go(&mut self, page: Page, now: f64) {
        if page != self.page {
            self.back.push(self.page);
            self.forward.clear();
            self.page = page;
            self.page_since = now;
            self.pages.opened(page);
            if page == Page::Settings {
                self.pages.settings.opened(&self.shell);
            }
            if page == Page::Trash {
                self.pages.trash.opened(&self.shell);
            }
            if page == Page::Changes {
                self.pages.changes.opened(&self.shell);
            }
            // Details shown as kept, and fetched again (cache first).
            match page {
                Page::Track(id) => self.shell.track_details.open(id),
                Page::Album(id) => self.shell.album_details.open(id),
                Page::Artist(id) => self.shell.artist_details.open(id),
                Page::Grouping(id) => self.shell.grouping_details.open(id),
                _ => {}
            }
        }
    }

    /// Carries out what the frame asked for.
    fn apply(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for action in std::mem::take(&mut self.shell.actions) {
            match action {
                Action::Go(page) => self.go(page, now),
                Action::Back => {
                    if let Some(page) = self.back.pop() {
                        self.forward.push(self.page);
                        self.page = page;
                        self.page_since = now;
                    }
                }
                Action::Forward => {
                    if let Some(page) = self.forward.pop() {
                        self.back.push(self.page);
                        self.page = page;
                        self.page_since = now;
                    }
                }
                Action::Play { tracks, start } => self.play(tracks, start),
                Action::Warm { a, b, at } => {
                    let Some(catalog) = self.shell.catalog.value.clone() else { continue };
                    let controller = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { controller.warm(queue_item(&catalog, a), queue_item(&catalog, b), at).await });
                }
                Action::Compare { track, other, at, follow } => {
                    let Some(catalog) = self.shell.catalog.value.clone() else { continue };
                    let controller = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { controller.compare(queue_item(&catalog, track), queue_item(&catalog, other), at, follow).await });
                }
                Action::Enqueue { tracks, next } => {
                    let Some(catalog) = self.shell.catalog.value.clone() else { continue };
                    let (controller, count) = (self.shell.controller.clone(), tracks.len());
                    self.shell.runtime.spawn(async move {
                        let items: Vec<QueueItem> = tracks.iter().map(|&i| queue_item(&catalog, i)).collect();
                        controller.enqueue(items, next).await
                    });
                    let what = acetate_core::format::count(count, "track");
                    let text = if next { format!("Queued {what} first") } else { format!("Queued {what}") };
                    self.shell.playlists.notice = Some((text, Instant::now()));
                }
                Action::PlayPlace(place) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.play_place(place).await });
                }
                Action::RemovePlace(place) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.remove(place).await });
                }
                Action::ClearQueued => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.clear_queued().await });
                }
                Action::MoveQueued(from, to) => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.move_queued(from, to).await });
                }
                Action::TogglePause => self.shell.controller.toggle_pause(),
                Action::Next => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.next().await });
                }
                Action::Previous => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.previous().await });
                }
                Action::SeekBy(seconds) => self.shell.controller.seek_by(seconds),
                Action::SeekTo(at) => self.shell.controller.seek(at),
                Action::VolumeBy(by) => {
                    let (volume, _) = self.shell.controller.volume();
                    self.shell.controller.set_volume(volume + by, false);
                }
                Action::SetVolume(volume) => self.shell.controller.set_volume(volume, false),
                Action::ToggleMute => {
                    let (volume, muted) = self.shell.controller.volume();
                    self.shell.controller.set_volume(volume, !muted);
                }
                Action::SetOutput(name) => {
                    self.shell.settings.output = name.clone();
                    self.shell.settings.save(&self.shell.data_dir);
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.set_output(name).await });
                }
                Action::SetStreamQuality(kbps) => {
                    self.shell.settings.stream_kbps = kbps;
                    self.shell.settings.save(&self.shell.data_dir);
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.set_quality(kbps).await });
                    if let Some(offline) = &self.shell.offline {
                        offline.set_quality(kbps);
                    }
                }
                Action::SetEqualizer(enabled, gains) => {
                    // Heard at once; saved a second after the last change, like the volume.
                    self.shell.settings.eq_enabled = enabled;
                    self.shell.settings.eq_gains = gains;
                    self.shell.controller.set_equalizer(enabled, gains);
                    self.settings_changed = Some(Instant::now());
                }
                Action::ToggleRepeat => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.toggle_repeat().await });
                }
                Action::ToggleShuffle => {
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.toggle_shuffle().await });
                }
                Action::ToggleStar(kind, id) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { starred: !mark.starred, ..mark });
                }
                Action::Rate(kind, id, rating) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { rating: rating.clamp(0, 10), ..mark });
                }
                Action::RateBy(kind, id, by) => {
                    let mark = self.shell.mark(id);
                    self.shell.change_mark(kind, id, Mark { rating: (mark.rating + by).clamp(0, 10), ..mark });
                }
                Action::AddToPlaylist(tracks) => self.pages.palette.open(ctx, views::palette::Mode::AddTo(tracks)),
                Action::RemoveFromPlaylist(tracks) => {
                    if self.shell.needs_server("taking tracks out of a playlist") {
                        let catalog = self.shell.catalog.value.clone();
                        if self.shell.playlists.containing(catalog.as_deref(), &tracks).is_empty() {
                            let text = if tracks.len() == 1 { "That track isn't in any of your playlists" } else { "None of those tracks is in any of your playlists" };
                            self.shell.playlists.notice = Some((text.into(), std::time::Instant::now()));
                        } else {
                            self.pages.palette.open(ctx, views::palette::Mode::RemoveFrom(tracks));
                        }
                    }
                }
                Action::NewPlaylist => {
                    if self.shell.needs_server("making a playlist") {
                        self.pages.palette.open(ctx, views::palette::Mode::Name(None));
                    }
                }
                Action::RenamePlaylist(id) => {
                    if self.shell.needs_server("renaming a playlist") {
                        self.pages.palette.open(ctx, views::palette::Mode::Name(Some(id)));
                    }
                }
                Action::RefreshLibrary => self.shell.refresh_library(ctx),
                Action::ShowKeys => self.keys_open = true,
                Action::ShowMessages => self.shell.messages.open = !self.shell.messages.open,
                Action::CopyShareLink(target) => self.open_share_dialog(target),
                Action::CopyPageShareLink => match self.page_target() {
                    Some(target) => self.open_share_dialog(target),
                    None => self.shell.playlists.notice = Some(("Share links are to a track or an album: open or select one, then Ctrl+Shift+L.".into(), Instant::now())),
                },
                Action::CopyPageLink => {
                    match self.page_target() {
                        Some(target) => self.shell.act(Action::CopyLink(target)),
                        None => self.shell.playlists.notice = Some(("Links are to a track, album or artist: open or select one, then Ctrl+L.".into(), Instant::now())),
                    }
                }
                Action::CopyLink(target) => {
                    if let Some(session) = self.shell.api.session() {
                        let link = crate::links::web(&session.server, target);
                        ctx.copy_text(link.clone());
                        self.shell.playlists.notice = Some((format!("Link copied: {link}"), Instant::now()));
                    }
                }
                Action::Pick { kind, except } => self.pages.palette.open(ctx, views::palette::Mode::Pick(kind, except)),
                Action::ToggleEditMode => {
                    if !self.shell.account.can("edit_metadata") {
                        self.shell.playlists.notice = Some(("Editing needs the edit_metadata permission.".into(), Instant::now()));
                    } else if self.shell.edit_mode || self.shell.needs_server("editing") {
                        // (Turning it off is always possible.)
                        self.shell.edit_mode = !self.shell.edit_mode;
                        self.shell.edit_mode_when_back = false;
                    }
                }
                Action::LoggedIn => {
                    self.shell.load_library(ctx);
                    self.page = Page::Albums;
                    self.back.clear();
                    self.forward.clear();
                }
                Action::LogOut => {
                    self.shell.mirror.shut_down(self.shell.runtime.handle());
                    self.shell.api.logout_and_end(self.shell.runtime.handle());
                    // (The queue was kept on disk as it played; it comes back at the next login.)
                    let c = self.shell.controller.clone();
                    self.shell.runtime.spawn(async move { c.stop().await });
                    self.shell.catalog = Cached::default();
                    self.shell.account = Default::default();
                    self.shell.connection = acetate_core::api::Connection::Online;
                    self.shell.controller.on_disk.set(None);
                    self.shell.marks.clear();
                    self.shell.offline = None;
                    self.shell.covers.set_here(covers::Here::default());
                    self.shell.playlists.reset(None, None);
                    self.shell.outbox = None;
                    self.shell.controller.send_listens_to(None);
                    self.shell.resume = None;
                    self.shell.track_details.reset(None);
                    self.shell.album_details.reset(None);
                    self.shell.artist_details.reset(None);
                    self.shell.grouping_details.reset(None);
                    self.shell.track_editors.clear();
                    self.shell.album_forms.clear();
                    self.shell.artist_forms.clear();
                    self.shell.flags.reset(None);
                    self.shell.store = None;
                    self.shell.generation += 1;
                }
            }
        }
    }

    /// Queues tracks from the catalog and starts playing.
    fn play(&mut self, tracks: Vec<u32>, start: usize) {
        let Some(catalog) = self.shell.catalog.value.clone() else { return };
        let controller = self.shell.controller.clone();
        // Built in the background: a queue of the whole library is 185k items.
        self.shell.runtime.spawn(async move {
            let items: Vec<QueueItem> = tracks.iter().map(|&i| queue_item(&catalog, i)).collect();
            controller.play(items, start).await
        });
    }

    /// Saves volume changes (made anywhere: keys, the slider, the system's media controls)
    /// a second after the last one.
    fn keep_settings(&mut self, player: &PlayerView) {
        let settings = &mut self.shell.settings;
        if (player.volume, player.muted) != (settings.volume, settings.muted) {
            settings.volume = player.volume;
            settings.muted = player.muted;
            self.settings_changed = Some(Instant::now());
        }
        if self.settings_changed.is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(1)) {
            self.settings_changed = None;
            self.shell.settings.save(&self.shell.data_dir);
        } else if self.settings_changed.is_some() {
            self.shell.ctx.request_repaint_after(std::time::Duration::from_millis(1100));
        }
    }

    /// Keys that work everywhere (outside text fields).
    fn keys(&mut self, ctx: &egui::Context) {
        // The key overlay takes its own keys (Esc closes it, not "back"); so does a popup.
        if self.keys_open || self.shell.modal_open || self.shell.messages.open {
            return;
        }
        let m = ctx.input(|i| i.modifiers);
        let key = |k: Key| ctx.input(|i| i.key_pressed(k));
        // By the character typed, not the key: "/" and "?" are on different keys (and
        // need Shift or not) on different keyboard layouts.
        let typed = |text: &str| ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t == text)));
        let pages = [
            (Key::Num1, Page::Albums),
            (Key::Num2, Page::Artists),
            (Key::Num3, Page::Tracks),
            (Key::Num4, Page::NowPlaying),
            (Key::Num5, Page::Queue),
            (Key::Num6, Page::Review),
        ];

        // Everywhere, text fields included.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::K)) {
            self.pages.palette.toggle(ctx);
        }
        if key(Key::F5) {
            self.shell.act(Action::RefreshLibrary);
        }
        // Alt+M: the earlier messages (Alt+M or Esc closes them: see `Messages::show`).
        if !self.shell.messages.open && ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::M)) {
            self.shell.act(Action::ShowMessages);
        }
        if key(Key::F1) && !self.keys_open {
            self.shell.act(Action::ShowKeys);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Comma)) {
            self.shell.act(Action::Go(Page::Settings));
        }
        // Alt+E: edit mode on or off, from a text field too (a commandline).
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, Key::E)) {
            // The "e" it typed (some keyboards do) doesn't go into the field either.
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t.eq_ignore_ascii_case("e"))));
            self.shell.act(Action::ToggleEditMode);
        }
        // Ctrl+L: a link to what's shown (a track, album or artist), to send to someone
        // with an account; Ctrl+Shift+L: a share link, for anyone. (Shift first: a key
        // asked for without it matches with it too.)
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::L)) {
            self.shell.act(Action::CopyPageShareLink);
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::L)) {
            self.shell.act(Action::CopyPageLink);
        }
        if m.alt && key(Key::ArrowLeft) {
            self.shell.act(Action::Back);
        } else if m.alt && key(Key::ArrowRight) {
            self.shell.act(Action::Forward);
        }
        if m.command {
            for (k, page) in pages {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k)) {
                    self.shell.act(Action::Go(page));
                    // Leave the field, so the page's keys work right away.
                    ctx.memory_mut(|mem| mem.stop_text_input());
                }
            }
        }

        // Outside text fields (there the arrows and letters are for the text), and when the
        // page doesn't take the keys itself.
        if ctx.egui_wants_keyboard_input() || self.shell.page_takes_keys {
            return;
        }
        if m.ctrl && key(Key::ArrowLeft) {
            self.shell.act(Action::Previous);
        } else if m.ctrl && key(Key::ArrowRight) {
            self.shell.act(Action::Next);
        } else if m.shift && key(Key::ArrowLeft) {
            self.shell.act(Action::SeekBy(-10.0));
        } else if m.shift && key(Key::ArrowRight) {
            self.shell.act(Action::SeekBy(10.0));
        }
        if m.shift && key(Key::S) {
            // Star what plays.
            // (Read first: the lock is released at the end of this statement.)
            let playing = playing_track(&self.shell.controller.view.lock().unwrap());
            if let Some(track) = playing {
                self.shell.act(Action::ToggleStar(Kind::Track, track));
            }
        }
        if m.ctrl || m.alt || m.command {
            return;
        }
        for (k, page) in pages {
            if key(k) {
                self.shell.act(Action::Go(page));
            }
        }
        if (key(Key::Escape) || key(Key::Backspace)) && !self.shell.form_dirty && !self.shell.page_takes_back {
            self.shell.act(Action::Back);
        } else if key(Key::Space) && self.page != Page::Review && !self.shell.page_takes_space {
            // (On the Review page, Space plays the track being fixed: the page does it.)
            self.shell.act(Action::TogglePause);
        } else if key(Key::Num9) {
            self.shell.act(Action::VolumeBy(-0.05));
        } else if key(Key::Num0) {
            self.shell.act(Action::VolumeBy(0.05));
        } else if !m.shift && key(Key::M) {
            self.shell.act(Action::ToggleMute);
        } else if !m.shift && key(Key::E) {
            self.shell.act(Action::ToggleEditMode);
        } else if !m.shift && key(Key::R) {
            self.shell.act(Action::ToggleRepeat);
        } else if !m.shift && key(Key::Z) {
            self.shell.act(Action::ToggleShuffle);
        } else if typed("?") {
            self.shell.act(Action::ShowKeys);
        } else if typed("/") {
            // Swallow the "/" itself, or the field would get it as text this frame.
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Slash);
                i.events.retain(|e| !matches!(e, egui::Event::Text(t) if t == "/"));
            });
            let target = self.shell.slash_target.unwrap_or(egui::Id::new(views::FILTER_ID));
            ctx.memory_mut(|mem| mem.request_focus(target));
        }
    }
}

impl eframe::App for App {
    /// The mirror stops (between files: background work would otherwise keep the app from
    /// closing until it's done).
    fn on_exit(&mut self) {
        self.shell.mirror.shut_down(self.shell.runtime.handle());
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        let dt = ((now - self.last_time) as f32).clamp(0.0, 0.1);
        self.last_time = now;
        if now - self.last_trim > 60.0 || std::mem::take(&mut self.shell.trim_asked) {
            self.last_trim = now;
            crate::memory::trim();
        }
        self.shell.covers.poll(&ctx);
        self.shell.take_updates();
        self.pages.login.poll(&mut self.shell);
        // A request was refused as not logged in: back to the login screen, saying why.
        if self.shell.api.take_expired() && self.shell.api.session().is_some() {
            self.shell.act(Action::LogOut);
            self.pages.login.ended("Your session ended (it was signed out, or it expired). Log in again.");
        }

        let Some(session) = self.shell.api.session() else {
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(P.bg))
                .show(ui, |ui| self.pages.login.show(ui, &mut self.shell));
            self.apply(&ctx);
            return;
        };
        // The server turned a request away for being another version (it was updated, or
        // this app): ask which it is now.
        if self.shell.api.take_incompatible() {
            self.shell.account.fetch(&self.shell.api, self.shell.runtime.handle(), &self.shell.waker);
        }
        self.shell.account.poll();
        // Not the same version as the server: nothing else works, so only the way out.
        if let Some(server) = self.shell.account.server.clone().filter(|s| s.blocked()) {
            egui::CentralPanel::default()
                .frame(Frame::NONE.fill(P.bg).inner_margin(Margin::symmetric(40, 20)))
                .show(ui, |ui| self.pages.incompatible.show(ui, &mut self.shell, &server));
            self.apply(&ctx);
            return;
        }
        self.keys(&ctx);
        self.open_links();
        self.take_share(&ctx);
        self.shell.take_sent();
        if let Some(notice) = self.shell.controller.notice.lock().unwrap().take() {
            self.shell.playlists.notice = Some((notice, Instant::now()));
        }
        self.shell.playlists.poll();
        self.shell.account.poll();
        // The server is of another version than this app: said once per server version.
        if let Some(warning) = self.shell.account.server.as_ref().and_then(|s| s.warning())
            && self.shell.warned_version.as_ref() != Some(&warning)
        {
            *self.shell.controller.error.lock().unwrap() = Some((warning.clone(), Instant::now()));
            self.shell.warned_version = Some(warning);
        }
        self.shell.follow_connection();
        // Admins: the mirror is opened (it runs if it's turned on).
        if self.shell.account.can("admin") {
            let shell = &mut self.shell;
            shell.mirror.start(shell.runtime.handle(), shell.api.clone(), shell.data_dir.clone(), shell.app_lock.clone(), ctx.clone());
        }
        // The library is asked what changed every two minutes (a delta: cheap), so
        // edits made elsewhere show up without F5.
        // (Two minutes after the last attempt, so a failing one isn't retried every frame.)
        if self.shell.refresh_asked.is_some_and(|at| at.elapsed() > std::time::Duration::from_secs(120)) {
            self.shell.act(Action::RefreshLibrary);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(121));
        // What the pins want changes with the library and the stars: looked at again now and then.
        if self.shell.downloads_checked.elapsed() > std::time::Duration::from_secs(30) {
            self.shell.refresh_downloads();
        }
        self.shell.track_details.poll();
        self.shell.album_details.poll();
        self.shell.artist_details.poll();
        self.shell.grouping_details.poll();
        if let Some(id) = self.shell.playlists.created.take() {
            self.shell.act(Action::Go(Page::Playlist(id)));
        }
        let player = self.shell.controller.view.lock().unwrap().clone();
        self.shell.resume_queue(&player);
        self.keep_settings(&player);
        // The next track's cover, loaded ahead, so it can fade in the moment it plays
        // (in the player bar, and on "now playing").
        if let Some(next) = player.next().and_then(|i| i.cover) {
            self.shell.covers.get(&ctx, next, covers::CoverSize::Small);
            self.shell.covers.get(&ctx, next, covers::CoverSize::Standard);
        }

        egui::Panel::bottom("player")
            .exact_size(84.0)
            .frame(Frame::NONE.fill(P.sidebar).inner_margin(Margin::symmetric(18, 10)).stroke(Stroke::new(1.0, P.line)))
            .show(ui, |ui| views::player_bar::show(ui, &mut self.shell, &player));

        egui::Panel::left("sidebar")
            .exact_size(212.0)
            .resizable(false)
            .frame(Frame::NONE.fill(P.sidebar).inner_margin(Margin::symmetric(14, 18)))
            .show(ui, |ui| views::sidebar::show(ui, &mut self.shell, self.page, &session.username));

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(P.bg).inner_margin(Margin::symmetric(28, 22)))
            .show(ui, |ui| {
                // The page slides and fades in when switched.
                let t = ease_out((((now - self.page_since) as f32) / 0.24).clamp(0.0, 1.0));
                if t < 1.0 {
                    ctx.request_repaint();
                }
                // Edit mode: an accent line along the top says so; it grows out from the middle
                // when edit mode is turned on, and shrinks back into it when it's off.
                let edit = covers::ease_out(ctx.animate_bool_with_time(egui::Id::new("edit-line"), self.shell.edit_mode, 0.3));
                if edit > 0.0 {
                    let top = ui.max_rect();
                    let width = (top.width() + 56.0) * edit;
                    let line = egui::Rect::from_center_size(egui::pos2(top.center().x, top.top() - 22.0 + 1.5), egui::vec2(width, 3.0));
                    ui.painter().rect_filled(line, 0, P.accent.gamma_multiply(edit));
                }
                ui.multiply_opacity(t);
                ui.add_space((1.0 - t) * 16.0);
                // Set again by a form with changes, if one is shown.
                self.shell.form_dirty = false;
                self.shell.page_takes_space = false;
                self.shell.page_takes_back = false;
                self.shell.page_takes_keys = false;
                self.shell.slash_target = None;
                self.shell.modal_was_open = self.shell.modal_open;
                self.shell.list_target = None;
                self.shell.modal_open = false;
                self.shell.palette_open = self.pages.palette.is_open();
                self.pages.show(ui, &mut self.shell, self.page, &player, &mut self.spectrum, dt);
            });
        self.pages.palette.show(&ctx, &mut self.shell);
        self.shell.show_original(&ctx);
        self.shell.show_watch(&ctx);
        self.show_share(&ctx);
        keymap::overlay(&ctx, &mut self.keys_open);
        self.shell.messages.show(&ctx);
        self.apply(&ctx);
    }
}

/// The window's handle, for the system's media controls (Windows ties them to a window;
/// None elsewhere).
fn window_handle(cc: &eframe::CreationContext) -> Option<*mut std::ffi::c_void> {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match cc.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut std::ffi::c_void),
            _ => None,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cc;
        None
    }
}

/// A track (a position in the catalog) as the queue keeps it.
pub fn queue_item(catalog: &Catalog, i: u32) -> QueueItem {
    crate::playback::queue_item(catalog, i)
}

pub use acetate_core::format::{count, thousands};

/// What plays right now, if anything (for pages marking the playing track).
pub fn playing_track(player: &PlayerView) -> Option<Uuid> {
    // A file that didn't import plays like a track, but isn't one.
    player.current().filter(|c| c.source == crate::playback::AudioSource::File).map(|c| c.track_id)
}

/// The core's way to wake the window: a repaint.
struct Repaint(egui::Context);

impl acetate_core::Wake for Repaint {
    fn changed(&self) {
        self.0.request_repaint();
    }
}
