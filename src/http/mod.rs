//! Router assembly. Handlers stay thin (PROG.md); auth is middleware.
//! Collector endpoints carry CORS for youtube.com page context (SPEC §11)
//! plus the Private-Network-Access header so local-dev testing from HTTPS
//! YouTube to a loopback server passes Chrome's preflight.

pub mod api;
pub mod auth_mw;

use crate::config::Config;
use crate::db::Db;
use axum::http::{HeaderValue, Method, header};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Arc<Config>,
    pub config_path: Arc<PathBuf>,
    pub basic_cache: auth_mw::BasicCache,
}

pub fn router(state: AppState) -> axum::Router {
    use axum::routing::{get, post};

    let panel = axum::Router::new()
        .route("/api/state", get(api::state))
        .route("/api/backups", get(api::backups_list).post(api::backup_now))
        .route("/api/harvest/enqueue", post(api::harvest_enqueue))
        .route("/admin", get(admin_page))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_mw::basic_auth,
        ));

    let collector_cors = CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("https://www.youtube.com"),
            HeaderValue::from_static("https://m.youtube.com"),
        ])
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(std::time::Duration::from_secs(3600));

    let collector_api = axum::Router::new()
        .route("/api/tasks", get(api::tasks_claim))
        .route("/api/tasks/{id}/result", post(api::task_result))
        .route("/api/tasks/{id}/fail", post(api::task_fail))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_mw::collector_auth,
        ))
        .layer(collector_cors)
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("access-control-allow-private-network"),
            HeaderValue::from_static("true"),
        ));

    // The bookmarklet fetches this cross-origin from youtube.com: public code,
    // permissive CORS + the PNA header (local-dev loopback preflights).
    let collector_source = axum::Router::new()
        .route("/collector.js", get(collector_js))
        .layer(CorsLayer::permissive())
        .layer(SetResponseHeaderLayer::overriding(
            header::HeaderName::from_static("access-control-allow-private-network"),
            HeaderValue::from_static("true"),
        ));

    axum::Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(collector_source)
        .merge(panel)
        .merge(collector_api)
        .with_state(state)
}

/// The collector source is public code (it holds no secrets — the token is
/// injected by the admin page / bookmarklet config); serving it openly keeps
/// the bookmarklet a one-line fetch.
async fn collector_js() -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../../collector/collector.js"),
    )
}

async fn admin_page() -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../web/admin.html"),
    )
}
