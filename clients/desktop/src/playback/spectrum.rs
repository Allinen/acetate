//! The spectrum visualizer: an FFT of the latest samples played (`player::Scope`), shaped
//! into bars on a log frequency scale, eased so they rise quickly and fall slowly.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::engine::Scope;

const WINDOW: usize = super::engine::SCOPE_LEN;

pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    /// Hann window: tapers the ends so the FFT doesn't smear.
    hann: Vec<f32>,
    /// The bars' current heights, 0 to 1.
    pub bars: Vec<f32>,
}

impl Spectrum {
    pub fn new(bars: usize) -> Spectrum {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        let hann = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (WINDOW - 1) as f32).cos())
            .collect();
        Spectrum { fft, hann, bars: vec![0.0; bars] }
    }

    /// Moves the bars toward what's playing now. `dt`: seconds since the last update.
    /// `playing` false: they fall to rest.
    pub fn update(&mut self, scope: &Scope, playing: bool, dt: f32) {
        let targets = if playing && scope.samples.len() >= WINDOW && scope.sample_rate > 0 {
            self.targets(scope)
        } else {
            vec![0.0; self.bars.len()]
        };
        for (bar, target) in self.bars.iter_mut().zip(targets) {
            if target > *bar {
                *bar += (target - *bar) * (dt * 28.0).min(1.0);
            } else {
                *bar = (*bar - dt * 1.4).max(target);
            }
        }
    }

    fn targets(&self, scope: &Scope) -> Vec<f32> {
        let start = scope.samples.len() - WINDOW;
        let mut buffer: Vec<Complex<f32>> = scope
            .samples
            .range(start..)
            .zip(&self.hann)
            .map(|(s, w)| Complex::new(s * w, 0.0))
            .collect();
        self.fft.process(&mut buffer);
        let magnitudes: Vec<f32> = buffer[..WINDOW / 2].iter().map(|c| c.norm() / WINDOW as f32).collect();

        // Bars spread evenly on a log scale from 35 Hz to 16 kHz (how pitch is heard).
        let n = self.bars.len();
        let hz_per_bin = scope.sample_rate as f32 / WINDOW as f32;
        let (low, high) = (35.0f32, 16_000.0f32.min(scope.sample_rate as f32 / 2.0));
        (0..n)
            .map(|i| {
                let from = low * (high / low).powf(i as f32 / n as f32);
                let to = low * (high / low).powf((i + 1) as f32 / n as f32);
                let a = (from / hz_per_bin) as usize;
                let b = ((to / hz_per_bin) as usize).max(a + 1);
                let peak = if b - a > 1 {
                    magnitudes[a.min(magnitudes.len() - 1)..b.min(magnitudes.len())].iter().fold(0.0f32, |m, v| m.max(*v))
                } else {
                    // Narrower than one FFT bin (the lowest bars): read between the
                    // neighbouring bins at the band's centre, so adjacent bars don't
                    // all show the same bin.
                    let at = ((from * to).sqrt() / hz_per_bin).min((magnitudes.len() - 1) as f32);
                    let (lo, frac) = (at as usize, at.fract());
                    let hi = (lo + 1).min(magnitudes.len() - 1);
                    magnitudes[lo] * (1.0 - frac) + magnitudes[hi] * frac
                };
                // Decibels, -72 dB (silence) to 0: that's the bar's height. Higher bands
                // are quieter in music, so they get a gentle lift.
                let tilt = 1.0 + 3.0 * i as f32 / n as f32;
                let db = 20.0 * (peak * tilt).max(1e-6).log10();
                ((db + 72.0) / 60.0).clamp(0.0, 1.0)
            })
            .collect()
    }
}
