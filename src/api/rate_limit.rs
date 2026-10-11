//! Rate limits for the things anyone on the internet can try: logging in and opening share
//! links. Counted in memory, and the failures are kept in the database too (`db::state`,
//! under `rate_limits`, written at most every 2 seconds), so a restart carries on from
//! them, as the upload limits do. (The share links' requests per minute aren't kept: a
//! restart forgetting a minute of them doesn't matter.)
//!
//! **Logins** (and wrong current passwords when changing password), per 15 minutes:
//! * 20 failures per IP address: someone trying many passwords, or many usernames;
//! * 10 failures per username: someone trying one account's password from many places.
//!
//! Only failures count. A successful login clears that username's failures, but not the
//! IP's: otherwise logging in to your own account would reset the counter for guessing at
//! others. While limited, the password isn't even checked. Trade-off: someone can keep an
//! account's login blocked by failing on purpose; existing sessions keep working.
//!
//! **Share links** (`/api/share/...`), per IP address:
//! * 20 failed lookups per 15 minutes (a token that doesn't work, or asking a share for
//!   something it doesn't contain): tokens can't realistically be guessed, but this makes
//!   trying pointless;
//! * 300 requests per minute in total, working links included: plenty for playing and
//!   seeking (which sends many range requests), but caps anyone hammering a link.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use sqlx::PgPool;

use super::{AppError, AppState};

const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOGIN_FAILURES_PER_IP: usize = 20;
const LOGIN_FAILURES_PER_USERNAME: usize = 10;
const SHARE_FAILURES_PER_IP: usize = 20;
const SHARE_REQUEST_WINDOW: Duration = Duration::from_secs(60);
const SHARE_REQUESTS_PER_IP: usize = 300;
const FETCH_WINDOW: Duration = Duration::from_secs(60);

/// The name the failures are kept under (`db::state`).
const KEPT_AS: &str = "rate_limits";
/// Failures are written at most this often: a burst of them is one write.
const KEEP_EVERY: Duration = Duration::from_secs(2);

/// All rate limits, shared by every request (it lives in `AppState` behind an `Arc`).
pub struct RateLimits {
    /// Failed logins and failed share lookups.
    failures: Limiter,
    /// Every request to a share route.
    share_requests: Limiter,
    /// Runs of extensions' programs, per user and extension.
    fetch_reads: Limiter,
    /// Where the failures are kept between runs (none: not kept, e.g. in unit tests).
    pool: Option<PgPool>,
    /// A write of the failures is on its way.
    keeping: AtomicBool,
}

impl RateLimits {
    pub fn new() -> Self {
        RateLimits {
            failures: Limiter::new(FAILURE_WINDOW),
            share_requests: Limiter::new(SHARE_REQUEST_WINDOW),
            fetch_reads: Limiter::new(FETCH_WINDOW),
            pool: None,
            keeping: AtomicBool::new(false),
        }
    }

    /// Like `new`, carrying on from the failures the database has (from before a restart),
    /// and keeping them there from now on.
    pub async fn load(pool: &PgPool) -> anyhow::Result<Self> {
        let mut limits = RateLimits::new();
        let mut tx = crate::db::begin(pool, None).await?;
        if let Some(kept) = crate::db::state::get::<HashMap<String, Vec<i64>>>(&mut tx, KEPT_AS).await? {
            limits.failures.restore(kept);
        }
        limits.pool = Some(pool.clone());
        Ok(limits)
    }

    /// Writes the failures to the database in a moment (in the background: the answer
    /// doesn't wait), unless a write is on its way already.
    fn keep(self: &Arc<Self>) {
        if self.pool.is_none() || self.keeping.swap(true, Ordering::AcqRel) {
            return;
        }
        let limits = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(KEEP_EVERY).await;
            // Before reading them: a failure from now on starts another write.
            limits.keeping.store(false, Ordering::Release);
            if let Err(err) = limits.keep_now().await {
                tracing::warn!("couldn't keep the rate limits' failures: {err:#}");
            }
        });
    }

    /// Writes the failures to the database now.
    async fn keep_now(&self) -> anyhow::Result<()> {
        let Some(pool) = &self.pool else { return Ok(()) };
        let kept = self.failures.snapshot();
        let mut tx = crate::db::begin(pool, None).await?;
        crate::db::state::set(&mut tx, KEPT_AS, &kept).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Counts a run of an extension's program for `user` (`max` a minute, for each
    /// extension); if the user has used them up: how long until the next.
    pub fn fetch_read(&self, extension: &str, user: Option<uuid::Uuid>, max: usize) -> Option<Duration> {
        let key = format!("fetch:{extension}:{}", user.map(|u| u.to_string()).unwrap_or_default());
        let wait = self.fetch_reads.check(&key, max);
        if wait.is_none() {
            self.fetch_reads.record(&key);
        }
        wait
    }

    // ---- Logins ----

    /// If `ip` or `username` has failed too often: how long until it can try again.
    pub fn check_login(&self, ip: IpAddr, username: &str) -> Option<Duration> {
        let by_ip = self.failures.check(&login_ip_key(ip), LOGIN_FAILURES_PER_IP);
        let by_user = self.failures.check(&username_key(username), LOGIN_FAILURES_PER_USERNAME);
        // If both are limited, the later one decides.
        by_ip.max(by_user)
    }

    pub fn login_failed(self: &Arc<Self>, ip: IpAddr, username: &str) {
        self.failures.record(&login_ip_key(ip));
        self.failures.record(&username_key(username));
        self.keep();
    }

    pub fn login_succeeded(self: &Arc<Self>, username: &str) {
        if self.failures.clear(&username_key(username)) {
            self.keep();
        }
    }
}

fn login_ip_key(ip: IpAddr) -> String {
    format!("login-ip:{ip}")
}

/// Usernames are case-insensitive, so the limit is too.
fn username_key(username: &str) -> String {
    format!("user:{}", username.trim().to_lowercase())
}

fn share_key(ip: IpAddr) -> String {
    format!("share-ip:{ip}")
}

/// Middleware for the public share routes: applies both share limits.
pub async fn limit_shares(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let limits = &state.rate_limits;
    let key = share_key(client_ip(request.headers(), peer, state.trust_proxy));

    if let Some(wait) = limits.share_requests.check(&key, SHARE_REQUESTS_PER_IP) {
        return Err(AppError::TooManyRequests(wait));
    }
    limits.share_requests.record(&key);
    if let Some(wait) = limits.failures.check(&key, SHARE_FAILURES_PER_IP) {
        return Err(AppError::TooManyRequests(wait));
    }

    let response = next.run(request).await;
    // The share routes answer 404 for anything that doesn't work, so that's a failure.
    if response.status() == StatusCode::NOT_FOUND {
        limits.failures.record(&key);
        limits.keep();
    }
    Ok(response)
}

/// The address of whoever sent the request. Behind a reverse proxy the connection comes
/// from the proxy, so with `trust_proxy` the last `X-Forwarded-For` entry is used instead:
/// the one our proxy added. (Earlier entries are whatever the client claimed, so they
/// can't be trusted. And without a proxy, the header itself can't be trusted at all.)
pub fn client_ip(headers: &HeaderMap, peer: SocketAddr, trust_proxy: bool) -> IpAddr {
    if trust_proxy {
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .last()
            .and_then(|ip| ip.trim().parse().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.ip()
}

/// Counts events per key over a sliding window.
struct Limiter {
    window: Duration,
    /// Event times per key, oldest first.
    /// A std `Mutex` is fine here: it's never held across an `.await`.
    events: Mutex<HashMap<String, VecDeque<Instant>>>,
}

/// Above this many keys, keys with no events in the window are dropped (so someone using
/// endless random usernames or addresses can't grow the map forever).
const PRUNE_ABOVE: usize = 10_000;

impl Limiter {
    fn new(window: Duration) -> Self {
        Limiter {
            window,
            events: Mutex::new(HashMap::new()),
        }
    }

    /// Some(wait) if `key` already has `max` events in the window.
    fn check(&self, key: &str, max: usize) -> Option<Duration> {
        let now = Instant::now();
        let mut events = self.events.lock().unwrap();
        let times = events.get_mut(key)?;
        while times.front().is_some_and(|t| now - *t >= self.window) {
            times.pop_front();
        }
        if times.len() < max {
            return None;
        }
        // Free again once the oldest event that still counts leaves the window.
        let oldest = times[times.len() - max];
        Some(self.window - (now - oldest))
    }

    fn record(&self, key: &str) {
        let now = Instant::now();
        let mut events = self.events.lock().unwrap();
        if events.len() > PRUNE_ABOVE {
            let window = self.window;
            events.retain(|_, times| times.back().is_some_and(|t| now - *t < window));
        }
        events.entry(key.to_string()).or_default().push_back(now);
    }

    /// Forgets `key`'s events. True if it had any.
    fn clear(&self, key: &str) -> bool {
        self.events.lock().unwrap().remove(key).is_some()
    }

    /// Every key's events still in the window, as Unix times in milliseconds (to keep).
    fn snapshot(&self) -> HashMap<String, Vec<i64>> {
        let (now, wall) = (Instant::now(), SystemTime::now());
        let events = self.events.lock().unwrap();
        events
            .iter()
            .filter_map(|(key, times)| {
                let kept: Vec<i64> = times
                    .iter()
                    .filter(|t| now - **t < self.window)
                    .map(|t| unix_ms(wall - (now - *t)))
                    .collect();
                (!kept.is_empty()).then(|| (key.clone(), kept))
            })
            .collect()
    }

    /// Takes in events from `snapshot` (of an earlier run): those still in the window.
    fn restore(&self, kept: HashMap<String, Vec<i64>>) {
        let (now, wall) = (Instant::now(), unix_ms(SystemTime::now()));
        let mut events = self.events.lock().unwrap();
        for (key, times) in kept {
            let times: VecDeque<Instant> = times
                .into_iter()
                .filter_map(|ms| {
                    let ago = Duration::from_millis(u64::try_from(wall - ms).ok()?);
                    if ago >= self.window {
                        return None;
                    }
                    now.checked_sub(ago)
                })
                .collect();
            if !times.is_empty() {
                events.insert(key, times);
            }
        }
    }
}

fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn login_limit_per_username_across_ips() {
        let limits = Arc::new(RateLimits::new());
        for i in 0..LOGIN_FAILURES_PER_USERNAME as u8 {
            assert!(limits.check_login(ip(i), "alice").is_none());
            limits.login_failed(ip(i), "alice");
        }
        assert!(limits.check_login(ip(200), "Alice").is_some(), "case-insensitive");
        assert!(limits.check_login(ip(200), "bob").is_none(), "other users unaffected");
    }

    #[test]
    fn login_limit_per_ip_across_usernames() {
        let limits = Arc::new(RateLimits::new());
        for i in 0..LOGIN_FAILURES_PER_IP {
            limits.login_failed(ip(1), &format!("user{i}"));
        }
        assert!(limits.check_login(ip(1), "someone").is_some());
        assert!(limits.check_login(ip(2), "someone").is_none());
    }

    #[test]
    fn login_success_clears_username_but_not_ip() {
        let limits = Arc::new(RateLimits::new());
        for _ in 0..LOGIN_FAILURES_PER_IP {
            limits.login_failed(ip(1), "alice");
        }
        limits.login_succeeded("alice");
        assert!(limits.check_login(ip(2), "alice").is_none());
        assert!(limits.check_login(ip(1), "someone").is_some());
    }

    #[test]
    fn events_survive_a_snapshot_and_old_ones_dont() {
        let limiter = Limiter::new(FAILURE_WINDOW);
        limiter.record("k");
        let mut kept = limiter.snapshot();
        // One from before the window: left out when taken back in.
        kept.insert("old".into(), vec![unix_ms(SystemTime::now()) - FAILURE_WINDOW.as_millis() as i64 - 1000]);
        let restarted = Limiter::new(FAILURE_WINDOW);
        restarted.restore(kept);
        assert!(restarted.check("k", 1).is_some(), "still counts after a restart");
        assert!(restarted.check("old", 1).is_none());
        assert!(!restarted.snapshot().contains_key("old"));
    }

    /// Through the database, as a restart does.
    #[sqlx::test]
    async fn failures_are_kept_between_runs(pool: PgPool) -> anyhow::Result<()> {
        let limits = Arc::new(RateLimits::load(&pool).await?);
        for i in 0..LOGIN_FAILURES_PER_USERNAME as u8 {
            limits.login_failed(ip(i), "alice");
        }
        limits.keep_now().await?;
        let restarted = RateLimits::load(&pool).await?;
        assert!(restarted.check_login(ip(200), "alice").is_some(), "alice is still limited");
        assert!(restarted.check_login(ip(200), "bob").is_none());
        Ok(())
    }

    #[test]
    fn events_expire() {
        let limiter = Limiter::new(Duration::from_millis(50));
        limiter.record("k");
        let wait = limiter.check("k", 1).expect("limited");
        assert!(wait <= Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(60));
        assert!(limiter.check("k", 1).is_none());
    }
}
