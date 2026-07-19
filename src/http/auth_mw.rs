//! Basic auth for the panel: username IS the role (owner | admin).
//! Successful verifications are cached by header value so argon2 (~100 ms by
//! design) runs once per session, not per request.

use super::AppState;
use crate::auth::{self, Role};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(600);
/// Two legit entries exist; anything approaching this is junk-header abuse.
const CACHE_CAP: usize = 32;

pub type BasicCache = Arc<Mutex<HashMap<String, (Role, Instant)>>>;

// ---- brute-force lockout, keyed by client IP (nginx X-Real-IP) --------------
const LOCK_THRESHOLD: u32 = 8; // failures within the window before lockout
const LOCK_WINDOW: Duration = Duration::from_secs(60);
const LOCK_COOLDOWN: Duration = Duration::from_secs(300);
const LOCK_CAP: usize = 4096; // bound memory against spray from many IPs

pub struct Fails {
    count: u32,
    window_start: Instant,
    locked_until: Option<Instant>,
}
/// Per-IP failure tracker for both basic and bearer auth. Keying on IP (not
/// username/token) means an attacker can't lock out the legit user, only itself.
pub type Lockout = Arc<Mutex<HashMap<String, Fails>>>;

/// Best-effort client IP: nginx sets X-Real-IP; fall back to the first
/// X-Forwarded-For hop, else a single shared bucket.
fn client_ip(req: &Request) -> String {
    let h = req.headers();
    if let Some(ip) = h.get("x-real-ip").and_then(|v| v.to_str().ok()) {
        return ip.trim().to_owned();
    }
    if let Some(xff) = h.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            return first.trim().to_owned();
        }
    }
    "unknown".to_owned()
}

/// Remaining cooldown if this IP is currently locked out.
fn locked_for(lock: &Lockout, ip: &str) -> Option<Duration> {
    let mut map = lock.lock().expect("lockout poisoned");
    let f = map.get(ip)?;
    let until = f.locked_until?;
    let now = Instant::now();
    if until > now {
        Some(until - now)
    } else {
        map.remove(ip); // expired — reset this IP
        None
    }
}

fn record_failure(lock: &Lockout, ip: &str) {
    let mut map = lock.lock().expect("lockout poisoned");
    if map.len() >= LOCK_CAP {
        map.retain(|_, f| f.locked_until.map(|u| u > Instant::now()).unwrap_or(false));
    }
    let now = Instant::now();
    let f = map.entry(ip.to_owned()).or_insert(Fails { count: 0, window_start: now, locked_until: None });
    if now.duration_since(f.window_start) > LOCK_WINDOW {
        f.count = 0;
        f.window_start = now;
    }
    f.count += 1;
    if f.count >= LOCK_THRESHOLD {
        f.locked_until = Some(now + LOCK_COOLDOWN);
        tracing::warn!(ip, "auth lockout engaged");
    }
}

fn clear_failures(lock: &Lockout, ip: &str) {
    lock.lock().expect("lockout poisoned").remove(ip);
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "300")],
        "too many failed attempts; try again later",
    )
        .into_response()
}

pub async fn basic_auth(
    State(st): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let header_value = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(header_value) = header_value else {
        return challenge();
    };

    // Cache hit skips the lockout check: a valid session is not an attack.
    if let Some(role) = cached(&st.basic_cache, &header_value) {
        req.extensions_mut().insert(role);
        return next.run(req).await;
    }

    let ip = client_ip(&req);
    if locked_for(&st.lockout, &ip).is_some() {
        return too_many();
    }

    let Some((user, password)) = decode_basic(&header_value) else {
        record_failure(&st.lockout, &ip);
        return challenge();
    };
    let Some(role) = Role::from_name(&user) else {
        record_failure(&st.lockout, &ip);
        return reject(&user).await;
    };

    let ok = st
        .db
        .call(move |c| auth::verify_password(c, user_name(role), &password))
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %format!("{e:#}"), "auth check failed");
            false
        });
    if !ok {
        record_failure(&st.lockout, &ip);
        return reject(user_name(role)).await;
    }

    clear_failures(&st.lockout, &ip);
    cache_put(&st.basic_cache, header_value, role);
    req.extensions_mut().insert(role);
    next.run(req).await
}

fn user_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "owner",
        Role::Admin => "admin",
    }
}

fn decode_basic(header_value: &str) -> Option<(String, String)> {
    let b64 = header_value.strip_prefix("Basic ")?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_owned(), pass.to_owned()))
}

fn cached(cache: &BasicCache, key: &str) -> Option<Role> {
    let mut map = cache.lock().expect("cache mutex poisoned");
    match map.get(key) {
        Some((role, at)) if at.elapsed() < CACHE_TTL => Some(*role),
        Some(_) => {
            map.remove(key);
            None
        }
        None => None,
    }
}

fn cache_put(cache: &BasicCache, key: String, role: Role) {
    let mut map = cache.lock().expect("cache mutex poisoned");
    if map.len() >= CACHE_CAP {
        map.clear();
    }
    map.insert(key, (role, Instant::now()));
}

fn challenge() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, r#"Basic realm="cyberaancha""#)],
        "authentication required",
    )
        .into_response()
}

/// Wrong credentials: a small constant delay on top of the IP lockout.
async fn reject(user: &str) -> Response {
    tracing::warn!(user, "failed basic-auth attempt");
    tokio::time::sleep(Duration::from_millis(250)).await;
    challenge()
}

/// Bearer auth for the collector edge; token hashes live in the DB auth table.
pub async fn collector_auth(state: State<AppState>, req: Request, next: Next) -> Response {
    check_bearer(state, req, next, "collector").await
}

/// Bearer auth for the preparer edge (integrate + transcribe workers on the Mac).
pub async fn preparer_auth(state: State<AppState>, req: Request, next: Next) -> Response {
    check_bearer(state, req, next, "preparer").await
}

/// Bearer auth for the MCP endpoint (her Claude / ours). Token = `mcp` purpose.
pub async fn mcp_auth(state: State<AppState>, req: Request, next: Next) -> Response {
    check_bearer(state, req, next, "mcp").await
}

async fn check_bearer(
    State(st): State<AppState>,
    req: Request,
    next: Next,
    purpose: &'static str,
) -> Response {
    let ip = client_ip(&req);
    if locked_for(&st.lockout, &ip).is_some() {
        return too_many();
    }
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let Some(token) = presented else {
        return (StatusCode::UNAUTHORIZED, "bearer token required").into_response();
    };
    let ok = st
        .db
        .call(move |c| auth::verify_token(c, purpose, &token))
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %format!("{e:#}"), "token check failed");
            false
        });
    if !ok {
        record_failure(&st.lockout, &ip);
        tracing::warn!(purpose, "failed bearer attempt");
        tokio::time::sleep(Duration::from_millis(250)).await;
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    clear_failures(&st.lockout, &ip);
    next.run(req).await
}
