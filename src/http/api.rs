//! Panel + worker API handlers — thin: authorize, delegate, serialize.

use super::AppState;
use crate::auth::{self, Role};
use crate::{answer, backup, db, kb, queue};
use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct QueryText {
    #[serde(default)]
    pub q: String,
}

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
            body["brand"] = json!(st.cfg.brand());
            body["owner"] = json!(st.cfg.owner_display());
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

/// Admin: fetch (minting once, then reusing) the collector token so the panel
/// can auto-fill the launcher — no copy-paste. The token is a real random value
/// (public write-endpoint stays protected), but deliberately NOT hidden: the
/// panel's users are trusted, and it's stored retrievable rather than only-hashed
/// so it can be shown. Not derived from public info (the repo is public).
pub async fn collector_token(State(st): State<AppState>, Extension(role): Extension<Role>) -> Response {
    if role != Role::Admin {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    let result = st
        .db
        .call(|c| {
            if let Some(t) = db::meta_get(c, "collector_token")? {
                return Ok(t);
            }
            let t = auth::gen_token(c, "collector")?; // stores the verifier hash too
            db::meta_set(c, "collector_token", &t)?;
            Ok(t)
        })
        .await;
    match result {
        Ok(token) => axum::Json(json!({ "token": token })).into_response(),
        Err(e) => internal(e),
    }
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

fn unprocessable(task: i64, e: anyhow::Error) -> Response {
    tracing::warn!(task, error = %format!("{e:#}"), "submission rejected");
    (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response()
}

// ---- preparer / KB (P4) ----------------------------------------------------

/// Admin: enqueue integrate tasks for harvested-but-unintegrated videos.
pub async fn process_enqueue(
    State(st): State<AppState>,
    Extension(role): Extension<Role>,
) -> Response {
    if role != Role::Admin {
        return (StatusCode::FORBIDDEN, "admin only").into_response();
    }
    match st.db.call(|c| queue::enqueue_integrate(c)).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => internal(e),
    }
}

/// Preparer: claim the one active integrate task, with the full video bundle.
pub async fn prep_claim(State(st): State<AppState>) -> Response {
    match st.db.call(|c| queue::claim_integrate(c)).await {
        Ok(Some(task)) => axum::Json(json!({ "task": task })).into_response(),
        Ok(None) => axum::Json(json!({ "task": Value::Null })).into_response(),
        Err(e) => internal(e),
    }
}

/// Preparer: submit an integrate result; rebuild the index if the KB changed.
pub async fn prep_result(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    axum::Json(result): axum::Json<Value>,
) -> Response {
    let outcome = st.db.call(move |c| queue::submit_integrate(c, id, &result)).await;
    match outcome {
        Ok(o) => {
            if o.reindex {
                if let Err(e) = super::reindex(&st).await {
                    return internal(e);
                }
            }
            axum::Json(o.summary).into_response()
        }
        Err(e) => unprocessable(id, e),
    }
}

/// Preparer: search existing articles to decide create-vs-merge.
pub async fn prep_search(State(st): State<AppState>, Query(p): Query<QueryText>) -> Response {
    articles_search(State(st), Query(p)).await
}

pub async fn transcribe_claim(State(st): State<AppState>) -> Response {
    match st.db.call(|c| queue::claim_transcribe(c)).await {
        Ok(v) => axum::Json(json!({ "task": v })).into_response(),
        Err(e) => internal(e),
    }
}

pub async fn transcribe_result(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    axum::Json(result): axum::Json<Value>,
) -> Response {
    match st.db.call(move |c| queue::submit_transcribe(c, id, &result)).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => unprocessable(id, e),
    }
}

/// Panel/test tab and (later) the TG bot share this exact answer engine.
pub async fn test_query(State(st): State<AppState>, axum::Json(body): axum::Json<QueryText>) -> Response {
    if body.q.trim().is_empty() {
        return bad_request(anyhow::anyhow!("empty query"));
    }
    let idx = st.index.clone();
    let cfg = st.cfg.clone();
    match st.db.call(move |c| answer::answer(c, &idx, &cfg.owner, &body.q)).await {
        Ok(a) => axum::Json(a).into_response(),
        Err(e) => internal(e),
    }
}

/// Search articles; returns slug + title + score for the panel and preparer.
pub async fn articles_search(State(st): State<AppState>, Query(p): Query<QueryText>) -> Response {
    if p.q.trim().is_empty() {
        return axum::Json(json!({ "results": [] })).into_response();
    }
    let idx = st.index.clone();
    let hits = match idx.search(&p.q, 20) {
        Ok(h) => h,
        Err(e) => return internal(e),
    };
    let result = st
        .db
        .call(move |c| {
            let mut out = vec![];
            for h in hits {
                let title: Option<String> = c
                    .query_row("SELECT title FROM articles WHERE slug = ?1", [&h.slug], |r| r.get(0))
                    .ok();
                out.push(json!({ "slug": h.slug, "title": title, "score": h.score }));
            }
            Ok(Value::Array(out))
        })
        .await;
    match result {
        Ok(results) => axum::Json(json!({ "results": results })).into_response(),
        Err(e) => internal(e),
    }
}

pub async fn article_get(State(st): State<AppState>, Path(slug): Path<String>) -> Response {
    match st.db.call(move |c| kb::get_article(c, &slug)).await {
        Ok(Some(a)) => axum::Json(a).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such article").into_response(),
        Err(e) => internal(e),
    }
}

/// Owner inline-edit (SPEC §10): the professor's edit is authoritative. Body is
/// a full ArticleInput; path slug must match. Reindexes on success.
pub async fn article_put(
    State(st): State<AppState>,
    Extension(role): Extension<Role>,
    Path(slug): Path<String>,
    axum::Json(input): axum::Json<kb::ArticleInput>,
) -> Response {
    if role != Role::Owner && role != Role::Admin {
        return (StatusCode::FORBIDDEN, "owner or admin only").into_response();
    }
    if input.slug != slug {
        return bad_request(anyhow::anyhow!("body slug {:?} != path {:?}", input.slug, slug));
    }
    if let Err(e) = st.db.call(move |c| kb::upsert_article(c, &input)).await {
        return bad_request(e);
    }
    match super::reindex(&st).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal(e),
    }
}

/// Owner/admin: delete an article (child rows cascade), then reindex.
pub async fn article_delete(
    State(st): State<AppState>,
    Extension(role): Extension<Role>,
    Path(slug): Path<String>,
) -> Response {
    if role != Role::Owner && role != Role::Admin {
        return (StatusCode::FORBIDDEN, "owner or admin only").into_response();
    }
    let result = st
        .db
        .call(move |c| {
            let n = c.execute("DELETE FROM articles WHERE slug = ?1", [&slug])?;
            Ok(n)
        })
        .await;
    match result {
        Ok(0) => (StatusCode::NOT_FOUND, "no such article").into_response(),
        Ok(_) => match super::reindex(&st).await {
            Ok(_) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => internal(e),
        },
        Err(e) => internal(e),
    }
}

/// Sources tab: video inventory with per-stage harvest/process status.
pub async fn videos_list(State(st): State<AppState>) -> Response {
    let result = st
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT yt_id, kind, title, approx_published, published_at, duration_s,
                        meta_done, captions_state, comments_state, chat_state, integrated
                 FROM videos ORDER BY COALESCE(published_at, approx_published) DESC LIMIT 1000",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "yt_id": r.get::<_, String>(0)?,
                        "kind": r.get::<_, String>(1)?,
                        "title": r.get::<_, String>(2)?,
                        "published": r.get::<_, Option<String>>(4)?.or(r.get::<_, Option<String>>(3)?),
                        "duration_s": r.get::<_, Option<i64>>(5)?,
                        "meta_done": r.get::<_, i64>(6)? == 1,
                        "captions": r.get::<_, String>(7)?,
                        "comments": r.get::<_, String>(8)?,
                        "chat": r.get::<_, String>(9)?,
                        "integrated": r.get::<_, i64>(10)? == 1,
                    }))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        })
        .await;
    match result {
        Ok(videos) => axum::Json(json!({ "videos": videos })).into_response(),
        Err(e) => internal(e),
    }
}

pub async fn questions_list(State(st): State<AppState>) -> Response {
    let result = st
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, article_slug, context, question, status, created_at
                 FROM questions WHERE status = 'open' ORDER BY id DESC LIMIT 500",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(json!({
                        "id": r.get::<_, i64>(0)?,
                        "article_slug": r.get::<_, Option<String>>(1)?,
                        "context": r.get::<_, String>(2)?,
                        "question": r.get::<_, String>(3)?,
                        "status": r.get::<_, String>(4)?,
                        "created_at": r.get::<_, String>(5)?,
                    }))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        })
        .await;
    match result {
        Ok(questions) => axum::Json(json!({ "questions": questions })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct AnswerBody {
    pub answer: String,
}

/// The professor answers a question — becomes a top-authority fact next integrate.
pub async fn question_answer(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    axum::Json(body): axum::Json<AnswerBody>,
) -> Response {
    if body.answer.trim().is_empty() {
        return bad_request(anyhow::anyhow!("empty answer"));
    }
    let result = st
        .db
        .call(move |c| {
            let n = c.execute(
                "UPDATE questions SET answer = ?2, status = 'answered', answered_at = ?3
                 WHERE id = ?1 AND status = 'open'",
                rusqlite::params![id, body.answer, jiff::Timestamp::now().to_string()],
            )?;
            if n == 0 {
                anyhow::bail!("no open question {id}");
            }
            Ok(())
        })
        .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => bad_request(e),
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
