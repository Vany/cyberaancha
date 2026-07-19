//! Router assembly. Handlers stay thin (PROG.md); auth is middleware.

pub mod api;
pub mod auth_mw;

use crate::config::Config;
use crate::db::Db;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Arc<Config>,
    pub config_path: Arc<PathBuf>,
    pub basic_cache: auth_mw::BasicCache,
}

pub fn router(state: AppState) -> axum::Router {
    use axum::routing::get;

    let panel_api = axum::Router::new()
        .route("/api/state", get(api::state))
        .route("/api/backups", get(api::backups_list).post(api::backup_now))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_mw::basic_auth,
        ));

    axum::Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(panel_api)
        .with_state(state)
}
