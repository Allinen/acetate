//! The equalizer: ten peaking filters (one per octave, 31 Hz to 16 kHz), applied to every
//! track before the visualizer sees it, so the scope shows what's heard.
//!
//! The settings live in one `Equalizer` shared by the audio thread and every track's
//! `Equalized` source; a change bumps its version and each source picks it up within a
//! sample. The gain is lowered by the largest boost, so boosting a band can't clip.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// The bands' centre frequencies (Hz).
pub const BANDS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// How far a band can be moved (dB, either way).
pub const RANGE_DB: f32 = 12.0;
/// One octave between bands.
const Q: f64 = 1.414;

/// Gains in dB, one per band.
pub type Gains = [f32; BANDS.len()];

/// What the equalizer is set to, shared between the audio thread and the tracks.
#[derive(Default)]
pub struct Equalizer {
    version: AtomicU64,
    enabled: AtomicBool,
    gains: [AtomicU32; BANDS.len()],
}

impl Equalizer {
    pub fn set(&self, enabled: bool, gains: Gains) {
        self.enabled.store(enabled, Relaxed);
        for (slot, gain) in self.gains.iter().zip(gains) {
            slot.store(gain.clamp(-RANGE_DB, RANGE_DB).to_bits(), Relaxed);
        }
        self.version.fetch_add(1, Relaxed);
    }

    fn read(&self) -> (bool, Gains) {
        let mut gains = [0.0; BANDS.len()];
        for (gain, slot) in gains.iter_mut().zip(&self.gains) {
            *gain = f32::from_bits(slot.load(Relaxed));
        }
        (self.enabled.load(Relaxed), gains)
    }
}

/// How much the whole signal is lowered (dB, 0 or less) so that the largest boost can't clip.
pub fn headroom_db(gains: &Gains) -> f32 {
    -gains.iter().cloned().fold(0.0, f32::max)
}

/// One peaking filter (RBJ cookbook), coefficients divided by a0.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    fn peaking(rate: f64, centre: f64, gain_db: f64) -> Biquad {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * centre / rate;
        let alpha = w.sin() / (2.0 * Q);
        let a0 = 1.0 + alpha / a;
        Biquad {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * w.cos() / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * w.cos() / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// A filter's memory (transposed direct form II), one per band and channel.
#[derive(Clone, Copy, Default)]
struct Memory {
    z1: f64,
    z2: f64,
}

/// Passes a source through the equalizer.
pub struct Equalized<S> {
    inner: S,
    eq: Arc<Equalizer>,
    version: u64,
    rate: u32,
    channels: usize,
    /// The filters that do something at the current settings (a band at 0 dB is skipped, so
    /// is one above the Nyquist frequency).
    filters: Vec<Biquad>,
    /// The bands `filters` are, for finding `memory`.
    used: Vec<usize>,
    /// `channels` × bands.
    memory: Vec<Memory>,
    preamp: f64,
    /// Which channel the next sample is.
    channel: usize,
}

impl<S: Source> Equalized<S> {
    pub fn new(inner: S, eq: Arc<Equalizer>) -> Equalized<S> {
        let mut source = Equalized {
            inner,
            eq,
            version: u64::MAX,
            rate: 0,
            channels: 0,
            filters: Vec::new(),
            used: Vec::new(),
            memory: Vec::new(),
            preamp: 1.0,
            channel: 0,
        };
        source.update();
        source
    }

    /// Takes the settings, and the source's format, as they are now.
    fn update(&mut self) {
        let version = self.eq.version.load(Relaxed);
        let (rate, channels) = (self.inner.sample_rate().get(), self.inner.channels().get() as usize);
        if version == self.version && rate == self.rate && channels == self.channels {
            return;
        }
        if channels != self.channels {
            self.channel = 0;
            self.memory = vec![Memory::default(); channels * BANDS.len()];
        }
        (self.version, self.rate, self.channels) = (version, rate, channels);
        let (enabled, gains) = self.eq.read();
        self.filters.clear();
        self.used.clear();
        self.preamp = 1.0;
        if enabled {
            for (band, (&centre, &gain)) in BANDS.iter().zip(&gains).enumerate() {
                if gain.abs() >= 0.05 && (centre as f64) < rate as f64 * 0.45 {
                    self.filters.push(Biquad::peaking(rate as f64, centre as f64, gain as f64));
                    self.used.push(band);
                }
            }
            self.preamp = 10f64.powf(headroom_db(&gains) as f64 / 20.0);
        }
    }
}

impl<S: Source> Iterator for Equalized<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        self.update();
        if self.filters.is_empty() {
            self.channel = (self.channel + 1) % self.channels.max(1);
            return Some(sample);
        }
        let mut x = sample as f64 * self.preamp;
        let base = self.channel * BANDS.len();
        for (filter, &band) in self.filters.iter().zip(&self.used) {
            let m = &mut self.memory[base + band];
            let y = filter.b0 * x + m.z1;
            m.z1 = filter.b1 * x - filter.a1 * y + m.z2;
            m.z2 = filter.b2 * x - filter.a2 * y;
            x = y;
        }
        self.channel = (self.channel + 1) % self.channels;
        Some(x as f32)
    }
}

impl<S: Source> Source for Equalized<S> {
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
        self.inner.try_seek(position)?;
        // What the filters remember is from somewhere else in the track.
        self.memory.fill(Memory::default());
        self.channel = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    /// A sine of `hz` at 48 kHz, mono.
    fn sine(hz: f32, seconds: f32) -> Vec<f32> {
        (0..(48_000.0 * seconds) as usize).map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / 48_000.0).sin() * 0.25).collect()
    }

    fn peak_after(eq: &Arc<Equalizer>, hz: f32) -> f32 {
        let buffer = SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(48_000).unwrap(), sine(hz, 1.0));
        let out: Vec<f32> = Equalized::new(buffer, eq.clone()).collect();
        // The second half, after the filter has settled.
        out[24_000..].iter().fold(0.0, |m: f32, s| m.max(s.abs()))
    }

    #[test]
    fn off_or_flat_leaves_the_sound_alone() {
        let eq = Arc::new(Equalizer::default());
        assert!((peak_after(&eq, 1000.0) - 0.25).abs() < 1e-3);
        eq.set(true, [0.0; 10]);
        assert!((peak_after(&eq, 1000.0) - 0.25).abs() < 1e-3);
        eq.set(false, [6.0; 10]);
        assert!((peak_after(&eq, 1000.0) - 0.25).abs() < 1e-3, "disabled ignores the gains");
    }

    #[test]
    fn a_band_boosts_its_frequency_and_the_gain_is_lowered_by_the_boost() {
        let eq = Arc::new(Equalizer::default());
        let mut gains = [0.0; 10];
        gains[5] = 6.0; // 1 kHz
        eq.set(true, gains);
        // +6 dB at the band, -6 dB overall: back to where it was.
        assert!((peak_after(&eq, 1000.0) - 0.25).abs() < 0.01);
        // Far away from the band, only the lowered gain remains: about half.
        assert!((peak_after(&eq, 31.0) - 0.125).abs() < 0.01);
        // A cut doesn't lower anything else.
        gains[5] = -6.0;
        eq.set(true, gains);
        assert!((peak_after(&eq, 1000.0) - 0.125).abs() < 0.01);
        assert!((peak_after(&eq, 31.0) - 0.25).abs() < 0.01);
    }

    #[test]
    fn stereo_channels_keep_their_own_memory() {
        let eq = Arc::new(Equalizer::default());
        let mut gains = [0.0; 10];
        gains[5] = -12.0;
        eq.set(true, gains);
        // Left 1 kHz, right silent: right must stay silent.
        let left = sine(1000.0, 0.2);
        let samples: Vec<f32> = left.iter().flat_map(|&s| [s, 0.0]).collect();
        let buffer = SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(48_000).unwrap(), samples);
        let out: Vec<f32> = Equalized::new(buffer, eq).collect();
        assert!(out.iter().skip(1).step_by(2).all(|s| s.abs() < 1e-9));
    }
}
