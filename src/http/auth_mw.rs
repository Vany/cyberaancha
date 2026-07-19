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

    if let Some(role) = cached(&st.basic_cache, &header_value) {
        req.extensions_mut().insert(role);
        return next.run(req).await;
    }

    let Some((user, password)) = decode_basic(&header_value) else {
        return challenge();
    };
    let Some(role) = Role::from_name(&user) else {
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
        return reject(user_name(role)).await;
    }

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
        [(header::WWW_AUTHENTICATE, r#"Basic realm="aancha""#)],
        "authentication required",
    )
        .into_response()
}

/// Wrong credentials: blunt brute force a little; governor arrives in P5.
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

async fn check_bearer(
    State(st): State<AppState>,
    req: Request,
    next: Next,
    purpose: &'static str,
) -> Response {
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
        tracing::warn!(purpose, "failed bearer attempt");
        tokio::time::sleep(Duration::from_millis(250)).await;
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    next.run(req).await
}
