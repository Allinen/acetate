//! Technical info about an audio file (codec, sample rate, duration...) via symphonia.
//!
//! This only demuxes the container; it never decodes audio, so it's fast and also
//! works for codecs symphonia can't decode (e.g. Opus).

use std::path::Path;

use anyhow::Context;
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

#[derive(Debug, Clone, Default)]
pub struct ProbeInfo {
    pub container: Option<String>,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bit_depth: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// Blocking: call from `spawn_blocking`.
pub fn probe_file(path: &Path, extension: Option<&str>) -> anyhow::Result<ProbeInfo> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }

    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("unrecognized audio format")?;

    let mut info = ProbeInfo {
        container: Some(format.format_info().short_name.to_string()),
        ..Default::default()
    };

    let Some(track) = format.default_track(TrackType::Audio) else {
        return Ok(info);
    };

    if let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) {
        info.codec = Some(codec_name(params.codec));
        info.sample_rate = params.sample_rate;
        info.channels = params.channels.as_ref().map(|c| c.count() as u32);
        info.bit_depth = params.bits_per_sample;
    }

    // Prefer the container's duration in its own time base; fall back to frames / sample rate.
    let from_time_base = match (track.time_base, track.duration) {
        (Some(tb), Some(dur)) => tb.calc_duration(dur).map(|t| t.as_millis() as u64),
        _ => None,
    };
    let from_frames = match (track.num_frames, info.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => Some(frames * 1000 / rate as u64),
        _ => None,
    };
    info.duration_ms = from_time_base.or(from_frames);

    Ok(info)
}

/// A short, stable name for a codec, e.g. "flac", "mp3", "opus".
fn codec_name(id: AudioCodecId) -> String {
    // Codecs symphonia can decode are in its registry, with a proper name.
    if let Some(decoder) = symphonia::default::get_codecs().get_audio_decoder(id) {
        return decoder.codec.info.short_name.to_string();
    }
    // Codecs it can demux but not decode.
    let name = match id {
        CODEC_ID_OPUS => "opus",
        CODEC_ID_AAC => "aac",
        CODEC_ID_MP3 => "mp3",
        CODEC_ID_FLAC => "flac",
        CODEC_ID_VORBIS => "vorbis",
        _ => return format!("unknown({id})"),
    };
    name.to_string()
}
