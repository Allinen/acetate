//! Running an extension's program (`[fetch]`, see docs/admin/extensions.md): a request as
//! one JSON object on its standard input, its answer as one on its standard output.
//!
//! Around it, whatever the extension asks for: a time limit, how many run at once (for
//! everyone together), how many a minute for one person, and what was answered lately for
//! the same request. The program gets only `PATH`, `HOME`, `LANG` and the environment
//! variables its manifest names. It runs as the server's user and is not sandboxed: the
//! manifest's `network` says what it does, it doesn't stop it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

use acetate_extensions::Metadata;

use super::registry;
use crate::api::{AppError, AppState};
use crate::auth::CurrentUser;

const TIMEOUT: Duration = Duration::from_secs(30);
const AT_ONCE: usize = 3;
const PER_USER_PER_MINUTE: usize = 10;
const KEPT_FOR: Duration = Duration::from_secs(10 * 60);
const KEPT_UP_TO: usize = 100;

/// Why a program didn't answer.
#[derive(Debug)]
pub enum FetchError {
    /// It needs something this machine lacks (yt-dlp): 503.
    Unavailable(String),
    /// It says it can't (the video isn't readable, the JSON isn't the platform's), or it
    /// took too long: 422.
    Failed(String),
    /// Too many running or asked for: try again after this long.
    TooBusy(Duration),
    Internal(anyhow::Error),
}

impl From<FetchError> for AppError {
    fn from(err: FetchError) -> AppError {
        match err {
            FetchError::Unavailable(why) => AppError::Unavailable(why),
            FetchError::Failed(why) => AppError::Internal(crate::ingest::Rejected::Invalid(why).into()),
            FetchError::TooBusy(wait) => AppError::TooManyRequests(wait),
            FetchError::Internal(err) => AppError::Internal(err),
        }
    }
}

/// Where the programs of the extensions that come with Acetate are (one folder each):
/// `ACETATE_BUILTIN_EXTENSIONS_DIR`, else `extensions` (in the image: /app/extensions).
fn builtin_dir() -> PathBuf {
    std::env::var_os("ACETATE_BUILTIN_EXTENSIONS_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("extensions"))
}

fn semaphore(extension: &str, size: usize) -> Arc<Semaphore> {
    static SEMAPHORES: OnceLock<Mutex<HashMap<String, Arc<Semaphore>>>> = OnceLock::new();
    SEMAPHORES.get_or_init(Default::default).lock().unwrap().entry(extension.to_string()).or_insert_with(|| Arc::new(Semaphore::new(size))).clone()
}

type Recent = Vec<(String, Instant, Value)>;

fn recent() -> &'static Mutex<Recent> {
    static RECENT: OnceLock<Mutex<Recent>> = OnceLock::new();
    RECENT.get_or_init(Default::default)
}

/// Asks the program of `extension` (a platform's name) for `request`. `remember`: a key
/// for the answer, to give it again for 10 minutes without running the program.
pub async fn run(state: &AppState, user: &CurrentUser, extension: &str, request: &Value, remember: Option<&str>) -> Result<Value, FetchError> {
    let registry = registry();
    let Some(manifest) = registry.extension(extension) else {
        return Err(FetchError::Failed(format!("no extension called {extension}")));
    };
    let Some(fetch) = &manifest.fetch else {
        return Err(FetchError::Failed(format!("{extension} has no program")));
    };
    let key = remember.map(|k| format!("{extension}:{k}"));
    if let Some(key) = &key {
        let mut recent = recent().lock().unwrap();
        recent.retain(|(_, at, _)| at.elapsed() < KEPT_FOR);
        if let Some((_, _, answer)) = recent.iter().find(|(kept, _, _)| kept == key) {
            return Ok(answer.clone());
        }
    }
    if let Some(wait) = state.rate_limits.fetch_read(extension, user.id(), fetch.per_user_per_minute.unwrap_or(PER_USER_PER_MINUTE)) {
        return Err(FetchError::TooBusy(wait));
    }
    let Ok(_running) = semaphore(extension, fetch.at_once.unwrap_or(AT_ONCE)).try_acquire_owned() else {
        return Err(FetchError::TooBusy(Duration::from_secs(5)));
    };
    let dir = manifest.dir.clone().unwrap_or_else(|| builtin_dir().join(extension));
    let answer = execute(extension, fetch, &dir, request).await?;
    if let Some(key) = key {
        let mut recent = recent().lock().unwrap();
        if recent.len() >= KEPT_UP_TO {
            recent.remove(0);
        }
        recent.push((key, Instant::now(), answer.clone()));
    }
    Ok(answer)
}

/// `run`, answered as the track's metadata.
pub async fn metadata(state: &AppState, user: &CurrentUser, extension: &str, request: &Value, remember: Option<&str>) -> Result<Metadata, FetchError> {
    let answer = run(state, user, extension, request, remember).await?;
    serde_json::from_value(answer).map_err(|err| FetchError::Failed(format!("{extension}'s program answered something that isn't a track's metadata: {err}")))
}

/// Runs the program in `dir` once: no limits, no memory.
async fn execute(extension: &str, fetch: &acetate_extensions::Fetch, dir: &std::path::Path, request: &Value) -> Result<Value, FetchError> {
    let Some((program, args)) = fetch.command.split_first() else {
        return Err(FetchError::Failed(format!("{extension}'s program has no command")));
    };
    let timeout = fetch.timeout.map(Duration::from_secs).unwrap_or(TIMEOUT);
    let mut command = Command::new(program);
    command.args(args).current_dir(dir).env_clear().env("ACETATE_FETCH_TIMEOUT", timeout.as_secs().to_string());
    for name in ["PATH", "HOME", "LANG"].into_iter().map(String::from).chain(fetch.env.iter().cloned()) {
        if let Some(value) = std::env::var_os(&name) {
            command.env(name, value);
        }
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => FetchError::Unavailable(format!("{extension}'s program ({program}) isn't there: {}", dir.display())),
            _ => FetchError::Internal(anyhow::anyhow!("running {extension}'s program: {err}")),
        })?;
    let mut stdin = child.stdin.take().ok_or_else(|| FetchError::Internal(anyhow::anyhow!("no stdin")))?;
    let body = serde_json::to_vec(request).map_err(|err| FetchError::Internal(err.into()))?;
    let output = tokio::time::timeout(timeout, async {
        // (A program that answers without reading it all closes the pipe: not an error.)
        let _ = stdin.write_all(&body).await;
        drop(stdin);
        child.wait_with_output().await
    })
    .await
    .map_err(|_| FetchError::Failed(format!("{extension}'s program took longer than {} seconds", timeout.as_secs())))?
    .map_err(|err| FetchError::Internal(anyhow::anyhow!("running {extension}'s program: {err}")))?;
    let answer: Value = match serde_json::from_slice(&output.stdout) {
        Ok(answer) => answer,
        Err(err) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it printed nothing");
            return Err(FetchError::Internal(anyhow::anyhow!("{extension}'s program didn't answer with JSON ({err}): {last}")));
        }
    };
    if let Some(why) = answer.get("error").and_then(Value::as_str) {
        return Err(if answer.get("unavailable").and_then(Value::as_bool) == Some(true) { FetchError::Unavailable(why.to_string()) } else { FetchError::Failed(why.to_string()) });
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acetate_extensions::Fetch;

    fn fetch(script: &str, timeout: u64) -> (Fetch, PathBuf) {
        let dir = std::env::temp_dir().join(format!("acetate-fetch-{}-{}", std::process::id(), script.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.py"), script).unwrap();
        let fetch = Fetch { command: vec!["python3".into(), "p.py".into()], timeout: Some(timeout), at_once: None, per_user_per_minute: None, network: false, env: vec!["SHOWN_TO_IT".into()], thumbnail_hosts: Vec::new() };
        (fetch, dir)
    }

    #[tokio::test]
    async fn a_program_gets_the_request_and_only_the_environment_it_asked_for() {
        // SAFETY: only this test sets these.
        unsafe {
            std::env::set_var("SHOWN_TO_IT", "yes");
            std::env::set_var("HIDDEN_FROM_IT", "no");
        }
        let (fetch, dir) = fetch("import json, os, sys\nr = json.load(sys.stdin)\nprint(json.dumps({'got': r, 'shown': os.environ.get('SHOWN_TO_IT'), 'hidden': os.environ.get('HIDDEN_FROM_IT'), 'timeout': os.environ.get('ACETATE_FETCH_TIMEOUT')}))\n", 7);
        let answer = execute("t", &fetch, &dir, &serde_json::json!({ "op": "x" })).await.unwrap();
        assert_eq!(answer, serde_json::json!({ "got": { "op": "x" }, "shown": "yes", "hidden": null, "timeout": "7" }));
    }

    #[tokio::test]
    async fn the_programs_that_come_with_acetate_read_pasted_json() {
        let registry = crate::extensions::registry();
        let ask = |name: &'static str, input: Value| async move {
            let fetch = registry.extension(name).unwrap().fetch.clone().unwrap();
            execute(name, &fetch, &builtin_dir().join(name), &serde_json::json!({ "op": "parse", "input": input })).await
        };
        let video = ask("youtube", serde_json::json!({ "id": "dQw4w9WgXcQ", "title": "Glass Harbor", "channel": "Mira Vale", "channel_id": "UCabc", "upload_date": "20210604" })).await.unwrap();
        let video: Metadata = serde_json::from_value(video).unwrap();
        assert_eq!((video.title.as_str(), video.date.as_deref(), video.source.id.as_str()), ("Glass Harbor", Some("2021-06-04"), "dQw4w9WgXcQ"));
        let sound = ask("soundcloud", serde_json::json!({ "id": 5, "title": "T", "waveform_url": "https://w/Abc_m.json", "user": { "id": 1, "username": "Mira" } })).await.unwrap();
        let sound: Metadata = serde_json::from_value(sound).unwrap();
        assert_eq!((sound.from.as_str(), sound.source.data["song_uids"].clone()), ("SoundCloud", serde_json::json!(["Abc"])));
        assert!(matches!(ask("soundcloud", serde_json::json!({ "id": 5 })).await, Err(FetchError::Failed(why)) if why.contains("title") || why.contains("waveform")));
    }

    #[tokio::test]
    async fn what_goes_wrong_is_said_as_what_it_is() {
        let (f, dir) = fetch("import json\nprint(json.dumps({'error': 'no such video'}))\n", 5);
        assert!(matches!(execute("t", &f, &dir, &Value::Null).await, Err(FetchError::Failed(why)) if why == "no such video"));
        let (f, dir) = fetch("import json\nprint(json.dumps({'error': 'no yt-dlp', 'unavailable': True}))\n", 5);
        assert!(matches!(execute("t", &f, &dir, &Value::Null).await, Err(FetchError::Unavailable(_))));
        let (f, dir) = fetch("print('not json')\n", 5);
        assert!(matches!(execute("t", &f, &dir, &Value::Null).await, Err(FetchError::Internal(_))));
        let (f, dir) = fetch("import time\ntime.sleep(30)\n", 1);
        assert!(matches!(execute("t", &f, &dir, &Value::Null).await, Err(FetchError::Failed(why)) if why.contains("took longer")));
        let (mut f, dir) = fetch("", 1);
        f.command = vec!["no-such-program-anywhere".into()];
        assert!(matches!(execute("t", &f, &dir, &Value::Null).await, Err(FetchError::Unavailable(_))));
    }
}
