//! MCP endpoint (SPEC §10): the same read-mostly KB surface the panel uses,
//! exposed to Claude (hers and ours) over streamable HTTP at `/mcp`. No LLM
//! server-side — these tools just search, read, and record. Bearer-gated by the
//! `mcp` token (middleware in http/mod.rs); URL + token shown in the panel.

use crate::db::Db;
use crate::index::SearchIndex;
use crate::{kb, queue};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, Implementation, ListPromptsResult,
    PaginatedRequestParams, Prompt, PromptMessage, ProtocolVersion, Role, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool, tool_handler, tool_router};

/// The preparer playbook, exposed as the `integrate` MCP prompt so a connected
/// Claude can run the whole loop. Kept in sync with prompts/integrate.md but
/// phrased for the MCP tools (next_unprocessed_video / submit_articles).
const INTEGRATE_PROMPT: &str = r#"You are the preparer for the cyberaancha knowledge base — you turn ONE harvested video into curated KB articles. This runs at build time; the production server has no LLM, so the quality of what you write here IS the quality of every future answer.

Iron rules:
- Quote and attribute; You record what the author said, with sources. When unsure, raise a question instead of guessing.
- Store article text in the source language (Russian).
- One video at a time (serialized). Always search the KB before creating.

Loop:
1. Call next_unprocessed_video → {task_id, video, bundle}. The bundle has: video metadata; transcript (segments with t_ms timestamps); comments (is_author=1 marks the author's OWN replies — top authority); the author's chat messages. If it returns {"done":true}, stop.
2. If the transcript is null or clearly unusable (wrong language, noise), call submit_articles with result_json = {"needs_transcription":true,"articles":[]} and go to 1 — the server will re-transcribe with Whisper.
3. Find the DISTINCT topics the author actually discusses. For each, call search_articles (try the Russian term, a colloquial phrasing, and any latin/EN name) to decide merge-vs-create.
   - If a topic matches an existing article, get_article it and read EVERYTHING already known about that topic — all stances, all facts, and the whole opinion timeline. COMPARE the new material against that entire picture: does it agree, add nuance, or contradict? RECONCILE it — do not just append. Place each new statement in the dated timeline (keep older stances even when a newer one revises them — that is exactly how "переосмыслено в …" is reconstructed), and update paragraph_ru to the latest reconciled opinion. If two sources genuinely conflict and you can't resolve it, add a `contradicts` link and raise a question. Never drop existing aliases/stances/facts.
4. Write each article: slug ([a-z0-9-]); title; paragraph_ru (ONE paragraph, the bot's answer, in the author's voice, ≤~800 chars — the CURRENT reconciled opinion across all sources); story_md (full narrative, chronological); status ("published" when solid, else "draft"); aliases (CRITICAL for recall — Russian stemming is imperfect, so include inflected forms of the title, plus colloquial synonyms, common misspellings, and latin/EN drug & gene names); stances (the dated timeline — video stances carry video_id = the bundle's yt_id, offset_ms from the transcript t_ms, and occurred_at); facts (with authority: panel > comment_author > spoken > inferred); links (related|parent|contradicts).
5. Raise questions for any contradiction or gap only the author can resolve.
6. Call submit_articles with {task_id, result_json: a JSON string of {"articles":[...],"questions":[...]}}. On a validation error, read it, fix, and resubmit. Success upserts the articles, files the questions, marks the video processed, and rebuilds the search index.

Write as if the author will read every article — because they will."#;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone)]
pub struct CyberaanchaMcp {
    db: Db,
    index: Arc<SearchIndex>,
    owner_name: String,
    // Consumed by the #[tool_handler]-generated call_tool routing (verified: all
    // tools list and call correctly). The compiler can't see through the macro.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SearchParams {
    /// Free-text query in Russian or a colloquial/latin term.
    query: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SlugParams {
    /// Article slug (from search_articles).
    slug: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AnswerParams {
    /// Question id (from list_questions).
    id: i64,
    /// The professor's answer; becomes a top-authority fact next cycle.
    answer: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SubmitParams {
    /// task_id from next_unprocessed_video.
    task_id: i64,
    /// JSON string of the integrate envelope: {"articles":[...],"questions":[...]}
    /// (schemas/integrate.json). Each article: slug, title, paragraph_ru, story_md,
    /// status, aliases[], stances[], facts[], links[]. Set needs_transcription:true
    /// (with articles:[]) instead if the transcript is unusable.
    result_json: String,
}

fn oops(e: anyhow::Error) -> McpError {
    McpError::internal_error(format!("{e:#}"), None)
}

#[tool_router]
impl CyberaanchaMcp {
    pub fn new(db: Db, index: Arc<SearchIndex>, owner_name: String) -> Self {
        Self {
            db,
            index,
            owner_name,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Full-text search Prof. Baranova's knowledge base. Returns matching article slugs, titles, and BM25 scores."
    )]
    async fn search_articles(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<String, McpError> {
        let hits = self.index.search(&p.query, 20).map_err(oops)?;
        let out = self
            .db
            .call(move |c| {
                let mut results = vec![];
                for h in hits {
                    let title: Option<String> = c
                        .query_row(
                            "SELECT title FROM articles WHERE slug = ?1",
                            [&h.slug],
                            |r| r.get(0),
                        )
                        .ok();
                    results.push(json!({ "slug": h.slug, "title": title, "score": h.score }));
                }
                Ok(json!({ "results": results }))
            })
            .await
            .map_err(oops)?;
        Ok(out.to_string())
    }

    #[tool(
        description = "Get one full article by slug: title, paragraph, story, aliases, stances/citations, facts, and links."
    )]
    async fn get_article(&self, Parameters(p): Parameters<SlugParams>) -> Result<String, McpError> {
        let slug = p.slug.clone();
        let article = self
            .db
            .call(move |c| kb::get_article(c, &slug))
            .await
            .map_err(oops)?;
        match article {
            Some(a) => Ok(serde_json::to_string(&a).map_err(|e| oops(e.into()))?),
            None => Err(McpError::resource_not_found(
                format!("no article {:?}", p.slug),
                None,
            )),
        }
    }

    #[tool(
        description = "List the open questions awaiting the professor's answer (contradictions and gaps found during integration)."
    )]
    async fn list_questions(&self) -> Result<String, McpError> {
        let out = self
            .db
            .call(|c| {
                let mut stmt = c.prepare(
                    "SELECT id, article_slug, context, question, created_at FROM questions
                     WHERE status = 'open' ORDER BY id DESC LIMIT 500",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(json!({
                            "id": r.get::<_, i64>(0)?,
                            "article_slug": r.get::<_, Option<String>>(1)?,
                            "context": r.get::<_, String>(2)?,
                            "question": r.get::<_, String>(3)?,
                            "created_at": r.get::<_, String>(4)?,
                        }))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(json!({ "questions": rows }))
            })
            .await
            .map_err(oops)?;
        Ok(out.to_string())
    }

    #[tool(
        description = "Answer an open question by id. Records the professor's answer as an authoritative fact for the next integration cycle."
    )]
    async fn answer_question(
        &self,
        Parameters(p): Parameters<AnswerParams>,
    ) -> Result<String, McpError> {
        if p.answer.trim().is_empty() {
            return Err(McpError::invalid_params("empty answer", None));
        }
        let (id, answer) = (p.id, p.answer.clone());
        let n = self
            .db
            .call(move |c| {
                Ok(c.execute(
                    "UPDATE questions SET answer = ?2, status = 'answered', answered_at = ?3
                     WHERE id = ?1 AND status = 'open'",
                    rusqlite::params![id, answer, jiff::Timestamp::now().to_string()],
                )?)
            })
            .await
            .map_err(oops)?;
        if n == 0 {
            return Err(McpError::resource_not_found(
                format!("no open question {id}"),
                None,
            ));
        }
        Ok(json!({ "answered": id }).to_string())
    }

    #[tool(
        description = "Knowledge-base statistics: counts of articles, videos, transcripts, comments, and the last gathered/processed timestamps."
    )]
    async fn kb_stats(&self) -> Result<String, McpError> {
        let out = self
            .db
            .call(|c| {
                let one = |sql: &str| -> rusqlite::Result<i64> { c.query_row(sql, [], |r| r.get(0)) };
                Ok(json!({
                    "articles": one("SELECT COUNT(*) FROM articles")?,
                    "published": one("SELECT COUNT(*) FROM articles WHERE status='published'")?,
                    "videos": one("SELECT COUNT(*) FROM videos")?,
                    "integrated": one("SELECT COUNT(*) FROM videos WHERE integrated=1")?,
                    "transcripts": one("SELECT COUNT(*) FROM transcripts")?,
                    "comments": one("SELECT COUNT(*) FROM comments")?,
                    "author_replies": one("SELECT COUNT(*) FROM comments WHERE is_author=1")?,
                    "open_questions": one("SELECT COUNT(*) FROM questions WHERE status='open'")?,
                    "last_gathered_at": crate::db::meta_get(c, "last_gathered_at")?,
                    "last_processed_at": crate::db::meta_get(c, "last_processed_at")?,
                    "unprocessed_ready": one(
                        "SELECT COUNT(*) FROM videos WHERE integrated=0 AND meta_done=1 AND transcribe_state='done'
                         AND comments_state!='pending' AND chat_state!='pending'")?,
                }))
            })
            .await
            .map_err(oops)?;
        Ok(out.to_string())
    }

    #[tool(
        description = "Claim the next un-integrated video and return its full bundle: metadata, transcript (segments with t_ms timestamps), comments (professor's replies flagged is_author), and her chat messages. Categorize it into KB articles, then call submit_articles. Returns {\"done\":true} when none remain. Serialized — one active at a time. Follow the 'integrate' MCP prompt for the method."
    )]
    async fn next_unprocessed_video(&self) -> Result<String, McpError> {
        let out = self
            .db
            .call(|c| {
                queue::enqueue_integrate(c)?; // idempotent: schedule any newly-ready videos
                queue::claim_integrate(c)
            })
            .await
            .map_err(oops)?;
        match out {
            Some(task) => Ok(json!({
                "task_id": task.id,
                "video": task.subject,
                "bundle": task.bundle,
                "next": "Search the KB (search_articles) before creating; merge into existing slugs when a topic matches. Emit inflected/colloquial/latin aliases. Then submit_articles(task_id, result_json).",
            })
            .to_string()),
            None => Ok(json!({ "done": true, "note": "no videos ready to process" }).to_string()),
        }
    }

    #[tool(
        description = "Submit the articles you extracted for a claimed video. task_id from next_unprocessed_video; result_json is the integrate envelope {\"articles\":[...],\"questions\":[...]} (or {\"needs_transcription\":true,\"articles\":[]} if the transcript is unusable). Upserts articles, files questions, marks the video processed, and rebuilds the search index."
    )]
    async fn submit_articles(
        &self,
        Parameters(p): Parameters<SubmitParams>,
    ) -> Result<String, McpError> {
        let result: Value = serde_json::from_str(&p.result_json).map_err(|e| {
            McpError::invalid_params(format!("result_json is not valid JSON: {e}"), None)
        })?;
        let task_id = p.task_id;
        let outcome = self
            .db
            .call(move |c| queue::submit_integrate(c, task_id, &result))
            .await
            .map_err(oops)?;
        if outcome.reindex {
            let docs = self.db.call(|c| kb::index_docs(c)).await.map_err(oops)?;
            let index = self.index.clone();
            tokio::task::spawn_blocking(move || index.rebuild(&docs))
                .await
                .map_err(|e| oops(e.into()))?
                .map_err(oops)?;
        }
        Ok(outcome.summary.to_string())
    }
}

#[tool_handler]
impl ServerHandler for CyberaanchaMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is #[non_exhaustive] — build from default, then set fields.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_prompts()
            .build();
        info.server_info = Implementation::from_build_env();
        info.server_info.name = "cyberaancha".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(format!(
            "Curated knowledge base of {}'s public statements (YouTube). Search \
             topics, read articles (with sources and opinion timelines), review \
             and answer open questions. Reference material — quotes and attributes \
             the owner; not medical advice.",
            self.owner_name,
        ));
        info
    }

    // Prompts are added manually (the #[tool_handler] macro only wires tools).
    async fn list_prompts(
        &self,
        _req: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "integrate",
            Some(
                "Turn one harvested video into curated KB articles — the preparer loop (next_unprocessed_video → categorize/reconcile → submit_articles).",
            ),
            None,
        )]))
    }

    async fn get_prompt(
        &self,
        req: GetPromptRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        match req.name.as_str() {
            "integrate" => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                Role::User,
                INTEGRATE_PROMPT,
            )])),
            other => Err(McpError::invalid_params(
                format!("unknown prompt {other:?}"),
                None,
            )),
        }
    }
}

/// Build the streamable-HTTP MCP service to nest at `/mcp`. A fresh handler per
/// session; db/index are cheap to clone (Arc/handle).
///
/// `public_host` is the panel/MCP hostname (from public_url): rmcp's default
/// DNS-rebinding protection accepts only loopback, so behind nginx we must add
/// our own host or every proxied request is rejected with a 403.
pub fn service(
    db: Db,
    index: Arc<SearchIndex>,
    public_host: Option<String>,
    owner_name: String,
) -> StreamableHttpService<CyberaanchaMcp, LocalSessionManager> {
    let mut allowed_hosts: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Some(host) = public_host {
        allowed_hosts.push(host);
    }
    // Config is #[non_exhaustive] — use the builder method, not a struct literal.
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts);
    StreamableHttpService::new(
        move || {
            Ok(CyberaanchaMcp::new(
                db.clone(),
                index.clone(),
                owner_name.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// Extract the bare host from a public_url like `https://youtube.serezhkin.com`.
pub fn host_of(public_url: &str) -> Option<String> {
    let after_scheme = public_url.split("://").nth(1).unwrap_or(public_url);
    let host = after_scheme.split(['/', ':']).next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}
