//! Playback: native decoding (symphonia, through rodio) and gapless queues.
//!
//! The audio device has to stay on one thread, so a dedicated thread owns it and the
//! rodio `Player`, and takes `Command`s over a channel. Everything else (the queue, which
//! track is next, downloading) is `controller`'s. For gapless playback the next track is
//! appended to the rodio player while the current one still plays.
//!
//! Every source goes through a `Tap`, which copies what's played (mixed down to mono)
//! into a shared `Scope` for the visualizer.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, Player, Sample, SampleRate, Source};

use super::stream::RemoteFile;

/// What the audio thread can be told to do.
pub enum Command {
    /// Stop whatever plays and play this file (it may still be downloading).
    Replace(RemoteFile),
    /// Play this file right after what's queued (gapless).
    Append(RemoteFile),
    Pause,
    Resume,
    /// Go on with the next appended track now.
    Skip,
    /// Jump to this point of the current track.
    Seek(Duration),
}

/// What the audio thread reports, read by the controller.
#[derive(Debug, Clone, Copy, Default)]
pub struct Status {
    /// Tracks in rodio's queue, the one playing included.
    pub queued: usize,
    pub position: Duration,
    pub paused: bool,
    /// How many `Replace`s were done: when it changes, the queue started over (so a
    /// shorter queue then isn't a track that ended).
    pub replacements: u64,
}

/// The most recent samples played (mono), for the visualizer.
#[derive(Default)]
pub struct Scope {
    pub samples: VecDeque<f32>,
    pub sample_rate: u32,
}

/// How many samples the scope keeps (enough for one FFT window).
pub const SCOPE_LEN: usize = 4096;

/// Starts the audio thread. Errors (no audio device, a file that doesn't decode) are
/// reported through `errors`.
pub fn start(
    errors: impl Fn(String) + Send + 'static,
) -> (mpsc::Sender<Command>, Arc<Mutex<Status>>, Arc<Mutex<Scope>>) {
    let (sender, receiver) = mpsc::channel::<Command>();
    let status = Arc::new(Mutex::new(Status::default()));
    let scope = Arc::new(Mutex::new(Scope::default()));
    let (shared, tap_scope) = (status.clone(), scope.clone());
    std::thread::spawn(move || {
        let device = match DeviceSinkBuilder::open_default_sink() {
            Ok(device) => device,
            Err(err) => {
                errors(format!("no audio output: {err}"));
                return;
            }
        };
        let player = Player::connect_new(device.mixer());
        let mut replacements = 0;
        loop {
            match receiver.recv_timeout(Duration::from_millis(30)) {
                Ok(command) => match command {
                    Command::Replace(file) => {
                        replacements += 1;
                        player.clear();
                        match decode(file) {
                            Ok(source) => {
                                player.append(Tap::new(source, tap_scope.clone()));
                                player.play();
                            }
                            Err(err) => errors(err),
                        }
                    }
                    Command::Append(file) => match decode(file) {
                        Ok(source) => player.append(Tap::new(source, tap_scope.clone())),
                        Err(err) => errors(err),
                    },
                    Command::Pause => player.pause(),
                    Command::Resume => player.play(),
                    Command::Skip => player.skip_one(),
                    Command::Seek(position) => {
                        if let Err(err) = player.try_seek(position) {
                            errors(format!("can't seek: {err}"));
                        }
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            *shared.lock().unwrap() = Status {
                queued: player.len(),
                position: player.get_pos(),
                paused: player.is_paused(),
                replacements,
            };
        }
    });
    (sender, status, scope)
}

/// Decodes a file as it downloads (reads wait for bytes still on their way). Gapless:
/// encoder padding (MP3, AAC) is left out.
fn decode(file: RemoteFile) -> Result<Decoder<RemoteFile>, String> {
    let length = file.len();
    Decoder::builder()
        .with_data(file)
        .with_byte_len(length)
        .with_seekable(true)
        .with_gapless(true)
        .build()
        .map_err(|err| format!("can't decode it: {err}"))
}

/// Passes a source through unchanged, copying what's played (mono) into the `Scope`.
/// Samples are collected locally and handed over in batches, so the audio callback
/// rarely touches the lock.
struct Tap<S> {
    inner: S,
    scope: Arc<Mutex<Scope>>,
    batch: Vec<f32>,
    /// Channels of the current frame added up so far.
    frame_sum: f32,
    frame_channels: u16,
}

impl<S: Source> Tap<S> {
    fn new(inner: S, scope: Arc<Mutex<Scope>>) -> Tap<S> {
        Tap { inner, scope, batch: Vec::with_capacity(512), frame_sum: 0.0, frame_channels: 0 }
    }

    fn flush(&mut self) {
        let mut scope = self.scope.lock().unwrap();
        scope.sample_rate = self.inner.sample_rate().get();
        scope.samples.extend(self.batch.drain(..));
        let excess = scope.samples.len().saturating_sub(SCOPE_LEN);
        scope.samples.drain(..excess);
    }
}

impl<S: Source> Iterator for Tap<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        self.frame_sum += sample;
        self.frame_channels += 1;
        if self.frame_channels >= self.inner.channels().get() {
            self.batch.push(self.frame_sum / self.frame_channels as f32);
            self.frame_sum = 0.0;
            self.frame_channels = 0;
            if self.batch.len() >= 512 {
                self.flush();
            }
        }
        Some(sample)
    }
}

impl<S: Source> Source for Tap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}
