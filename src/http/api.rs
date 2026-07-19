//! Panel API handlers — thin: authorize, delegate, serialize.

use super::AppState;
use crate::auth::Role;
use crate::{backup, db};
use axum::Extension;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// System-tab heartbeat: clocks, watermarks, config surface. Grows with phases.
pub async fn state(State(st): State<AppState>, Extension(role): Extension<Role>) -> Response {
    let result = st
        .db
        .call(|c| {
            Ok(json!({
                "last_gathered_at": db::meta_get(c, "last_gathered_at")?,
                "last_processed_at": db::meta_get(c, "last_processed_at")?,
                "last_backup_at": db::meta_get(c, "last_backup_at")?,
                "last_backup_status": db::meta_get(c, "last_backup_status")?,
            }))
        })
        .await;
    match result {
        Ok(clocks) => axum::Json(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "role": role,
            "channel": st.cfg.channel.handle,
            "window_days": st.cfg.harvest.window_days,
            "clocks": clocks,
        }))
        .into_response(),
        Err(e) => internal(e),
    }
}

pub async fn backups_list(State(st): State<AppState>) -> Response {
    match backup::list(&st.cfg.backup.dir) {
        Ok(files) => axum::Json(json!({
            "backups": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(e) => internal(e),
    }
}

/// Immediate backup (SPEC §15). Admin-only: owner shouldn't carry ops levers.
pub async fn backup_now(
    State(st): State<AppState>,
    Extension(role): Extension<Role>,
) -> Response {
    if role != Role::Admin {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    let (cfg, path) = (st.cfg.clone(), st.config_path.clone());
    match st.db.call(move |c| backup::create(c, &cfg, &path)).await {
        Ok(file) => axum::Json(json!({ "created": file.display().to_string() })).into_response(),
        Err(e) => internal(e),
    }
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!(error = %format!("{e:#}"), "api error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
