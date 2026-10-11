//! Work in the background whose answer a page polls for each frame.
//!
//! `start` runs a future and `poll` gives its answer once, on the frame after it's there
//! (the window is asked to repaint for you). Starting again forgets the one before; for a
//! read (`start`) that also stops it, and so does dropping the `Task` (the page was left).
//! A write must not be stopped halfway by that, so `send` lets it finish and only forgets
//! the answer.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::api::Api;
use crate::ui::Shell;

pub struct Task<T> {
    slot: Arc<Mutex<Option<T>>>,
    /// The running read, to be stopped when it's replaced or this is dropped.
    reading: Option<JoinHandle<()>>,
    /// Started and not polled yet.
    waiting: bool,
}

impl<T> Default for Task<T> {
    fn default() -> Self {
        Task { slot: Arc::default(), reading: None, waiting: false }
    }
}

impl<T: Send + 'static> Task<T> {
    /// Runs a read. What was running is stopped.
    pub fn start(&mut self, shell: &Shell, work: impl Future<Output = T> + Send + 'static) {
        let handle = self.launch(shell, work);
        self.reading = Some(handle);
    }

    /// Like `start`, with the `Api` to use.
    pub fn start_with<F: Future<Output = T> + Send + 'static>(&mut self, shell: &Shell, work: impl FnOnce(Api) -> F) {
        self.start(shell, work(shell.api.clone()));
    }

    /// Runs a write: it finishes even if this is replaced or dropped (nobody gets the answer then).
    pub fn send(&mut self, shell: &Shell, work: impl Future<Output = T> + Send + 'static) {
        self.launch(shell, work);
    }

    /// Like `send`, with the `Api` to use.
    pub fn send_with<F: Future<Output = T> + Send + 'static>(&mut self, shell: &Shell, work: impl FnOnce(Api) -> F) {
        self.send(shell, work(shell.api.clone()));
    }

    fn launch(&mut self, shell: &Shell, work: impl Future<Output = T> + Send + 'static) -> JoinHandle<()> {
        if let Some(old) = self.reading.take() {
            old.abort();
        }
        // A fresh slot: an answer still on its way from before can't land in the new one.
        let slot: Arc<Mutex<Option<T>>> = Arc::default();
        self.slot = slot.clone();
        self.waiting = true;
        let ctx = shell.ctx.clone();
        shell.runtime.spawn(async move {
            let answer = work.await;
            *slot.lock().unwrap() = Some(answer);
            ctx.request_repaint();
        })
    }

    /// The answer, once.
    pub fn poll(&mut self) -> Option<T> {
        let answer = self.slot.lock().unwrap().take();
        if answer.is_some() {
            self.waiting = false;
            self.reading = None;
        }
        answer
    }

    /// Started and not answered yet (for a spinner).
    pub fn is_running(&self) -> bool {
        self.waiting
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let Some(old) = self.reading.take() {
            old.abort();
        }
    }
}

/// A value fetched in the background and kept, for a page that reads it every frame (and
/// asks for it again when it opens).
pub struct Fetched<T> {
    task: Task<T>,
    value: Option<T>,
}

impl<T> Default for Fetched<T> {
    fn default() -> Self {
        Fetched { task: Task::default(), value: None }
    }
}

impl<T: Send + 'static> Fetched<T> {
    /// Asks again; what's known stays until the answer comes.
    pub fn start(&mut self, shell: &Shell, work: impl Future<Output = T> + Send + 'static) {
        self.task.start(shell, work);
    }

    pub fn start_with<F: Future<Output = T> + Send + 'static>(&mut self, shell: &Shell, work: impl FnOnce(Api) -> F) {
        self.task.start_with(shell, work);
    }

    /// The value, once it's there.
    pub fn get(&mut self) -> Option<&T> {
        if let Some(answer) = self.task.poll() {
            self.value = Some(answer);
        }
        self.value.as_ref()
    }

    /// Sets it (it's known without asking).
    pub fn set(&mut self, value: T) {
        self.value = Some(value);
    }
}
