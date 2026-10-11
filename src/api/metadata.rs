//! Metadata from elsewhere, for the track editor's "import metadata from": a YouTube
//! video's, read with `yt-dlp` (optional on the server, like ffmpeg: where it's missing,
//! this says so, and pasting a yt-dlp JSON still works). The client parses it, the same as
//! a pasted one.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use super::{AppError, AppState};
use crate::auth::CurrentUser;

/// yt-dlp gets this long.
const TIMEOUT: Duration = Duration::from_secs(60);

/// At most this many yt-dlp processes run at once, for everyone together.
const AT_ONCE: usize = 3;
static RUNNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(AT_ONCE);

/// What yt-dlp's JSON has that no one here reads, and that makes it big.
const LEFT_OUT: &[&str] = &["formats", "requested_formats", "requested_downloads", "thumbnails", "automatic_captions", "subtitles", "heatmap", "_format_sort_fields"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Youtube {
    /// A video's link (youtube.com, youtu.be, music.youtube.com) or its id.
    link: String,
}

/// `POST /api/metadata/youtube`, JSON `{"link": "…"}`: the video's metadata, as
/// `yt-dlp -J` says it (without its formats, thumbnails and captions). Only YouTube: the
/// link is read for the video's id, and yt-dlp is given that video's own link. 503 where
/// yt-dlp isn't installed; 422 when it can't read the video (with why) or takes too long;
/// 429 past 10 videos a minute per user, or while 3 are being read already.
pub async fn youtube(State(state): State<AppState>, user: CurrentUser, Json(body): Json<Youtube>) -> Result<Json<Value>, AppError> {
    let id = video_id(&body.link).ok_or_else(|| AppError::BadRequest("not a YouTube video's link or id".to_string()))?;
    Ok(Json(fetch_youtube(&state, &user, &id).await?))
}

/// The video's metadata as `yt-dlp -J` says it (see `youtube`), for the video with this id.
pub async fn fetch_youtube(state: &AppState, user: &CurrentUser, id: &str) -> Result<Value, AppError> {
    if let Some(wait) = state.rate_limits.youtube_read(user.id()) {
        return Err(AppError::TooManyRequests(wait));
    }
    let Ok(_running) = RUNNING.try_acquire() else {
        return Err(AppError::TooManyRequests(Duration::from_secs(5)));
    };
    let url = format!("https://www.youtube.com/watch?v={id}");
    let mut command = Command::new("yt-dlp");
    command.args(["-J", "--no-playlist", "--skip-download", "--no-warnings", "--ignore-no-formats-error"]);
    // A cookies.txt (Netscape format) of a logged-in browser, for "Sign in to confirm you're not a bot".
    if let Some(cookies) = std::env::var_os("ACETATE_YTDLP_COOKIES").filter(|c| !c.is_empty()) {
        command.arg("--cookies").arg(cookies);
    }
    let run = command
        .args(["--", &url])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(TIMEOUT, run).await {
        Err(_) => return Err(AppError::Internal(crate::ingest::Rejected::Invalid(format!("yt-dlp took longer than {} seconds", TIMEOUT.as_secs())).into())),
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(AppError::Unavailable("yt-dlp isn't installed on the server: paste the video's yt-dlp JSON instead".to_string()));
        }
        Ok(Err(err)) => return Err(AppError::Internal(anyhow::anyhow!("running yt-dlp: {err}"))),
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it failed").trim().trim_start_matches("ERROR: ");
        let hint = if why.contains("Sign in") { " (set ACETATE_YTDLP_COOKIES to a cookies.txt file from a logged-in browser)" } else { "" };
        return Err(AppError::Internal(crate::ingest::Rejected::Invalid(format!("yt-dlp couldn't read the video: {why}{hint}")).into()));
    }
    let mut json: Value = serde_json::from_slice(&output.stdout).map_err(|err| AppError::Internal(anyhow::anyhow!("yt-dlp's answer isn't JSON: {err}")))?;
    if let Some(map) = json.as_object_mut() {
        for key in LEFT_OUT {
            map.remove(*key);
        }
    }
    Ok(json)
}

/// A YouTube video's id from its link (watch?v=, youtu.be/, /shorts/, /embed/, /live/, on
/// youtube.com, its m./www./music. hosts or youtu.be) or the id itself.
pub fn video_id(link: &str) -> Option<String> {
    let link = link.trim();
    let is_id = crate::ingest::youtube::is_id;
    if is_id(link) {
        return Some(link.to_string());
    }
    let rest = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://")).unwrap_or(link);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_lowercase();
    let host = host.strip_prefix("www.").or_else(|| host.strip_prefix("m.")).or_else(|| host.strip_prefix("music.")).unwrap_or(&host);
    let candidate = match host {
        "youtu.be" => path.split(['?', '#', '/']).next().map(str::to_string),
        "youtube.com" => {
            let (route, query) = path.split_once('?').unwrap_or((path, ""));
            match route.split('/').collect::<Vec<_>>().as_slice() {
                ["watch"] => query.split('&').find_map(|kv| kv.strip_prefix("v=")).map(|v| v.split('#').next().unwrap_or(v).to_string()),
                ["shorts" | "embed" | "live" | "v", id, ..] => Some(id.to_string()),
                _ => None,
            }
        }
        _ => None,
    };
    candidate.filter(|id| is_id(id))
}

#[cfg(test)]
mod tests {
    use super::video_id;

    #[test]
    fn video_ids_from_links() {
        let id = Some("dQw4w9WgXcQ".to_string());
        assert_eq!(video_id("dQw4w9WgXcQ"), id);
        assert_eq!(video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s"), id);
        assert_eq!(video_id("https://music.youtube.com/watch?list=x&v=dQw4w9WgXcQ"), id);
        assert_eq!(video_id("http://youtu.be/dQw4w9WgXcQ?si=abc"), id);
        assert_eq!(video_id("youtube.com/shorts/dQw4w9WgXcQ"), id);
        assert_eq!(video_id("https://m.youtube.com/embed/dQw4w9WgXcQ"), id);
        assert_eq!(video_id("https://evil.example/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(video_id("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(video_id("https://www.youtube.com/@channel"), None);
    }
}
