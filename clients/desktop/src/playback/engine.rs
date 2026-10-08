//! Playback: native decoding (symphonia, through rodio) and gapless track changes.
//!
//! The audio device has to stay on one thread, so a dedicated thread owns it and the
//! rodio `Player`, and takes `Command`s over a channel. Everything else (the queue, which
//! track is next, downloading) is the controller's.
//!
//! Gapless: the player plays one `Chain`, a source that plays the current track and,
//! the moment it ends, goes on with whatever is in its "next" slot. The slot can be
//! filled, replaced or emptied at any time until then (rodio's own queue can't take back
//! a sound once it's queued), so turning repeat or shuffle on or off changes what comes
//! next even in a track's last second.
//!
//! Every track goes through the equalizer (`eq`) and then a `Tap`, which copies what's played (mixed down to mono)
//! into a shared `Scope` for the visualizer.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uuid::Uuid;

use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, Player, Sample, SampleRate, Source};

use super::eq::{Equalized, Equalizer, Gains};
use super::opus::OpusSource;
use super::stream::RemoteFile;

/// What the audio thread can be told to do.
pub enum Command {
    /// Stop whatever plays and play this file (it may still be downloading), from `at`;
    /// `paused`: ready, but paused (a queue restored after a restart).
    /// `fade`: the old one fades out and the new one in, very briefly (comparing two
    /// files: no click, and the new one is opened and positioned before the old one is
    /// touched, so the switch is seamless). `follow`: start where the old one is at the
    /// moment of the switch, not at `at`.
    /// Two files (with their ids) decoded side by side, in step, one heard: switching
    /// between them (`side_b`) is instant, and the same moment of both. `at`, `follow`:
    /// as in `Replace`, for when they start playing (not when only the side changes).
    Compare { a: (Uuid, RemoteFile), b: (Uuid, RemoteFile), side_b: bool, at: Duration, follow: bool },
    /// Opens two files for a `Compare` at `at`, ahead of it, so it starts at once.
    Warm { a: (Uuid, RemoteFile), b: (Uuid, RemoteFile), at: Duration },
    Replace { file: RemoteFile, paused: bool, at: Duration, fade: bool, follow: bool },
    /// What to play when the current track ends (None: nothing, stop there). Replaces
    /// whatever was there before.
    SetNext(Option<RemoteFile>),
    Pause,
    Resume,
    /// Go on with the next track now.
    Skip,
    /// Jump to this point of the current track.
    Seek(Duration),
    /// Loudness, as an amplitude factor (0: silent, 1: as the file is).
    Volume(f32),
    /// Stop, and forget what was playing and what was next (logging out).
    Stop,
    /// Play through this output device (by its name; None: the system's default). What
    /// played stops: the controller starts it again there.
    Output(Option<String>),
    /// The equalizer: on or off, and each band's gain (dB). Applies at once, to what plays too.
    Equalizer(bool, Gains),
}

/// What the audio thread reports, read by the controller.
#[derive(Debug, Clone, Copy, Default)]
pub struct Status {
    /// A track is playing (or paused), not finished.
    pub playing: bool,
    /// How far into the current track.
    pub position: Duration,
    pub paused: bool,
    /// Counts the times playback went on to the track in the "next" slot.
    pub switches: u64,
}

/// The most recent samples played (mono), for the visualizer.
#[derive(Default)]
pub struct Scope {
    pub samples: VecDeque<f32>,
    pub sample_rate: u32,
}

/// How many samples the scope keeps (enough for one FFT window).
pub const SCOPE_LEN: usize = 4096;

type Track = Box<dyn Source + Send>;

/// A fading switch (see `Command::Replace`): out, then in.
const FADE_OUT: Duration = Duration::from_millis(6);
const FADE_IN: Duration = Duration::from_millis(12);
const FADE_STEPS: u32 = 3;

/// Shared between the audio thread's commands and the `Chain` the player plays.
#[derive(Default)]
struct Control {
    next: Mutex<Option<Track>>,
    skip: AtomicBool,
    ended: AtomicBool,
    switches: AtomicU64,
    /// Samples (all channels) played of the current track, and its rate × channels,
    /// for the position.
    played: AtomicU64,
    per_second: AtomicU64,
}

impl Control {
    fn position(&self) -> Duration {
        let per_second = self.per_second.load(Relaxed).max(1);
        Duration::from_secs_f64(self.played.load(Relaxed) as f64 / per_second as f64)
    }
}

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
        let Some(mut device) = open_output(None, &errors) else { return };
        let mut player = Player::connect_new(device.mixer());
        let mut volume = 1.0;
        let mut control: Option<Arc<Control>> = None;
        let mut switches_before = 0; // switches of chains that ended
        // A fade in going on: since when (the volume goes from 0 to `volume`).
        let mut fading_in: Option<std::time::Instant> = None;
        // Two files playing side by side, one heard (see `Command::Compare`).
        let mut pair: Option<Pair_> = None;
        // Two files opened and in place, waiting for `Compare`.
        let mut warm: Option<Warm> = None;
        let equalizer = Arc::new(Equalizer::default());
        let open_track = |file: RemoteFile| -> Result<Track, String> { Ok(Box::new(Tap::new(Equalized::new(decode(file)?, equalizer.clone()), tap_scope.clone()))) };
        loop {
            // Short waits while fading in (the volume moves in small steps), else 30 ms.
            let wait = if fading_in.is_some() { Duration::from_millis(5) } else { Duration::from_millis(30) };
            match receiver.recv_timeout(wait) {
                Ok(command) => match command {
                    Command::Replace { file, paused, at, fade, follow } => {
                        pair = None;
                        // Open the new one first, while the old one plays on.
                        let opened = open_track(file);
                        replace(&mut player, &mut control, &mut switches_before, &mut fading_in, volume, opened, paused, at, fade, follow, &errors);
                    }
                    Command::Compare { a, b, side_b, at, follow } => {
                        // The same two files already play side by side: just hear the other.
                        let live = pair.as_ref().filter(|p| {
                            p.a == a.0 && p.b == b.0 && control.as_ref().is_some_and(|c| Arc::ptr_eq(c, &p.control) && c.switches.load(Relaxed) == 0 && !c.ended.load(Relaxed))
                        });
                        if let Some(p) = live {
                            p.select.store(side_b, Relaxed);
                            player.play();
                        } else {
                            pair = None;
                            // Warmed up (see `Command::Warm`) for exactly this: already open and
                            // in place.
                            let warmed = warm.take().filter(|w| w.a == a.0 && w.b == b.0 && w.at == at && !follow);
                            let was_warm = warmed.is_some();
                            let (select, opened) = match warmed {
                                Some(w) => {
                                    w.select.store(side_b, Relaxed);
                                    (w.select, Ok(w.track))
                                }
                                None => {
                                    let select = Arc::new(AtomicBool::new(side_b));
                                    let opened = match (decode(a.1), decode(b.1)) {
                                        (Ok(x), Ok(y)) => {
                                            let (x, y) = unify(x, y);
                                            Ok(Box::new(Tap::new(Equalized::new(Pair::new(x, y, select.clone()), equalizer.clone()), tap_scope.clone())) as Track)
                                        }
                                        (Err(err), _) | (_, Err(err)) => Err(err),
                                    };
                                    (select, opened)
                                }
                            };
                            let fresh = replace(&mut player, &mut control, &mut switches_before, &mut fading_in, volume, opened, false, if was_warm { Duration::ZERO } else { at }, true, follow, &errors);
                            if let Some(control) = fresh {
                                pair = Some(Pair_ { a: a.0, b: b.0, select, control });
                            }
                        }
                    }
                    Command::Warm { a, b, at } => {
                        warm = None;
                        if let (Ok(x), Ok(y)) = (decode(a.1), decode(b.1)) {
                            let (x, y) = unify(x, y);
                            let select = Arc::new(AtomicBool::new(false));
                            let mut track: Track = Box::new(Tap::new(Equalized::new(Pair::new(x, y, select.clone()), equalizer.clone()), tap_scope.clone()));
                            // (Seeking is what takes the time: the download is asked to go there.)
                            if at > Duration::ZERO {
                                let _ = track.try_seek(at);
                            }
                            warm = Some(Warm { a: a.0, b: b.0, at, select, track });
                        }
                    }
                    Command::SetNext(file) => {
                        let track = match file.map(open_track) {
                            Some(Ok(track)) => Some(track),
                            Some(Err(err)) => {
                                errors(err);
                                None
                            }
                            None => None,
                        };
                        match (&control, track) {
                            // The current track already ended (the next wasn't ready in
                            // time): play it now, as a switch.
                            (Some(c), Some(track)) if c.ended.load(Relaxed) => {
                                switches_before += c.switches.load(Relaxed) + 1;
                                let new = Arc::new(Control::default());
                                player.append(Chain::new(track, new.clone()));
                                control = Some(new);
                            }
                            (Some(c), track) => *c.next.lock().unwrap() = track,
                            (None, _) => {}
                        }
                    }
                    Command::Stop => {
                        pair = None;
                        warm = None;
                        player.clear();
                        if let Some(old) = control.take() {
                            switches_before += old.switches.load(Relaxed);
                        }
                    }
                    Command::Pause => player.pause(),
                    Command::Resume => player.play(),
                    Command::Skip => {
                        if let Some(c) = &control {
                            c.skip.store(true, Relaxed);
                        }
                    }
                    Command::Seek(position) => {
                        if let Err(err) = player.try_seek(position) {
                            errors(format!("Couldn't jump there in the track: {err}"));
                        }
                    }
                    Command::Volume(amplitude) => {
                        volume = amplitude;
                        player.set_volume(volume);
                    }
                    Command::Equalizer(enabled, gains) => equalizer.set(enabled, gains),
                    Command::Output(name) => {
                        if let Some(opened) = open_output(name.as_deref(), &errors) {
                            pair = None;
                            player.stop();
                            if let Some(old) = control.take() {
                                switches_before += old.switches.load(Relaxed);
                            }
                            device = opened;
                            player = Player::connect_new(device.mixer());
                            player.set_volume(volume);
                        }
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            if let Some(since) = fading_in {
                let done = (since.elapsed().as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
                player.set_volume(volume * done);
                if done >= 1.0 {
                    fading_in = None;
                }
            }
            let (playing, position, switches) = match &control {
                Some(c) => (!c.ended.load(Relaxed), c.position(), switches_before + c.switches.load(Relaxed)),
                None => (false, Duration::ZERO, switches_before),
            };
            *shared.lock().unwrap() = Status { playing, position, paused: player.is_paused(), switches };
        }
    });
    (sender, status, scope)
}

/// What `Command::Compare` set going: which files, which one is heard, and the chain.
struct Pair_ {
    a: Uuid,
    b: Uuid,
    select: Arc<AtomicBool>,
    control: Arc<Control>,
}

/// Two files opened and positioned, waiting (see `Command::Warm`).
struct Warm {
    a: Uuid,
    b: Uuid,
    at: Duration,
    select: Arc<AtomicBool>,
    track: Track,
}

/// Puts `opened` in place of what plays (see `Command::Replace`); the new chain's control.
#[allow(clippy::too_many_arguments)]
fn replace(
    player: &mut Player,
    control: &mut Option<Arc<Control>>,
    switches_before: &mut u64,
    fading_in: &mut Option<std::time::Instant>,
    volume: f32,
    opened: Result<Track, String>,
    paused: bool,
    at: Duration,
    fade: bool,
    follow: bool,
    errors: &impl Fn(String),
) -> Option<Arc<Control>> {
    let track = match opened {
        Ok(track) => track,
        Err(err) => {
            errors(err);
            return None;
        }
    };
    let new = Arc::new(Control::default());
    let mut chain = Chain::new(track, new.clone());
    let started = std::time::Instant::now();
    let from = match (&*control, follow) {
        (Some(old), true) if !player.empty() => old.position(),
        _ => at,
    };
    if from > Duration::ZERO {
        let _ = chain.try_seek(from);
        if let (true, false, Some(old)) = (follow, player.is_paused(), &*control) {
            // Seeking took time, in which the old one played on: seek again (quick now),
            // to where it is.
            let _ = chain.try_seek(old.position() + started.elapsed() / 4);
        }
    }
    if fade && !player.is_paused() && !player.empty() {
        for step in (0..FADE_STEPS).rev() {
            player.set_volume(volume * step as f32 / FADE_STEPS as f32);
            std::thread::sleep(FADE_OUT / FADE_STEPS);
        }
    }
    player.clear();
    player.set_volume(volume);
    *fading_in = None;
    if let Some(old) = control.take() {
        *switches_before += old.switches.load(Relaxed);
    }
    if paused {
        player.pause();
    }
    if fade && !paused {
        player.set_volume(0.0);
        *fading_in = Some(std::time::Instant::now());
    }
    player.append(chain);
    if !paused {
        player.play();
    }
    *control = Some(new.clone());
    Some(new)
}

/// Both in one format (the higher sample rate, the more channels), converting the one
/// that differs, so they can be played in step.
fn unify(x: Track, y: Track) -> (Track, Track) {
    let rate = x.sample_rate().max(y.sample_rate());
    let channels = x.channels().max(y.channels());
    let convert = |t: Track| -> Track {
        if t.sample_rate() == rate && t.channels() == channels {
            t
        } else {
            Box::new(rodio::source::UniformSourceIterator::new(t, channels, rate))
        }
    };
    (convert(x), convert(y))
}

/// Two tracks of the same format (see `unify`) played in step (each sample of both is read), one
/// heard. The switch is a short crossfade (a few ms, against a click). Ends with the
/// heard one.
struct Pair {
    a: Track,
    b: Track,
    /// The one heard: b (else a).
    select: Arc<AtomicBool>,
    /// How much of b is heard (0..=1), moving towards what `select` says.
    mix: f32,
    step: f32,
}

impl Pair {
    fn new(a: Track, b: Track, select: Arc<AtomicBool>) -> Pair {
        let mix = if select.load(Relaxed) { 1.0 } else { 0.0 };
        let samples_per_ms = a.sample_rate().get() as f32 * a.channels().get() as f32 / 1000.0;
        Pair { a, b, select, mix, step: 1.0 / (samples_per_ms * 4.0) }
    }
}

impl Iterator for Pair {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let (x, y) = (self.a.next(), self.b.next());
        let target = if self.select.load(Relaxed) { 1.0 } else { 0.0 };
        if self.mix < target {
            self.mix = (self.mix + self.step).min(target);
        } else if self.mix > target {
            self.mix = (self.mix - self.step).max(target);
        }
        let heard_ended = if target > 0.5 { y.is_none() } else { x.is_none() };
        if heard_ended && self.mix == target {
            return None;
        }
        Some(x.unwrap_or(0.0) * (1.0 - self.mix) + y.unwrap_or(0.0) * self.mix)
    }
}

impl Source for Pair {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.a.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.a.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.a.try_seek(position)?;
        self.b.try_seek(position)
    }
}

/// The names of the output devices there are (the system's default first).
pub fn outputs() -> Vec<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let host = rodio::cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.description().ok()).map(|d| d.name().to_string());
    let mut names: Vec<String> = host
        .output_devices()
        .map(|devices| devices.filter_map(|d| d.description().ok().map(|d| d.name().to_string())).collect())
        .unwrap_or_default();
    names.dedup();
    if let Some(default) = default {
        names.retain(|n| *n != default);
        names.insert(0, default);
    }
    names
}

/// Opens an output device by name (None, or one that isn't there anymore: the default).
fn open_output(name: Option<&str>, errors: &impl Fn(String)) -> Option<rodio::MixerDeviceSink> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    if let Some(name) = name {
        let host = rodio::cpal::default_host();
        let device = host
            .output_devices()
            .ok()
            .and_then(|mut devices| devices.find(|d| d.description().ok().is_some_and(|desc| desc.name() == name)));
        match device.map(DeviceSinkBuilder::from_device).map(|b| b.and_then(|b| b.open_stream())) {
            Some(Ok(sink)) => return Some(sink),
            Some(Err(err)) => errors(format!("Couldn't use the output “{name}” ({err}); using the default one.")),
            None => errors(format!("The output “{name}” isn't there anymore; using the default one.")),
        }
    }
    match DeviceSinkBuilder::open_default_sink() {
        Ok(sink) => Some(sink),
        Err(err) => {
            errors(format!("No sound output: {err}. Is a sound device connected?"));
            None
        }
    }
}

/// Plays the current track, then (gaplessly) the one in the control's "next" slot, and
/// so on. Like rodio's queue, a change of track is also a span boundary, so a different
/// sample rate or channel count is picked up; and a track that ends mid-frame is padded
/// with silence, so channels don't get swapped.
struct Chain {
    current: Track,
    control: Arc<Control>,
    /// Samples of the current span given out so far.
    in_span: usize,
    padding: usize,
    /// Skipping: the rest of the current track isn't played (after the padding).
    skipping: bool,
    /// Samples played of the current track, published every so often.
    played: u64,
}

impl Chain {
    fn new(track: Track, control: Arc<Control>) -> Chain {
        control.per_second.store(per_second(&track), Relaxed);
        control.played.store(0, Relaxed);
        Chain { current: track, control, in_span: 0, padding: 0, skipping: false, played: 0 }
    }

    /// Goes on with the next track; false if there is none.
    fn go_next(&mut self) -> bool {
        let Some(next) = self.control.next.lock().unwrap().take() else { return false };
        self.current = next;
        self.played = 0;
        self.control.played.store(0, Relaxed);
        self.control.per_second.store(per_second(&self.current), Relaxed);
        self.control.switches.fetch_add(1, Relaxed);
        true
    }
}

fn per_second(track: &Track) -> u64 {
    track.sample_rate().get() as u64 * track.channels().get() as u64
}

impl Iterator for Chain {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        loop {
            if self.padding > 0 {
                self.padding -= 1;
                return Some(0.0);
            }
            let skip = self.skipping || self.control.skip.swap(false, Relaxed);
            if !skip
                && let Some(sample) = self.current.next()
            {
                self.in_span += 1;
                self.played += 1;
                if self.played.is_multiple_of(1024) {
                    self.control.played.store(self.played, Relaxed);
                }
                return Some(sample);
            }
            // The track ended (or is skipped): finish the frame, then the next one.
            let channels = self.current.channels().get() as usize;
            let partial = self.in_span % channels;
            self.in_span = 0;
            if partial > 0 {
                self.padding = channels - partial;
                self.skipping = skip;
                continue;
            }
            self.skipping = false;
            if !self.go_next() {
                self.control.ended.store(true, Relaxed);
                return None;
            }
        }
    }
}

impl Source for Chain {
    fn current_span_len(&self) -> Option<usize> {
        // The end of a track must end a span too: when the track doesn't say how long
        // its span is, say how much of it is left (or a short, frame-aligned span).
        if let Some(len) = self.current.current_span_len() {
            return Some(len);
        }
        let (left, _) = self.current.size_hint();
        if left > 0 {
            return Some(left);
        }
        let channels = self.current.channels().get() as usize;
        Some(512usize.div_ceil(channels) * channels)
    }
    fn channels(&self) -> ChannelCount {
        self.current.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.current.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.current.try_seek(position)?;
        self.played = (position.as_secs_f64() * per_second(&self.current) as f64) as u64;
        self.control.played.store(self.played, Relaxed);
        Ok(())
    }
}

/// Decodes a file as it downloads (reads wait for bytes still on their way). Gapless:
/// encoder padding (MP3, AAC) is left out. What rodio can't decode may be Opus, which is
/// decoded on its own (see `opus`).
fn decode(file: RemoteFile) -> Result<Track, String> {
    let length = file.len();
    let again = file.clone();
    let decoded = Decoder::builder().with_data(file).with_byte_len(length).with_seekable(true).with_gapless(true).build();
    match decoded {
        Ok(decoder) => Ok(Box::new(decoder)),
        Err(err) => match OpusSource::new(Box::new(again)) {
            Ok(opus) => Ok(Box::new(opus)),
            Err(_) => Err(format!("Couldn't play this file: it doesn't decode ({err}).")),
        },
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    fn track(channels: u16, samples: &[f32]) -> Track {
        Box::new(SamplesBuffer::new(NonZero::new(channels).unwrap(), NonZero::new(48_000).unwrap(), samples.to_vec()))
    }

    #[test]
    fn a_pair_switches_in_step_without_a_gap() {
        let select = Arc::new(AtomicBool::new(false));
        let mut pair = Pair::new(track(1, &[1.0; 4000]), track(1, &[2.0; 4000]), select.clone());
        for _ in 0..1000 {
            assert_eq!(pair.next(), Some(1.0));
        }
        select.store(true, Relaxed);
        // A short crossfade, then only b, from the same moment (never silence).
        let after: Vec<f32> = (0..1000).map(|_| pair.next().unwrap()).collect();
        assert!(after.iter().all(|s| (1.0..=2.0).contains(s)));
        assert_eq!(after[999], 2.0);
        // b ends: so does the pair.
        assert_eq!(pair.by_ref().count(), 2000);
    }

    #[test]
    fn the_next_slot_can_change_until_the_last_sample() {
        let control = Arc::new(Control::default());
        let mut chain = Chain::new(track(1, &[1.0, 1.0, 1.0]), control.clone());
        *control.next.lock().unwrap() = Some(track(1, &[2.0]));
        assert_eq!(chain.next(), Some(1.0));
        assert_eq!(chain.next(), Some(1.0));
        // One sample left: the next track is changed (say, repeat was turned on).
        *control.next.lock().unwrap() = Some(track(1, &[3.0, 3.0]));
        assert_eq!(chain.next(), Some(1.0));
        assert_eq!(chain.next(), Some(3.0), "the new next track, gapless");
        assert_eq!(control.switches.load(Relaxed), 1);
        // Emptied (repeat turned off at the end of the queue): it stops after this one.
        *control.next.lock().unwrap() = None;
        assert_eq!(chain.next(), Some(3.0));
        assert_eq!(chain.next(), None);
        assert!(control.ended.load(Relaxed));
    }

    #[test]
    fn skipping_goes_on_at_once_and_a_cut_off_frame_is_padded() {
        let control = Arc::new(Control::default());
        // Stereo, but ends mid-frame (3 samples): one silent sample completes the frame.
        let mut chain = Chain::new(track(2, &[1.0, 1.0, 1.0]), control.clone());
        *control.next.lock().unwrap() = Some(track(2, &[2.0, 2.0, 2.0, 2.0]));
        let first: Vec<f32> = (0..6).map(|_| chain.next().unwrap()).collect();
        assert_eq!(first, [1.0, 1.0, 1.0, 0.0, 2.0, 2.0]);
        // A skip mid-frame: the frame is completed with silence first.
        assert_eq!(chain.next(), Some(2.0));
        *control.next.lock().unwrap() = Some(track(2, &[5.0, 5.0]));
        control.skip.store(true, Relaxed);
        assert_eq!(chain.next(), Some(0.0), "the frame's other channel");
        assert_eq!(chain.next(), Some(5.0), "then the next track");
        assert_eq!(control.switches.load(Relaxed), 2);
    }
}
