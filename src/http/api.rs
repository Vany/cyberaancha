//! Panel + worker API handlers — thin: authorize, delegate, serialize.

use super::AppState;
use crate::auth::Role;
use crate::{backup, db, queue};
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// System-tab heartbeat: clocks, watermarks, queue counts. Grows with phases.
pub async fn state(State(st): State<AppState>, Extension(role): Extension<Role>) -> Response {
    let result = st
        .db
        .call(|c| {
            Ok(json!({
                "clocks": {
                    "last_gathered_at": db::meta_get(c, "last_gathered_at")?,
                    "last_processed_at": db::meta_get(c, "last_processed_at")?,
                    "last_backup_at": db::meta_get(c, "last_backup_at")?,
                    "last_backup_status": db::meta_get(c, "last_backup_status")?,
                },
                "watermarks": {
                    "oldest": db::meta_get(c, "wm_oldest")?,
                    "newest": db::meta_get(c, "wm_newest")?,
                },
                "queue": queue::counts(c)?,
            }))
        })
        .await;
    match result {
        Ok(body) => {
            let mut body = body;
            body["version"] = json!(env!("CARGO_PKG_VERSION"));
            body["role"] = json!(role);
            body["channel"] = json!(st.cfg.channel.handle);
            body["window_days"] = json!(st.cfg.harvest.window_days);
            axum::Json(body).into_response()
        }
        Err(e) => internal(e),
    }
}

#[derive(serde::Deserialize)]
pub struct EnqueueParams {
    #[serde(default = "default_direction")]
    pub direction: String,
}
fn default_direction() -> String {
    "back".into()
}

/// Admin: open a harvest wave (creates/reopens the discover task).
pub async fn harvest_enqueue(
    State(st): State<AppState>,
    Extension(role): Extension<Role>,
    Query(p): Query<EnqueueParams>,
) -> Response {
    if role != Role::Admin {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    let cfg = st.cfg.clone();
    match st.db.call(move |c| queue::enqueue_wave(c, &cfg, &p.direction)).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => bad_request(e),
    }
}

#[derive(serde::Deserialize)]
pub struct ClaimParams {
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    5
}

/// Collector: claim a batch of tasks (lease-based).
pub async fn tasks_claim(State(st): State<AppState>, Query(p): Query<ClaimParams>) -> Response {
    let cfg = st.cfg.clone();
    match st
        .db
        .call(move |c| queue::claim(c, &cfg, "collector", p.limit))
        .await
    {
        Ok(tasks) => axum::Json(json!({ "tasks": tasks })).into_response(),
        Err(e) => internal(e),
    }
}

/// Collector: submit a validated result. Rejections are 422 with the reasons —
/// the worker shows them; the server never repairs.
pub async fn task_result(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    axum::Json(result): axum::Json<Value>,
) -> Response {
    let cfg = st.cfg.clone();
    match st.db.call(move |c| queue::submit(c, &cfg, id, &result)).await {
        Ok(summary) => axum::Json(summary).into_response(),
        Err(e) => {
            tracing::warn!(task = id, error = %format!("{e:#}"), "submission rejected");
            (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response()
        }
    }
}

#[derive(serde::Deserialize)]
pub struct FailBody {
    pub error: String,
}

pub async fn task_fail(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    axum::Json(body): axum::Json<FailBody>,
) -> Response {
    match st.db.call(move |c| queue::fail(c, id, &body.error)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad_request(e),
    }
}

fn bad_request(e: anyhow::Error) -> Response {
    (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response()
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
