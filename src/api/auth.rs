//! Logging in and out, finding out who makes a request, and permission checks.
//!
//! Requests without a session are made by a *guest*, who has no permissions at all: a
//! guest can only log in, or open a share link (`api::shares`).
//!
//! A session token is accepted from either:
//! * the `acetate_session` cookie (set by `POST /api/auth/login`; used by the web UI,
//!   including `<audio>`/`<img>` requests, which can't send headers), or
//! * an `Authorization: Bearer <token>` header (other clients; the token is in the login
//!   response).
//!
//! The cookie is `SameSite=Lax`: browsers don't send it with requests that other sites
//! trigger (forms, fetches), which is what stops them from acting as you (CSRF).

use anyhow::Context;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Path, Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::rate_limit::client_ip;
use super::{AppError, AppState};
use crate::auth::{self, CurrentUser, Login, Permission};
use crate::db;

const COOKIE_NAME: &str = "acetate_session";
/// Matches the session lifetime in `db::users` (the server extends the session itself;
/// this is just so the browser doesn't drop the cookie first).
const COOKIE_MAX_AGE_SECS: u32 = 30 * 24 * 60 * 60;

/// Middleware in front of every API route: finds out who is making the request and makes
/// it available to handlers (take a `CurrentUser` argument).
///
/// No session token: a guest. A token that doesn't work (expired, logged out): 401, so
/// the client knows to log in again instead of silently continuing as a guest.
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let user = match session_token(request.headers()) {
        None => CurrentUser::guest(),
        Some(token) => {
            let Some(session) =
                db::users::find_session(&state.pool, &auth::token_hash(&token)).await?
            else {
                return Err(AppError::Unauthorized("session expired or invalid, log in again"));
            };
            db::users::touch_session(&state.pool, &session).await?;
            current_user(session)
        }
    };

    // Extensions are a type-keyed map attached to the request, for passing things along.
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

/// Wraps a route so only users with at least one of `permissions` can use it; everyone
/// else gets 403. Admins pass every check.
///
/// ```ignore
/// .route("/api/upload", require(&[Permission::Upload], post(upload::upload)))
/// ```
///
/// Use this for "may this user do this kind of thing at all". Whether they may see a
/// particular track is decided by the data (row-level security), not here.
pub fn require(
    permissions: &'static [Permission],
    route: MethodRouter<AppState>,
) -> MethodRouter<AppState> {
    route.layer(middleware::from_fn(move |request: Request, next: Next| async move {
        // Set by `authenticate`, which runs first.
        let allowed = request
            .extensions()
            .get::<CurrentUser>()
            .is_some_and(|user| user.has_any(permissions));
        if !allowed {
            let is_guest = request
                .extensions()
                .get::<CurrentUser>()
                .is_none_or(|user| user.login.is_none());
            // A guest could get further by logging in, so tell them that (401).
            if is_guest {
                return Err(AppError::Unauthorized("log in first"));
            }
            return Err(AppError::Forbidden(missing_permission_message(permissions)));
        }
        Ok(next.run(request).await)
    }))
}

fn missing_permission_message(permissions: &[Permission]) -> String {
    let names: Vec<&str> = permissions.iter().map(|p| p.name()).collect();
    match names.as_slice() {
        [one] => format!("requires the `{one}` permission"),
        _ => format!("requires one of these permissions: {}", names.join(", ")),
    }
}

// Lets handlers take `user: CurrentUser` as an argument. Only works behind `authenticate`
// (which every API route is).
// (`S` is the router's state type; this works whatever it is, since it isn't used.)
impl<S: Send + Sync> FromRequestParts<S> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<CurrentUser>()
            .cloned()
            .ok_or(AppError::Unauthorized("not logged in"))
    }
}

fn current_user(session: db::users::Session) -> CurrentUser {
    CurrentUser {
        login: Some(Login {
            id: session.user_id,
            username: session.username,
            session_id: session.session_id,
        }),
        permissions: parse_permissions(session.permissions),
    }
}

fn parse_permissions(names: Vec<String>) -> Vec<Permission> {
    names
        .iter()
        // The startup check makes sure every name in the database is known.
        .filter_map(|name| name.parse().ok())
        .collect()
}

/// The token from `Authorization: Bearer ...`, or else from the session cookie.
fn session_token(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        return value.strip_prefix("Bearer ").map(|t| t.trim().to_string());
    }
    // A request can have several Cookie headers, each `name=value; name2=value2`.
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE_NAME).then(|| value.to_string())
        })
}

fn session_cookie(token: &str, secure: bool, max_age: u32) -> String {
    // HttpOnly: page scripts can't read it, so an XSS bug can't steal the session.
    let secure = if secure { "; Secure" } else { "" };
    format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    /// For clients using `Authorization: Bearer`. The web UI can ignore it (cookie).
    token: String,
    user: CurrentUser,
}

/// `POST /api/auth/login`, JSON `{"username": "...", "password": "..."}`. Sets the session
/// cookie and returns the token and user. Too many failures give 429 (see `rate_limit`).
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, AppError> {
    let ip = client_ip(&headers, peer, state.trust_proxy);
    let username = body.username.trim();
    if let Some(wait) = state.rate_limits.check_login(ip, username) {
        return Err(AppError::TooManyRequests(wait));
    }

    let login = db::users::find_login(&state.pool, username).await?;
    let hash = login.as_ref().and_then(|l| l.password_hash.clone());
    let valid = auth::verify_password_async(body.password, hash).await;
    // Same answer for "no such user" and "wrong password".
    let Some(login) = login.filter(|_| valid) else {
        state.rate_limits.login_failed(ip, username);
        return Err(AppError::Unauthorized("wrong username or password"));
    };
    state.rate_limits.login_succeeded(username);

    let token = auth::new_token();
    let token_hash = auth::token_hash(&token);
    let user_agent = headers.get(USER_AGENT).and_then(|v| v.to_str().ok());
    let ip_text = ip.to_string();
    db::users::create_session(&state.pool, login.user_id, &token_hash, user_agent, &ip_text).await?;
    let session = db::users::find_session(&state.pool, &token_hash)
        .await?
        .context("new session not found")?;

    let cookie = session_cookie(&token, state.secure_cookies, COOKIE_MAX_AGE_SECS);
    let body = LoginResponse {
        token,
        user: current_user(session),
    };
    Ok(([(SET_COOKIE, cookie)], Json(body)).into_response())
}

/// Returns the login, or 401 for guests. For routes that only make sense logged in.
fn logged_in(user: &CurrentUser) -> Result<&Login, AppError> {
    user.login.as_ref().ok_or(AppError::Unauthorized("not logged in"))
}

/// `POST /api/auth/logout`: ends the current session (other devices stay logged in).
pub async fn logout(State(state): State<AppState>, user: CurrentUser) -> Result<Response, AppError> {
    if let Some(login) = &user.login {
        db::users::delete_session(&state.pool, login.id, login.session_id).await?;
    }
    let cookie = session_cookie("", state.secure_cookies, 0);
    Ok((StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response())
}

/// `GET /api/auth/me`: who you're logged in as (no `id`/`username` for guests), and what
/// you may do.
pub async fn me(user: CurrentUser) -> Json<CurrentUser> {
    Json(user)
}

#[derive(Debug, Deserialize)]
pub struct ChangePassword {
    current_password: String,
    new_password: String,
}

/// `POST /api/auth/password`, JSON `{"current_password": "...", "new_password": "..."}`.
/// Logs out all your other sessions. Wrong current passwords count towards the same limit
/// as failed logins (a stolen session shouldn't allow guessing the password).
pub async fn change_password(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    user: CurrentUser,
    Json(body): Json<ChangePassword>,
) -> Result<StatusCode, AppError> {
    let login = logged_in(&user)?;
    let ip = client_ip(&headers, peer, state.trust_proxy);
    if let Some(wait) = state.rate_limits.check_login(ip, &login.username) {
        return Err(AppError::TooManyRequests(wait));
    }
    check_password(&body.new_password)?;

    let mut tx = state.db(&user).await?;
    let current_hash = db::users::password_hash(&mut tx, login.id).await?;
    if !auth::verify_password_async(body.current_password, current_hash).await {
        state.rate_limits.login_failed(ip, &login.username);
        return Err(AppError::BadRequest("current password is wrong".to_string()));
    }
    let new_hash = auth::hash_password_async(body.new_password).await?;
    db::users::set_password(&mut tx, login.id, &new_hash).await?;
    tx.commit().await?;

    db::users::delete_sessions_of(&state.pool, login.id, Some(login.session_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/auth/sessions`: your sessions (devices you're logged in on), newest first.
pub async fn sessions(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<Json<Vec<db::users::SessionInfo>>, AppError> {
    let login = logged_in(&user)?;
    let list = db::users::list_sessions(&state.pool, login.id, login.session_id).await?;
    Ok(Json(list))
}

/// `DELETE /api/auth/sessions/{id}`: logs out one of your sessions.
pub async fn end_session(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let login = logged_in(&user)?;
    // Only deletes it if it's yours; someone else's id is a plain 404.
    if !db::users::delete_session(&state.pool, login.id, session_id).await? {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/auth/sessions`: logs out all your sessions except this one.
pub async fn end_other_sessions(
    State(state): State<AppState>,
    user: CurrentUser,
) -> Result<StatusCode, AppError> {
    let login = logged_in(&user)?;
    db::users::delete_sessions_of(&state.pool, login.id, Some(login.session_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Password rules, for the API. (The CLI applies the same minimum length.)
pub fn check_password(password: &str) -> Result<(), AppError> {
    if password.chars().count() < auth::MIN_PASSWORD_LENGTH {
        return Err(AppError::BadRequest(format!(
            "password must be at least {} characters",
            auth::MIN_PASSWORD_LENGTH
        )));
    }
    Ok(())
}
