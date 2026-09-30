//! Video files (music videos, kept as extras): what they are, read with `ffprobe`, and a
//! thumbnail (a frame), cut with `ffmpeg`. Both are optional: where they aren't installed,
//! a video is kept without them (the Docker image has them).

use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

/// File names taken for videos.
pub const EXTENSIONS: &[&str] = &["mp4", "m4v", "mkv", "webm", "mov", "avi"];

/// Whether a file's name says it's a video.
pub fn is_video_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

/// What a video file is.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VideoFacts {
    pub container: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub frame_rate: Option<f32>,
    pub video_codec: Option<String>,
    /// Its sound.
    pub audio_codec: Option<String>,
    pub sample_rate: Option<i32>,
    pub channels: Option<i16>,
    pub duration_ms: Option<i64>,
}

#[derive(Deserialize)]
struct Probed {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
    avg_frame_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<i16>,
}

#[derive(Deserialize)]
struct Format {
    format_name: Option<String>,
    duration: Option<String>,
}

/// Reads a video's facts with `ffprobe` (None: not installed, or not a video it can read).
pub async fn probe(path: &Path) -> Option<VideoFacts> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    facts_from(&output.stdout)
}

/// `ffprobe`'s JSON, as facts (None: no video stream in it).
fn facts_from(json: &[u8]) -> Option<VideoFacts> {
    let probed: Probed = serde_json::from_slice(json).ok()?;
    let video = probed.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"))?;
    let audio = probed.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"));
    Some(VideoFacts {
        container: probed.format.as_ref().and_then(|f| f.format_name.clone()),
        width: video.width,
        height: video.height,
        frame_rate: video.avg_frame_rate.as_deref().and_then(rate),
        video_codec: video.codec_name.clone(),
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
        sample_rate: audio.and_then(|a| a.sample_rate.as_deref()?.parse().ok()),
        channels: audio.and_then(|a| a.channels),
        duration_ms: probed.format.as_ref().and_then(|f| f.duration.as_deref()?.parse::<f64>().ok()).map(|s| (s * 1000.0).round() as i64),
    })
}

/// "30000/1001" → 29.97 (None: "0/0").
fn rate(text: &str) -> Option<f32> {
    let (n, d) = text.split_once('/')?;
    let (n, d): (f32, f32) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then(|| n / d)
}

/// What an extra's new file is: a video (read with `ffprobe`, a thumbnail stored in `store`),
/// or audio (as tracks are probed), or neither (only its size is known).
pub async fn examine(store: &crate::storage::BlobStore, path: &Path, name: &str) -> crate::db::extras::FileFacts {
    use crate::db::extras::FileFacts;
    if is_video_name(name)
        && let Some(v) = probe(path).await
    {
        let thumb = match thumbnail(path, v.duration_ms).await {
            Some(jpeg) => store.put_bytes(&jpeg).await.ok().map(|sha| (sha, store.default_volume().to_string())),
            None => None,
        };
        return FileFacts {
            container: v.container,
            codec: v.audio_codec,
            sample_rate: v.sample_rate,
            channels: v.channels,
            duration_ms: v.duration_ms,
            width: v.width,
            height: v.height,
            frame_rate: v.frame_rate,
            video_codec: v.video_codec,
            thumb,
            ..Default::default()
        };
    }
    let path = path.to_path_buf();
    let extension = Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    match tokio::task::spawn_blocking(move || super::probe::probe_file(&path, extension.as_deref())).await {
        Ok(Ok(p)) => FileFacts {
            container: p.container,
            codec: p.codec,
            sample_rate: p.sample_rate.and_then(|v| i32::try_from(v).ok()),
            channels: p.channels.and_then(|v| i16::try_from(v).ok()),
            bit_depth: p.bit_depth.and_then(|v| i16::try_from(v).ok()),
            duration_ms: p.duration_ms.and_then(|v| i64::try_from(v).ok()),
            ..Default::default()
        },
        _ => FileFacts::default(),
    }
}

/// A thumbnail: a JPEG frame a tenth of the way in (at most 10 s), at most 640 wide. None:
/// `ffmpeg` isn't installed, or couldn't.
pub async fn thumbnail(path: &Path, duration_ms: Option<i64>) -> Option<Vec<u8>> {
    let at = duration_ms.map(|ms| (ms as f64 / 10_000.0).min(10.0)).unwrap_or(0.0);
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", &format!("{at:.2}"), "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-vf", "scale='min(640,iw)':-2", "-f", "image2pipe", "-vcodec", "mjpeg", "-"])
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    (output.status.success() && !output.stdout.is_empty()).then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffprobe_json_as_facts() {
        let json = br#"{"streams": [
            {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "avg_frame_rate": "30000/1001"},
            {"codec_type": "audio", "codec_name": "aac", "sample_rate": "48000", "channels": 2}],
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2", "duration": "215.250000"}}"#;
        let f = facts_from(json).unwrap();
        assert_eq!((f.width, f.height, f.video_codec.as_deref(), f.audio_codec.as_deref()), (Some(1920), Some(1080), Some("h264"), Some("aac")));
        assert!((f.frame_rate.unwrap() - 29.97).abs() < 0.01);
        assert_eq!((f.sample_rate, f.channels, f.duration_ms), (Some(48000), Some(2), Some(215_250)));
        assert!(facts_from(br#"{"streams": [{"codec_type": "audio", "codec_name": "flac"}]}"#).is_none(), "no video");
        assert!(is_video_name("Clip.MKV") && !is_video_name("song.m4a"));
    }

    /// With ffmpeg installed: a video made on the spot is read and gets a thumbnail.
    #[tokio::test]
    async fn a_video_is_read_and_gets_a_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=2", "-f", "lavfi", "-i", "sine=duration=2"])
            .args(["-shortest", "-c:v", "mpeg4", "-c:a", "aac"])
            .arg(&path)
            .status()
            .await;
        if !made.is_ok_and(|s| s.success()) {
            return; // no ffmpeg here
        }
        let facts = probe(&path).await.unwrap();
        assert_eq!((facts.width, facts.height), (Some(320), Some(240)));
        assert!(facts.duration_ms.unwrap() >= 1900);
        let jpeg = thumbnail(&path, facts.duration_ms).await.unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
    }
}
