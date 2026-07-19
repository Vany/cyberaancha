//! Knowledge base domain: articles and their aliases / stances / facts / links.
//! `upsert` is the write path used by integrate (P4b) and tests; the reads feed
//! the index builder, the answer engine, and the panel.

use anyhow::{Result, bail};
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// One full article as integrate produces it. Replacing an article replaces its
/// child rows wholesale — integrate always sends the complete current picture.
#[derive(Debug, Deserialize)]
pub struct ArticleInput {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub paragraph_ru: String,
    #[serde(default)]
    pub story_md: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub stances: Vec<Stance>,
    #[serde(default)]
    pub facts: Vec<Fact>,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Stance {
    pub text: String,
    #[serde(default)]
    pub video_id: Option<String>,
    #[serde(default)]
    pub offset_ms: Option<i64>,
    pub source_kind: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub authority: String,
    #[serde(default)]
    pub occurred_at: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Fact {
    pub text: String,
    pub source_kind: String,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub authority: String,
    #[serde(default)]
    pub confidence: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Link {
    pub to_slug: String,
    pub kind: String,
}

/// Flat doc the tantivy builder indexes (one per article).
pub struct IndexDoc {
    pub slug: String,
    pub title: String,
    pub aliases: String,
    pub paragraph: String,
    pub story: String,
}

/// A citation for the answer template: where she said it, with a timemark.
#[derive(Debug, Serialize)]
pub struct Citation {
    pub video_id: Option<String>,
    pub offset_ms: Option<i64>,
    pub authority: String,
    pub occurred_at: Option<String>,
    pub text: String,
}

/// Open a transaction and upsert. Use `upsert_article_tx` when already in one.
pub fn upsert_article(conn: &mut Connection, a: &ArticleInput) -> Result<()> {
    let tx = conn.transaction()?;
    upsert_article_tx(&tx, a)?;
    tx.commit()?;
    Ok(())
}

pub fn upsert_article_tx(tx: &Connection, a: &ArticleInput) -> Result<()> {
    if a.slug.is_empty() || !a.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bail!("slug must be non-empty [a-z0-9-]: {:?}", a.slug);
    }
    let status = a.status.as_deref().unwrap_or("draft");
    let now = Timestamp::now().to_string();
    tx.execute(
        "INSERT INTO articles (slug, title, paragraph_ru, story_md, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(slug) DO UPDATE SET title = excluded.title, paragraph_ru = excluded.paragraph_ru,
             story_md = excluded.story_md, status = excluded.status, updated_at = excluded.updated_at",
        params![a.slug, a.title, a.paragraph_ru, a.story_md, status, now],
    )?;
    // Children are fully rewritten from the incoming complete picture.
    tx.execute("DELETE FROM article_aliases WHERE article_slug = ?1", [&a.slug])?;
    tx.execute("DELETE FROM stances WHERE article_slug = ?1", [&a.slug])?;
    tx.execute("DELETE FROM facts WHERE article_slug = ?1", [&a.slug])?;
    tx.execute("DELETE FROM article_links WHERE from_slug = ?1", [&a.slug])?;

    for alias in &a.aliases {
        tx.execute(
            "INSERT OR IGNORE INTO article_aliases (article_slug, alias) VALUES (?1, ?2)",
            params![a.slug, alias],
        )?;
    }
    for s in &a.stances {
        tx.execute(
            "INSERT INTO stances (article_slug, text, video_id, offset_ms, source_kind, source_ref, authority, occurred_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![a.slug, s.text, s.video_id, s.offset_ms, s.source_kind, s.source_ref, s.authority, s.occurred_at, now],
        )?;
    }
    for f in &a.facts {
        tx.execute(
            "INSERT INTO facts (article_slug, text, source_kind, source_ref, authority, confidence, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![a.slug, f.text, f.source_kind, f.source_ref, f.authority, f.confidence, now],
        )?;
    }
    for l in &a.links {
        // Skip dangling targets rather than fail the whole integrate.
        let exists = tx
            .query_row("SELECT 1 FROM articles WHERE slug = ?1", [&l.to_slug], |_| Ok(()))
            .optional()?
            .is_some();
        if exists && l.to_slug != a.slug {
            tx.execute(
                "INSERT OR IGNORE INTO article_links (from_slug, to_slug, kind) VALUES (?1, ?2, ?3)",
                params![a.slug, l.to_slug, l.kind],
            )?;
        }
    }
    Ok(())
}

pub fn index_docs(conn: &Connection) -> Result<Vec<IndexDoc>> {
    let mut stmt = conn.prepare(
        "SELECT a.slug, a.title, a.paragraph_ru, a.story_md,
                COALESCE((SELECT group_concat(alias, ' ') FROM article_aliases WHERE article_slug = a.slug), '')
         FROM articles a WHERE a.status = 'published'",
    )?;
    let docs = stmt
        .query_map([], |r| {
            Ok(IndexDoc {
                slug: r.get(0)?,
                title: r.get(1)?,
                paragraph: r.get(2)?,
                story: r.get(3)?,
                aliases: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(docs)
}

/// Newest-first stances that have a citable video link — for the answer template.
pub fn citations(conn: &Connection, slug: &str) -> Result<Vec<Citation>> {
    let mut stmt = conn.prepare(
        "SELECT video_id, offset_ms, authority, occurred_at, text FROM stances
         WHERE article_slug = ?1
         ORDER BY (occurred_at IS NULL), occurred_at DESC, id DESC",
    )?;
    let out = stmt
        .query_map([slug], |r| {
            Ok(Citation {
                video_id: r.get(0)?,
                offset_ms: r.get(1)?,
                authority: r.get(2)?,
                occurred_at: r.get(3)?,
                text: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(out)
}

/// Full article, enough to render AND to round-trip an owner edit without loss.
/// `citations` is a display projection of `stances`; the panel edits and
/// re-sends `stances`/`facts`/`links` verbatim (upsert replaces children).
#[derive(Debug, Serialize)]
pub struct ArticleView {
    pub slug: String,
    pub title: String,
    pub paragraph_ru: String,
    pub story_md: String,
    pub status: String,
    pub aliases: Vec<String>,
    pub citations: Vec<Citation>,
    pub stances: Vec<Stance>,
    pub facts: Vec<Fact>,
    pub links: Vec<Link>,
}

pub fn get_article(conn: &Connection, slug: &str) -> Result<Option<ArticleView>> {
    let base = conn
        .query_row(
            "SELECT slug, title, paragraph_ru, story_md, status FROM articles WHERE slug = ?1",
            [slug],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((slug, title, paragraph_ru, story_md, status)) = base else {
        return Ok(None);
    };
    let mut stmt = conn.prepare("SELECT alias FROM article_aliases WHERE article_slug = ?1")?;
    let aliases = stmt
        .query_map([&slug], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;

    let mut sstmt = conn.prepare(
        "SELECT text, video_id, offset_ms, source_kind, source_ref, authority, occurred_at
         FROM stances WHERE article_slug = ?1 ORDER BY id",
    )?;
    let stances = sstmt
        .query_map([&slug], |r| {
            Ok(Stance {
                text: r.get(0)?,
                video_id: r.get(1)?,
                offset_ms: r.get(2)?,
                source_kind: r.get(3)?,
                source_ref: r.get(4)?,
                authority: r.get(5)?,
                occurred_at: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;

    let mut fstmt = conn.prepare(
        "SELECT text, source_kind, source_ref, authority, confidence FROM facts
         WHERE article_slug = ?1 ORDER BY id",
    )?;
    let facts = fstmt
        .query_map([&slug], |r| {
            Ok(Fact {
                text: r.get(0)?,
                source_kind: r.get(1)?,
                source_ref: r.get(2)?,
                authority: r.get(3)?,
                confidence: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;

    let mut lstmt =
        conn.prepare("SELECT to_slug, kind FROM article_links WHERE from_slug = ?1 ORDER BY to_slug")?;
    let links = lstmt
        .query_map([&slug], |r| Ok(Link { to_slug: r.get(0)?, kind: r.get(1)? }))?
        .collect::<std::result::Result<_, _>>()?;

    Ok(Some(ArticleView {
        citations: citations(conn, &slug)?,
        slug,
        title,
        paragraph_ru,
        story_md,
        status,
        aliases,
        stances,
        facts,
        links,
    }))
}

/// Log a query and its best hit (or miss) — feeds the questions + alias loop.
pub fn log_query(conn: &Connection, q: &str, hit: Option<&str>, score: Option<f32>) -> Result<()> {
    conn.execute(
        "INSERT INTO queries (q, hit_slug, score, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![q, hit, score, Timestamp::now().to_string()],
    )?;
    Ok(())
}
