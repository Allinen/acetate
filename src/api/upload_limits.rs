//! Limits on what comes in through the API's uploads (music, covers, ingest from another
//! machine): a size per request, and an amount per day for the whole server (every
//! account together), so a stolen account can't fill the disks with junk. Imports on the
//! server itself (`acetate ingest`, `import`) don't come through here, and aren't limited.
//!
//! One middleware (`limit`) on every upload route: a request that says (Content-Length)
//! it's over either limit is refused before any of it is read; otherwise its bytes are
//! counted as they arrive, and the stream stops where a limit is crossed (the handler then
//! drops what it had half written, as for any broken upload). The answer is then 413 (too
//! large) or 429 (the day's amount is used up, with when it starts again).
//!
//! The day's count is kept in memory (a restart starts it again) and starts over at
//! midnight UTC.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{NaiveDate, Utc};
use futures_util::StreamExt;

use super::{AppError, AppState};

pub struct UploadLimits {
    /// Bytes per request (0: no limit).
    per_request: u64,
    /// Bytes per day, all together (0: no limit).
    per_day: u64,
    /// The day counted, and what came in on it.
    today: Mutex<(NaiveDate, u64)>,
}

/// What stopped a request (kept per request, read after the handler is done).
const FINE: u8 = 0;
const TOO_LARGE: u8 = 1;
const DAY_USED_UP: u8 = 2;

impl UploadLimits {
    pub fn new(per_request: u64, per_day: u64) -> UploadLimits {
        UploadLimits { per_request, per_day, today: Mutex::new((Utc::now().date_naive(), 0)) }
    }

    /// Bytes left today (None: no daily limit).
    fn left_today(&self) -> Option<u64> {
        if self.per_day == 0 {
            return None;
        }
        let mut today = self.today.lock().unwrap();
        let now = Utc::now().date_naive();
        if today.0 != now {
            *today = (now, 0);
        }
        Some(self.per_day.saturating_sub(today.1))
    }

    /// Counts `bytes` against today. False if they're over what's left (then they aren't
    /// counted: they're not kept).
    fn take(&self, bytes: u64) -> bool {
        if self.per_day == 0 {
            return true;
        }
        let mut today = self.today.lock().unwrap();
        let now = Utc::now().date_naive();
        if today.0 != now {
            *today = (now, 0);
        }
        if today.1 + bytes > self.per_day {
            return false;
        }
        today.1 += bytes;
        true
    }

    fn too_large(&self, got: Option<u64>) -> AppError {
        let got = got.map(|b| format!(" (this one is {})", size(b))).unwrap_or_default();
        AppError::TooLarge(format!(
            "An upload can be at most {} at a time{got}. Send the files one by one; a single file over that has to be imported on the server (acetate ingest).",
            size(self.per_request)
        ))
    }

    fn day_used_up(&self) -> AppError {
        let now = Utc::now();
        let midnight = (now.date_naive() + chrono::Days::new(1)).and_hms_opt(0, 0, 0).unwrap().and_utc();
        let wait = (midnight - now).to_std().unwrap_or_default();
        let (hours, minutes) = (wait.as_secs() / 3600, wait.as_secs() / 60 % 60);
        AppError::UploadQuota {
            message: format!(
                "Today's uploads are used up ({} a day, all accounts together, so a stolen account can't fill the disks). It starts again at midnight UTC, in {hours} h {minutes} min.",
                size(self.per_day)
            ),
            wait,
        }
    }
}

/// `12.3 MB`, `20 GB`.
fn size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{} GB", (bytes as f64 / 1e9 * 10.0).round() / 10.0)
    } else {
        format!("{} MB", (bytes as f64 / 1e6 * 10.0).round() / 10.0)
    }
}

/// The middleware on upload routes (see the module's comment).
pub async fn limit(State(state): State<AppState>, request: Request, next: Next) -> Result<Response, AppError> {
    let limits = state.upload_limits.clone();
    // Said up front: refused before reading any of it.
    let declared = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if let Some(length) = declared {
        if limits.per_request > 0 && length > limits.per_request {
            return Err(limits.too_large(Some(length)));
        }
        if limits.left_today().is_some_and(|left| length > left) {
            return Err(limits.day_used_up());
        }
    }
    // Counted as it comes in; the stream stops where a limit is crossed.
    let stopped = Arc::new(AtomicU8::new(FINE));
    let (parts, body) = request.into_parts();
    let mut seen: u64 = 0;
    let (counting, flag) = (limits.clone(), stopped.clone());
    let stream = body.into_data_stream().map(move |chunk| {
        let chunk = chunk?;
        seen += chunk.len() as u64;
        let why = if counting.per_request > 0 && seen > counting.per_request {
            TOO_LARGE
        } else if !counting.take(chunk.len() as u64) {
            DAY_USED_UP
        } else {
            return Ok(chunk);
        };
        flag.store(why, Ordering::Relaxed);
        Err(axum::Error::new(std::io::Error::other("upload limit reached")))
    });
    let response = next.run(Request::from_parts(parts, Body::from_stream(stream))).await;
    // The handler saw a broken upload; say why it broke instead.
    Ok(match stopped.load(Ordering::Relaxed) {
        TOO_LARGE => limits.too_large(None).into_response(),
        DAY_USED_UP => limits.day_used_up().into_response(),
        _ => response,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_holds_what_it_holds_then_refuses() {
        let limits = UploadLimits::new(100, 250);
        assert!(limits.take(100) && limits.take(100));
        assert_eq!(limits.left_today(), Some(50));
        assert!(!limits.take(100), "over what's left");
        assert!(limits.take(50), "a refused one isn't counted");
        assert_eq!(limits.left_today(), Some(0));

        // Yesterday's count doesn't carry over.
        limits.today.lock().unwrap().0 = Utc::now().date_naive() - chrono::Days::new(1);
        assert_eq!(limits.left_today(), Some(250));

        let unlimited = UploadLimits::new(0, 0);
        assert!(unlimited.take(u64::MAX / 2) && unlimited.left_today().is_none());
    }
}
