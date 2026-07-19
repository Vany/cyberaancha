//! MCP endpoint (SPEC §10): the same read-mostly KB surface the panel uses,
//! exposed to Claude (hers and ours) over streamable HTTP at `/mcp`. No LLM
//! server-side — these tools just search, read, and record. Bearer-gated by the
//! `mcp` token (middleware in http/mod.rs); URL + token shown in the panel.

use crate::db::Db;
use crate::index::SearchIndex;
use crate::kb;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct AanchaMcp {
    db: Db,
    index: Arc<SearchIndex>,
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

fn oops(e: anyhow::Error) -> McpError {
    McpError::internal_error(format!("{e:#}"), None)
}

#[tool_router]
impl AanchaMcp {
    pub fn new(db: Db, index: Arc<SearchIndex>) -> Self {
        Self { db, index, tool_router: Self::tool_router() }
    }

    #[tool(description = "Full-text search Prof. Baranova's knowledge base. Returns matching article slugs, titles, and BM25 scores.")]
    async fn search_articles(&self, Parameters(p): Parameters<SearchParams>) -> Result<String, McpError> {
        let hits = self.index.search(&p.query, 20).map_err(oops)?;
        let out = self
            .db
            .call(move |c| {
                let mut results = vec![];
                for h in hits {
                    let title: Option<String> = c
                        .query_row("SELECT title FROM articles WHERE slug = ?1", [&h.slug], |r| r.get(0))
                        .ok();
                    results.push(json!({ "slug": h.slug, "title": title, "score": h.score }));
                }
                Ok(json!({ "results": results }))
            })
            .await
            .map_err(oops)?;
        Ok(out.to_string())
    }

    #[tool(description = "Get one full article by slug: title, paragraph, story, aliases, stances/citations, facts, and links.")]
    async fn get_article(&self, Parameters(p): Parameters<SlugParams>) -> Result<String, McpError> {
        let slug = p.slug.clone();
        let article = self.db.call(move |c| kb::get_article(c, &slug)).await.map_err(oops)?;
        match article {
            Some(a) => Ok(serde_json::to_string(&a).map_err(|e| oops(e.into()))?),
            None => Err(McpError::resource_not_found(format!("no article {:?}", p.slug), None)),
        }
    }

    #[tool(description = "List the open questions awaiting the professor's answer (contradictions and gaps found during integration).")]
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

    #[tool(description = "Answer an open question by id. Records the professor's answer as an authoritative fact for the next integration cycle.")]
    async fn answer_question(&self, Parameters(p): Parameters<AnswerParams>) -> Result<String, McpError> {
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
            return Err(McpError::resource_not_found(format!("no open question {id}"), None));
        }
        Ok(json!({ "answered": id }).to_string())
    }

    #[tool(description = "Knowledge-base statistics: counts of articles, videos, transcripts, comments, and the last gathered/processed timestamps.")]
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
                }))
            })
            .await
            .map_err(oops)?;
        Ok(out.to_string())
    }
}

#[tool_handler]
impl ServerHandler for AanchaMcp {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo is #[non_exhaustive] — build from default, then set fields.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.server_info.name = "aancha".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "Curated knowledge base of Prof. Ancha Baranova's public statements \
             (YouTube). Search topics, read articles (with sources and opinion \
             timelines), review and answer open questions. Reference material — \
             quotes and attributes the professor; not medical advice."
                .into(),
        );
        info
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
) -> StreamableHttpService<AanchaMcp, LocalSessionManager> {
    let mut allowed_hosts: Vec<String> =
        vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    if let Some(host) = public_host {
        allowed_hosts.push(host);
    }
    // Config is #[non_exhaustive] — use the builder method, not a struct literal.
    let config = StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts);
    StreamableHttpService::new(
        move || Ok(AanchaMcp::new(db.clone(), index.clone())),
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
