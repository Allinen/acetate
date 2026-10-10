//! The mirror inside the app (admins): opened after logging in, once the app holds its
//! lock and the mirror service (if it runs) has stepped aside; then kept up to date in the
//! background for as long as the app is open, while mirroring is on. The Settings page
//! changes it through `Command`s, run one at a time by `supervise`: a change of settings
//! restarts the watching, a rehash or a removal pauses it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::sync::mpsc;

use super::{Detected, Mirror, Snapshot, Status};
use crate::api::Api;

pub enum Command {
    /// The settings changed, or "Sync now": watch again with them (or stop, if mirroring
    /// is off). Ends a pause.
    Restart,
    /// Stop what's running and don't watch until `Restart` (or the app starts again).
    Pause,
    /// Ask the server for its list (to show what there is before mirroring is on).
    Refresh,
    Rehash,
    /// Remove the removable blobs of a group (state, volume): the user chose to.
    Remove(Option<String>, Option<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Opening {
    NotYet,
    /// Waiting for the app's lock, or for the mirror service to step aside.
    Waiting(String),
    Open,
    Failed(String),
}

#[derive(Default)]
pub struct InApp {
    mirror: Arc<Mutex<Option<Arc<Mirror>>>>,
    opening: Arc<Mutex<Option<Opening>>>,
    commands: Option<mpsc::UnboundedSender<Command>>,
    task: Option<tokio::task::JoinHandle<()>>,
    /// What the last rehash or removal did (or why it failed), to show.
    pub outcome: Arc<Mutex<Option<String>>>,
    /// Read in the background (`poll`), every few seconds while shown.
    snapshot: Arc<Mutex<Option<Result<Snapshot, String>>>>,
    snapshot_at: Option<Instant>,
    snapshot_running: Arc<AtomicBool>,
    /// Paused (`Command::Pause`).
    pub paused: Arc<AtomicBool>,
    /// Mirroring is turned on in the settings (kept here for the pages, every frame).
    enabled: Arc<AtomicBool>,
    /// Which of the server's folders a folder here has the files of (`detect`), and a way
    /// to stop looking.
    pub detected: Arc<Mutex<Option<(PathBuf, Option<Result<Detected, String>>)>>>,
    detect_stop: Arc<AtomicBool>,
}

impl InApp {
    /// Opens the mirror (once): waits for the app's lock (`app_lock`), then for the mirror
    /// lock (the service steps aside within seconds of the app opening), then keeps
    /// watching while mirroring is on.
    pub fn start(&mut self, runtime: &Handle, api: Api, root: PathBuf, app_lock: Arc<AtomicBool>, ctx: eframe::egui::Context) {
        if self.task.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        self.commands = Some(sender);
        let (slot, opening, outcome, paused, enabled) = (self.mirror.clone(), self.opening.clone(), self.outcome.clone(), self.paused.clone(), self.enabled.clone());
        *opening.lock().unwrap() = Some(Opening::NotYet);
        self.task = Some(runtime.spawn(async move {
            let started = Instant::now();
            let mirror = loop {
                if !app_lock.load(Relaxed) {
                    *opening.lock().unwrap() = Some(Opening::Waiting("Waiting for another Acetate window to close…".into()));
                } else {
                    let (root, api) = (root.clone(), api.clone());
                    match tokio::task::spawn_blocking(move || Mirror::open(&root, api)).await {
                        Ok(Ok(mirror)) => break Arc::new(mirror),
                        Ok(Err(err)) if started.elapsed() < Duration::from_secs(60) => {
                            let _ = err;
                            *opening.lock().unwrap() = Some(Opening::Waiting("Waiting for the mirror service to step aside…".into()));
                        }
                        Ok(Err(err)) => {
                            *opening.lock().unwrap() = Some(Opening::Failed(format!("{err:#}")));
                            ctx.request_repaint();
                            return;
                        }
                        Err(err) => {
                            *opening.lock().unwrap() = Some(Opening::Failed(err.to_string()));
                            return;
                        }
                    }
                }
                ctx.request_repaint();
                tokio::time::sleep(Duration::from_secs(1)).await;
            };
            *slot.lock().unwrap() = Some(mirror.clone());
            *opening.lock().unwrap() = Some(Opening::Open);
            ctx.request_repaint();
            supervise(mirror, receiver, outcome, paused, enabled, ctx).await;
        }));
    }

    /// Stops the mirror and lets go of it (logging out, moving the root). Waits for it
    /// (moments: a run stops between files).
    pub fn shut_down(&mut self, runtime: &Handle) {
        self.commands = None;
        self.detect_stop.store(true, Relaxed);
        if let Some(mirror) = self.mirror.lock().unwrap().take() {
            mirror.stop();
        }
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = runtime.block_on(task);
        }
        *self.opening.lock().unwrap() = None;
        *self.snapshot.lock().unwrap() = None;
        self.snapshot_at = None;
    }

    /// The server is back: a mirror that was waiting to try again (it couldn't reach it)
    /// tries now. Paused or busy, it's left alone.
    pub fn server_back(&self) {
        let idle = self.status().is_some_and(|s| s.doing == super::Doing::Idle);
        if idle && self.is_enabled() && !self.paused.load(Relaxed) {
            self.send(Command::Restart);
        }
    }

    /// Mirroring is on: the pages have no use for a "Download" then (the files are here).
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Relaxed)
    }

    pub fn opening(&self) -> Option<Opening> {
        self.opening.lock().unwrap().clone()
    }

    pub fn status(&self) -> Option<Status> {
        Some(self.mirror.lock().unwrap().as_ref()?.status.lock().unwrap().clone())
    }

    pub fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Changes the settings and watches with them.
    pub fn set_config(&mut self, config: &super::index::Config) -> Result<(), String> {
        let mirror = self.mirror.lock().unwrap().clone().ok_or("The mirror isn't open yet.")?;
        mirror.set_config(config).map_err(|e| format!("{e:#}"))?;
        self.enabled.store(config.enabled, Relaxed);
        // Shown at once, not after the next look.
        if let Some(Ok(snapshot)) = &mut *self.snapshot.lock().unwrap() {
            snapshot.config = config.clone();
        }
        self.snapshot_at = None;
        self.send(Command::Restart);
        Ok(())
    }

    /// Works out (in the background) which of the server's folders `folder` has the files
    /// of: `detected` has it when done.
    pub fn detect(&mut self, runtime: &Handle, folder: PathBuf, ctx: &eframe::egui::Context) {
        let Some(mirror) = self.mirror.lock().unwrap().clone() else { return };
        self.detect_stop.store(true, Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        self.detect_stop = stop.clone();
        *self.detected.lock().unwrap() = Some((folder.clone(), None));
        let (slot, ctx) = (self.detected.clone(), ctx.clone());
        runtime.spawn_blocking(move || {
            let found = mirror.detect(&folder, &|| stop.load(Relaxed)).map_err(|e| format!("{e:#}"));
            let mut slot = slot.lock().unwrap();
            // Still the folder asked about (not cancelled, not another one since).
            if !stop.load(Relaxed) && slot.as_ref().is_some_and(|(f, _)| *f == folder) {
                *slot = Some((folder, Some(found)));
            }
            ctx.request_repaint();
        });
    }

    /// Stops looking at a folder (and forgets it).
    pub fn cancel_detect(&mut self) {
        self.detect_stop.store(true, Relaxed);
        *self.detected.lock().unwrap() = None;
    }

    /// The latest snapshot, reading a new one in the background if this one's older than
    /// `every`.
    pub fn snapshot(&mut self, runtime: &Handle, every: Duration, ctx: &eframe::egui::Context) -> Option<Result<Snapshot, String>> {
        let due = self.snapshot_at.is_none_or(|at| at.elapsed() >= every);
        let mirror = self.mirror.lock().unwrap().clone();
        if let Some(mirror) = mirror
            && due
            && !self.snapshot_running.swap(true, Relaxed)
        {
            self.snapshot_at = Some(Instant::now());
            let (slot, running, ctx) = (self.snapshot.clone(), self.snapshot_running.clone(), ctx.clone());
            runtime.spawn_blocking(move || {
                *slot.lock().unwrap() = Some(mirror.snapshot().map_err(|e| format!("{e:#}")));
                running.store(false, Relaxed);
                ctx.request_repaint();
            });
        }
        self.snapshot.lock().unwrap().clone()
    }
}

/// Watches while mirroring is on, and runs each command from the app, one at a time
/// (a command stops the watching; it starts again after).
async fn supervise(
    mirror: Arc<Mirror>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    outcome: Arc<Mutex<Option<String>>>,
    paused: Arc<AtomicBool>,
    enabled: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
) {
    loop {
        let is_enabled = mirror.config().is_ok_and(|c| c.enabled);
        enabled.store(is_enabled, Relaxed);
        let on = is_enabled && !paused.load(Relaxed);
        let command = if on {
            tokio::select! {
                _ = mirror.watch() => commands.recv().await,
                command = commands.recv() => command,
            }
        } else {
            commands.recv().await
        };
        // Whatever ran stops (the watching dropped above may have blocking work going on).
        mirror.stop();
        let said = match command {
            None => return,
            Some(Command::Restart) => {
                paused.store(false, Relaxed);
                None
            }
            Some(Command::Pause) => {
                paused.store(true, Relaxed);
                None
            }
            Some(Command::Refresh) => mirror.refresh(None).await.err().map(|e| format!("Couldn't ask the server: {e:#}")),
            Some(Command::Rehash) => Some(match mirror.rehash().await {
                Ok(r) if r.set_aside.is_empty() && r.missing == 0 => format!("All {} files hashed: each is what its name says.", r.checked),
                Ok(r) => format!(
                    "{} files hashed: {} weren't what their names say (set aside in the corrupt folder), {} were missing. They're fetched again.",
                    r.checked,
                    r.set_aside.len(),
                    r.missing
                ),
                Err(err) => format!("The rehash stopped: {err:#}"),
            }),
            Some(Command::Remove(state, volume)) => {
                let m = mirror.clone();
                let removed = tokio::task::spawn_blocking(move || m.remove_group(state.as_deref(), volume.as_deref())).await;
                Some(match removed {
                    Ok(Ok(n)) => format!("{n} files removed from this computer."),
                    Ok(Err(err)) => format!("Couldn't remove them: {err:#}"),
                    Err(err) => format!("Couldn't remove them: {err}"),
                })
            }
        };
        if let Some(said) = said {
            *outcome.lock().unwrap() = Some(said);
        }
        ctx.request_repaint();
    }
}
