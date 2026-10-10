//! Opus files (Ogg Opus, or Opus in Matroska/WebM). Symphonia (what rodio decodes with)
//! reads these containers and hands out their packets, but has no Opus decoder, so rodio's
//! `Decoder` can't play them. Here symphonia finds the packets and `rusty_opus` (pure Rust,
//! no C library) turns them into samples; `OpusSource` is what the player plays.
//!
//! Opus is always decoded at 48 kHz. The encoder's first samples aren't part of the sound
//! (the header's "pre-skip", 312 of them usually), nor is what's past the end of the last
//! page; both are dropped here, so tracks are gapless. Symphonia marks the end on the last
//! packet (`trim_end`), but not the start: the track says how long the delay is (`delay`,
//! after `start_ts`; Matroska has it taken off the timestamps already), so the start is
//! left out here, by timestamp. The header's output gain is applied.

use std::num::NonZero;
use std::time::Duration;

use rusty_opus::multistream::OpusMSDecoder;
use rusty_opus::OpusDecoder;
use rodio::source::SeekError;
use rodio::{ChannelCount, Sample, SampleRate, Source};
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Timestamp;

const RATE: u32 = 48_000;
/// The longest an Opus packet can be: 120 ms.
const MAX_FRAMES: usize = RATE as usize * 120 / 1000;
/// After a jump, decoding starts this much earlier, and what's decoded before the point
/// asked for is dropped: the decoder needs it to sound right again (RFC 7845, 4.6).
const PREROLL: Duration = Duration::from_millis(80);

pub struct OpusSource {
    format: Box<dyn FormatReader>,
    track_id: u32,
    decoder: Decoder,
    head: Head,
    channels: ChannelCount,
    /// The header's output gain, as an amplitude factor.
    gain: f32,
    /// Decoded samples (interleaved) not played yet: `buffer[at..]`.
    buffer: Vec<f32>,
    at: usize,
    /// A packet's worth of samples, decoded into.
    scratch: Vec<f32>,
    /// Where the sound starts, as a timestamp (at 48 kHz, per channel).
    origin: i64,
    /// Samples before this timestamp are dropped: the pre-skip, and after a jump what's
    /// before the point asked for.
    skip_until: i64,
    duration: Option<Duration>,
}

impl OpusSource {
    /// Opens the file if it's Opus (Err otherwise: not a container symphonia knows, or
    /// not Opus in it).
    pub fn new(source: Box<dyn MediaSource>) -> Result<OpusSource, String> {
        let stream = MediaSourceStream::new(source, Default::default());
        let format = symphonia::default::get_probe()
            .probe(&Hint::new(), stream, FormatOptions::default(), MetadataOptions::default())
            .map_err(|err| err.to_string())?;
        let (track, params) = format
            .tracks()
            .iter()
            .find_map(|t| match &t.codec_params {
                Some(CodecParameters::Audio(p)) if p.codec == CODEC_ID_OPUS => Some((t, p)),
                _ => None,
            })
            .ok_or("not Opus")?;
        let head = Head::parse(params.extra_data.as_deref().unwrap_or_default())?;
        let decoder = Decoder::new(&head)?;
        let channels = NonZero::new(head.channels as u16).ok_or("Opus: no channels")?;
        let duration = track.duration.map(|d| Duration::from_secs_f64(d.get().saturating_sub(track.delay.unwrap_or(0).into()) as f64 / RATE as f64));
        let origin = track.start_ts.get() + i64::from(track.delay.unwrap_or(0));
        Ok(OpusSource {
            origin,
            track_id: track.id,
            format,
            decoder,
            channels,
            gain: 10f32.powf(head.gain_db / 20.0),
            head,
            buffer: Vec::new(),
            at: 0,
            scratch: vec![0.0; MAX_FRAMES * channels.get() as usize],
            skip_until: origin,
            duration,
        })
    }

    /// Decodes the next packet into `buffer`. False at the end (or when the file stops
    /// making sense: what's left of it isn't played).
    fn refill(&mut self) -> bool {
        let channels = self.channels.get() as usize;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                _ => return false,
            };
            if packet.track_id != self.track_id {
                continue;
            }
            // How long it is, all of it (`dur` is without what's left out).
            let length = packet.block_dur().get() as usize;
            // A packet that doesn't decode is left out (a moment of silence, not the end).
            let Ok(frames) = self.decoder.decode(&packet.data, length.min(MAX_FRAMES), &mut self.scratch) else { continue };
            // What the container says to leave out, and what's before a jump's target.
            let start = packet.trim_start.get() as usize;
            let end = frames.saturating_sub(packet.trim_end.get() as usize);
            let early = self.skip_until.saturating_sub(packet.pts.get()).max(0) as usize;
            let start = start + early.min(end.saturating_sub(start));
            if start >= end {
                continue;
            }
            self.buffer.clear();
            self.buffer.extend(self.scratch[start * channels..end * channels].iter().map(|s| s * self.gain));
            self.at = 0;
            return true;
        }
    }
}

impl Iterator for OpusSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        if self.at >= self.buffer.len() && !self.refill() {
            return None;
        }
        self.at += 1;
        Some(self.buffer[self.at - 1])
    }
}

impl Source for OpusSource {
    fn current_span_len(&self) -> Option<usize> {
        // The same channels and rate throughout.
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        NonZero::new(RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let timestamp = |at: Duration| self.origin + (at.as_secs_f64() * RATE as f64).round() as i64;
        let to = SeekTo::Timestamp { ts: Timestamp::new(timestamp(position.saturating_sub(PREROLL))), track_id: self.track_id };
        self.format.seek(SeekMode::Accurate, to).map_err(|err| SeekError::Other(std::sync::Arc::new(err)))?;
        // A fresh decoder: what it remembers of the sound before doesn't fit anymore.
        self.decoder = Decoder::new(&self.head).map_err(|err| SeekError::Other(std::sync::Arc::new(std::io::Error::other(err))))?;
        self.buffer.clear();
        self.at = 0;
        // Decoded from a little earlier, heard from the point asked for.
        self.skip_until = timestamp(position);
        Ok(())
    }
}

/// What's needed from the Opus header ("OpusHead", RFC 7845 5.1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Head {
    channels: usize,
    /// Samples (per channel) at the start that aren't part of the sound.
    pre_skip: u64,
    /// How the channels are coded: 0 (mono or stereo, one stream) or 1 (up to 8 channels,
    /// in the order Vorbis uses).
    family: u8,
    gain_db: f32,
}

impl Head {
    fn parse(head: &[u8]) -> Result<Head, String> {
        if head.len() < 19 || &head[..8] != b"OpusHead" {
            return Err("Opus: no header".into());
        }
        let (channels, family) = (head[9] as usize, head[18]);
        let pre_skip = u16::from_le_bytes([head[10], head[11]]).into();
        // Q7.8 decibels.
        let gain_db = i16::from_le_bytes([head[16], head[17]]) as f32 / 256.0;
        match (family, channels) {
            (0, 1..=2) | (1, 1..=8) => Ok(Head { channels, pre_skip, family, gain_db }),
            _ => Err(format!("Opus: {channels} channels coded as {family}, which isn't played")),
        }
    }
}

/// One stream (mono, stereo) or several (more channels).
enum Decoder {
    /// (Boxed: it's large, and the other is a list of them.)
    One(Box<OpusDecoder>),
    Several(OpusMSDecoder),
}

impl Decoder {
    fn new(head: &Head) -> Result<Decoder, String> {
        let decoder = match head.family {
            0 => OpusDecoder::new(RATE as i32, head.channels).map(|d| Decoder::One(Box::new(d))),
            _ => OpusMSDecoder::new(RATE as i32, head.channels, head.family.into()).map(Decoder::Several),
        };
        decoder.map_err(|err| format!("Opus: {err}"))
    }

    /// Decodes a packet of `frames` samples (per channel) into `out`, interleaved.
    fn decode(&mut self, packet: &[u8], frames: usize, out: &mut [f32]) -> Result<usize, &'static str> {
        match self {
            Decoder::One(d) => d.decode(packet, frames, out).map_err(|err| err.message()),
            Decoder::Several(d) => d.decode(packet, frames, out).map_err(|err| err.message()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str) -> OpusSource {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test-audio").join(name);
        OpusSource::new(Box::new(std::fs::File::open(path).unwrap())).unwrap()
    }

    #[test]
    fn headers() {
        let mut head = b"OpusHead\x01\x02\x38\x01\x80\xbb\x00\x00\x00\x01\x00".to_vec();
        assert_eq!(Head::parse(&head).unwrap(), Head { channels: 2, pre_skip: 312, family: 0, gain_db: 1.0 });
        head[9] = 6; // 5.1 in one stream: not a thing
        assert!(Head::parse(&head).is_err());
        head[18] = 1;
        assert_eq!(Head::parse(&head).unwrap().channels, 6);
        assert!(Head::parse(b"OggS").is_err());
    }

    #[test]
    fn a_file_plays_through_and_jumps() {
        let mut source = open("no-artist.opus");
        let (channels, duration) = (source.channels().get() as usize, source.total_duration().unwrap());
        let samples: Vec<f32> = source.by_ref().collect();
        let played = samples.len() / channels;
        // As many as libopus gives (through ffmpeg): 2 s, pre-skip and end left out.
        let expected = 96_000;
        assert_eq!(played, expected);
        assert_eq!(duration, Duration::from_secs(2));
        assert!(samples.iter().any(|s| s.abs() > 0.01), "not silent");
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 2.0));

        // Jumping to the middle plays what's left from there.
        let mut source = open("no-artist.opus");
        source.try_seek(duration / 2).unwrap();
        let rest = source.by_ref().count() / channels;
        let expected = expected - (duration.as_secs_f64() / 2.0 * RATE as f64) as usize;
        assert!(rest.abs_diff(expected) <= RATE as usize / 100, "{rest} left, not {expected}");
    }
}
