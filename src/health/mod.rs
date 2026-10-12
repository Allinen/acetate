//! The library's health check: things that should be true of the library and its files,
//! looked for on request (Settings → Health in the desktop app, `acetate verify --blobs`).
//! A check only reads: it reports what it finds and never changes the library or deletes a
//! file; what to do about a finding is left to a person. (A repair a person asks for, like
//! `video_tracks::extract`, is its own request and never part of a run.)
//!
//! To add a check: describe it in `CHECKS` and give `run_check` a branch for its id. A
//! check returns findings; checks of one run share what is costly to get (the list of
//! stored files, the blobs the database knows) through `Context`.
//!
//! A run is started in the background (one at a time) and reports progress; the last
//! finished run's report is kept in memory until the server restarts.

pub mod blobs;
pub mod video_tracks;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::OnceCell;

use crate::db::blobs::MirrorEntry;
use crate::storage::BlobStore;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CheckInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// Reads every file (minutes to hours on a big library): not part of a quick run.
    pub heavy: bool,
}

pub const CHECKS: &[CheckInfo] = &[
    CheckInfo {
        id: "blobs-present",
        title: "Files are where the database says",
        description: "Every file the library uses is in the blob store, on the volume it is recorded on. Reads the folder listings, not the files.",
        heavy: false,
    },
    CheckInfo {
        id: "blobs-unused",
        title: "Files nothing uses",
        description: "Files in the blob store that no track, cover, extra or unimported file refers to (a failed upload, a crash while purging). Nothing is deleted.",
        heavy: false,
    },
    CheckInfo {
        id: "staged-covers",
        title: "Staged cover images nobody used",
        description: "Images put in the editor that no edit ever applied, a day or more ago.",
        heavy: false,
    },
    CheckInfo {
        id: "video-tracks",
        title: "Tracks that play a video file",
        description: "Tracks made from a video before its sound was taken out: they play the video, which only works where the player can decode its container. “Take the sound out” repairs them: the sound becomes the track's file and the video stays as an extra.",
        heavy: false,
    },
    CheckInfo {
        id: "blobs-intact",
        title: "Files are undamaged",
        description: "Reads every file in use and checks its content against its name (its SHA-256). Reads the whole library.",
        heavy: true,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something is wrong: a file is missing or damaged.
    Problem,
    /// Worth knowing, not harmful.
    Note,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub message: String,
}

impl Finding {
    pub fn problem(message: impl Into<String>) -> Finding {
        Finding { severity: Severity::Problem, message: message.into() }
    }

    pub fn note(message: impl Into<String>) -> Finding {
        Finding { severity: Severity::Note, message: message.into() }
    }
}

/// How far the check being run has got.
#[derive(Default)]
pub struct Progress {
    step: Mutex<String>,
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub fn step(&self, what: &str, total: u64) {
        *self.step.lock().unwrap() = what.to_string();
        self.done.store(0, Relaxed);
        self.total.store(total, Relaxed);
    }

    pub fn set_done(&self, done: u64) {
        self.done.store(done, Relaxed);
    }
}

/// What a run's checks share.
pub struct Context {
    pub pool: PgPool,
    pub store: BlobStore,
    pub progress: Arc<Progress>,
    scan: OnceCell<blobs::Scan>,
    entries: OnceCell<Vec<MirrorEntry>>,
}

impl Context {
    pub fn new(pool: PgPool, store: BlobStore, progress: Arc<Progress>) -> Context {
        Context { pool, store, progress, scan: OnceCell::new(), entries: OnceCell::new() }
    }

    /// The files in the blob store, listed once.
    pub(crate) async fn scan(&self) -> anyhow::Result<&blobs::Scan> {
        self.scan.get_or_try_init(|| blobs::Scan::list(&self.store)).await
    }

    /// Every blob the database records, with what it is for, got once.
    pub(crate) async fn entries(&self) -> anyhow::Result<&Vec<MirrorEntry>> {
        self.entries
            .get_or_try_init(|| async {
                let mut tx = crate::db::begin(&self.pool, None).await?;
                Ok(crate::db::blobs::mirror_all(&mut tx).await?)
            })
            .await
    }
}

pub async fn run_check(id: &str, ctx: &Context) -> anyhow::Result<Vec<Finding>> {
    match id {
        "blobs-present" => blobs::present(ctx).await,
        "blobs-unused" => blobs::unused(ctx).await,
        "staged-covers" => blobs::staged_covers(ctx).await,
        "blobs-intact" => blobs::intact(ctx).await,
        "video-tracks" => video_tracks::check(ctx).await,
        _ => anyhow::bail!("no check called {id}"),
    }
}

/// The most findings of one check kept in a report (the count is kept whole).
const MAX_SHOWN: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub id: &'static str,
    pub title: &'static str,
    /// How many there were; `findings` has the first `MAX_SHOWN`.
    pub total: usize,
    pub findings: Vec<Finding>,
    /// Why the check couldn't run, if it couldn't.
    pub error: Option<String>,
    pub seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub by: String,
    pub results: Vec<CheckResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunningStatus {
    pub started_at: DateTime<Utc>,
    pub by: String,
    /// The checks asked for, in order, and the one being run.
    pub checks: Vec<&'static str>,
    pub current: Option<&'static str>,
    pub step: String,
    pub done: u64,
    pub total: u64,
    pub finished: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub checks: &'static [CheckInfo],
    pub running: Option<RunningStatus>,
    pub last: Option<Report>,
}

struct Running {
    started_at: DateTime<Utc>,
    by: String,
    checks: Vec<&'static str>,
    current: Option<&'static str>,
    progress: Arc<Progress>,
    finished: Vec<CheckResult>,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    last: Option<Report>,
}

/// The server's runs: at most one at a time.
#[derive(Default)]
pub struct Health {
    state: Mutex<State>,
}

pub fn info(id: &str) -> Option<&'static CheckInfo> {
    CHECKS.iter().find(|c| c.id == id)
}

impl Health {
    pub fn status(&self) -> Status {
        let state = self.state.lock().unwrap();
        let running = state.running.as_ref().map(|r| RunningStatus {
            started_at: r.started_at,
            by: r.by.clone(),
            checks: r.checks.clone(),
            current: r.current,
            step: r.progress.step.lock().unwrap().clone(),
            done: r.progress.done.load(Relaxed),
            total: r.progress.total.load(Relaxed),
            finished: r.finished.clone(),
        });
        Status { checks: CHECKS, running, last: state.last.clone() }
    }

    /// Starts these checks in the background. Errors (with a sentence) if a run is going
    /// already or an id isn't a check.
    pub fn start(self: &Arc<Self>, pool: PgPool, store: BlobStore, ids: &[String], by: String) -> Result<(), String> {
        let mut checks = Vec::new();
        for id in ids {
            let info = info(id).ok_or_else(|| format!("no check called `{id}`"))?;
            if !checks.contains(&info.id) {
                checks.push(info.id);
            }
        }
        if checks.is_empty() {
            return Err("no checks asked for".to_string());
        }
        let progress = Arc::new(Progress::default());
        {
            let mut state = self.state.lock().unwrap();
            if state.running.is_some() {
                return Err("a health check is running already".to_string());
            }
            state.running = Some(Running {
                started_at: Utc::now(),
                by,
                checks: checks.clone(),
                current: None,
                progress: progress.clone(),
                finished: Vec::new(),
            });
        }
        let health = self.clone();
        tokio::spawn(async move {
            // A panic in a check must not leave the run "running" for good.
            let outcome = tokio::spawn(health.clone().run(pool, store, checks, progress)).await;
            if outcome.is_err() {
                tracing::error!("a health check panicked");
                health.finish();
            }
        });
        Ok(())
    }

    async fn run(self: Arc<Self>, pool: PgPool, store: BlobStore, checks: Vec<&'static str>, progress: Arc<Progress>) {
        let ctx = Context::new(pool, store, progress);
        for id in checks {
            let info = info(id).expect("checked when started");
            if let Some(running) = self.state.lock().unwrap().running.as_mut() {
                running.current = Some(info.id);
            }
            ctx.progress.step(info.title, 0);
            let started = Instant::now();
            let (findings, error) = match run_check(id, &ctx).await {
                Ok(findings) => (findings, None),
                Err(err) => {
                    tracing::warn!("health check {id}: {err:#}");
                    (Vec::new(), Some(format!("{err:#}")))
                }
            };
            let total = findings.len();
            let result = CheckResult {
                id: info.id,
                title: info.title,
                total,
                findings: findings.into_iter().take(MAX_SHOWN).collect(),
                error,
                seconds: started.elapsed().as_secs_f64(),
            };
            if let Some(running) = self.state.lock().unwrap().running.as_mut() {
                running.finished.push(result);
            }
        }
        self.finish();
    }

    /// The run is over: its results become the last report.
    fn finish(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.take() {
            state.last = Some(Report { started_at: running.started_at, finished_at: Utc::now(), by: running.by, results: running.finished });
        }
    }
}
