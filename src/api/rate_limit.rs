//! Rate limits for the things anyone on the internet can try: logging in and opening share
//! links. Kept in memory: simple, and plenty for one server. A restart forgets them.
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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use super::{AppError, AppState};

const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
const LOGIN_FAILURES_PER_IP: usize = 20;
const LOGIN_FAILURES_PER_USERNAME: usize = 10;
const SHARE_FAILURES_PER_IP: usize = 20;
const SHARE_REQUEST_WINDOW: Duration = Duration::from_secs(60);
const SHARE_REQUESTS_PER_IP: usize = 300;

/// All rate limits, shared by every request (it lives in `AppState` behind an `Arc`).
pub struct RateLimits {
    /// Failed logins and failed share lookups.
    failures: Limiter,
    /// Every request to a share route.
    share_requests: Limiter,
}

impl RateLimits {
    pub fn new() -> Self {
        RateLimits {
            failures: Limiter::new(FAILURE_WINDOW),
            share_requests: Limiter::new(SHARE_REQUEST_WINDOW),
        }
    }

    // ---- Logins ----

    /// If `ip` or `username` has failed too often: how long until it can try again.
    pub fn check_login(&self, ip: IpAddr, username: &str) -> Option<Duration> {
        let by_ip = self.failures.check(&login_ip_key(ip), LOGIN_FAILURES_PER_IP);
        let by_user = self.failures.check(&username_key(username), LOGIN_FAILURES_PER_USERNAME);
        // If both are limited, the later one decides.
        by_ip.max(by_user)
    }

    pub fn login_failed(&self, ip: IpAddr, username: &str) {
        self.failures.record(&login_ip_key(ip));
        self.failures.record(&username_key(username));
    }

    pub fn login_succeeded(&self, username: &str) {
        self.failures.clear(&username_key(username));
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

    fn clear(&self, key: &str) {
        self.events.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn login_limit_per_username_across_ips() {
        let limits = RateLimits::new();
        for i in 0..LOGIN_FAILURES_PER_USERNAME as u8 {
            assert!(limits.check_login(ip(i), "alice").is_none());
            limits.login_failed(ip(i), "alice");
        }
        assert!(limits.check_login(ip(200), "Alice").is_some(), "case-insensitive");
        assert!(limits.check_login(ip(200), "bob").is_none(), "other users unaffected");
    }

    #[test]
    fn login_limit_per_ip_across_usernames() {
        let limits = RateLimits::new();
        for i in 0..LOGIN_FAILURES_PER_IP {
            limits.login_failed(ip(1), &format!("user{i}"));
        }
        assert!(limits.check_login(ip(1), "someone").is_some());
        assert!(limits.check_login(ip(2), "someone").is_none());
    }

    #[test]
    fn login_success_clears_username_but_not_ip() {
        let limits = RateLimits::new();
        for _ in 0..LOGIN_FAILURES_PER_IP {
            limits.login_failed(ip(1), "alice");
        }
        limits.login_succeeded("alice");
        assert!(limits.check_login(ip(2), "alice").is_none());
        assert!(limits.check_login(ip(1), "someone").is_some());
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
