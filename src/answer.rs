//! The answer engine: query → search → templated Russian reply with citations.
//! This is exactly what the Telegram bot will send (test tab now, TG later).
//! No LLM (C1): it only selects a prebaked article and formats its links.

use crate::index::SearchIndex;
use crate::kb;
use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// A tantivy hit already means ≥1 query term matched a curated field; a genuine
/// miss returns no rows at all. So the floor only rejects the very weakest
/// matches, and must be corpus-size robust: BM25 scores scale with IDF, which
/// collapses toward zero on a tiny corpus (a term in the only article has ~0
/// IDF). Keep it permissive; precision is tuned from the `queries` miss-log once
/// a real corpus exists (SPEC §7). Empty results, not a low score, is the miss.
const SCORE_FLOOR: f32 = 0.1;
/// SPEC §6: at most 5 citation links per answer.
const MAX_LINKS: usize = 5;

#[derive(Debug, Serialize)]
pub struct Answer {
    pub hit: bool,
    /// Rendered Russian text — what the bot posts.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Sibling topics that also matched, for "смежные темы".
    pub related: Vec<String>,
}

pub fn answer(conn: &Connection, index: &SearchIndex, query: &str) -> Result<Answer> {
    let hits = index.search(query, 6)?;
    let best = hits.first().filter(|h| h.score >= SCORE_FLOOR);

    let Some(best) = best else {
        kb::log_query(conn, query, None, hits.first().map(|h| h.score))?;
        return Ok(Answer {
            hit: false,
            text: "Профессор эту тему пока не разбирала. Вопрос сохранён.".to_string(),
            slug: None,
            related: vec![],
        });
    };

    kb::log_query(conn, query, Some(&best.slug), Some(best.score))?;
    let Some(article) = kb::get_article(conn, &best.slug)? else {
        // Index and DB disagree (article deleted since last rebuild) — treat as miss.
        return Ok(Answer {
            hit: false,
            text: "Профессор эту тему пока не разбирала. Вопрос сохранён.".to_string(),
            slug: None,
            related: vec![],
        });
    };

    let related: Vec<String> = hits
        .iter()
        .skip(1)
        .filter(|h| h.score >= SCORE_FLOOR)
        .take(3)
        .map(|h| h.slug.clone())
        .collect();

    Ok(Answer {
        hit: true,
        text: render(&article, &related),
        slug: Some(article.slug),
        related,
    })
}

fn render(article: &kb::ArticleView, related: &[String]) -> String {
    let mut links = vec![];
    let mut latest_reconsider: Option<String> = None;
    for c in &article.citations {
        if let Some(url) = citation_url(c) {
            // The newest cited stance reads as the current/reconsidered take.
            if latest_reconsider.is_none() && links.is_empty() {
                latest_reconsider = Some(url.clone());
            }
            links.push(url);
        }
        if links.len() >= MAX_LINKS {
            break;
        }
    }

    let mut out = String::new();
    out.push_str(&format!("Про «{}»", article.title));
    if !links.is_empty() {
        let head = &links[..links.len().min(MAX_LINKS)];
        out.push_str(" — ");
        out.push_str(&head.join(", "));
    }
    out.push_str(".\n");
    if !article.paragraph_ru.is_empty() {
        out.push_str(&format!("Мнение профессора: {}\n", article.paragraph_ru));
    }
    if !related.is_empty() {
        out.push_str(&format!("Смежные темы: {}.\n", related.join(", ")));
    }
    out.push_str("— Справочный материал по выступлениям проф. Барановой, не медицинская рекомендация.");
    out
}

/// A YouTube link with timemark, when the stance came from a video.
fn citation_url(c: &kb::Citation) -> Option<String> {
    let vid = c.video_id.as_deref()?;
    let t = c.offset_ms.unwrap_or(0) / 1000;
    Some(format!("https://youtu.be/{vid}?t={t}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::kb::{ArticleInput, Stance};

    fn seed(conn: &mut Connection) {
        let a = ArticleInput {
            slug: "gemorroj".into(),
            title: "Геморрой".into(),
            paragraph_ru: "Профессор советует не игнорировать симптомы.".into(),
            story_md: "Полная история…".into(),
            status: Some("published".into()),
            aliases: vec!["боль в заднице".into(), "узлы".into()],
            stances: vec![
                Stance {
                    text: "старое мнение".into(),
                    video_id: Some("aaaaaaaaaaa".into()),
                    offset_ms: Some(60_000),
                    source_kind: "video".into(),
                    source_ref: None,
                    authority: "spoken".into(),
                    occurred_at: Some("2024-01-01T00:00:00Z".into()),
                },
                Stance {
                    text: "новое мнение".into(),
                    video_id: Some("bbbbbbbbbbb".into()),
                    offset_ms: Some(30_000),
                    source_kind: "video".into(),
                    source_ref: None,
                    authority: "spoken".into(),
                    occurred_at: Some("2026-06-01T00:00:00Z".into()),
                },
            ],
            facts: vec![],
            links: vec![],
        };
        kb::upsert_article(conn, &a).unwrap();
    }

    #[test]
    fn hit_renders_links_newest_first_and_disclaimer() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let idx = SearchIndex::open(&dir.path().join("idx"), 15)?;
        db.with(|c| {
            seed(c);
            idx.rebuild(&kb::index_docs(c)?)?;
            let a = answer(c, &idx, "боль в заднице")?;
            assert!(a.hit);
            // Newest stance (2026, video bbbb) leads the citations.
            assert!(a.text.contains("https://youtu.be/bbbbbbbbbbb?t=30"));
            assert!(a.text.contains("https://youtu.be/aaaaaaaaaaa?t=60"));
            assert!(a.text.contains("не медицинская рекомендация"));
            assert!(a.text.contains("Мнение профессора"));
            // Query was logged as a hit.
            let logged: String =
                c.query_row("SELECT hit_slug FROM queries ORDER BY id DESC LIMIT 1", [], |r| r.get(0))?;
            assert_eq!(logged, "gemorroj");
            Ok(())
        })
    }

    #[test]
    fn miss_is_honest_and_logged() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let idx = SearchIndex::open(&dir.path().join("idx"), 15)?;
        db.with(|c| {
            seed(c);
            idx.rebuild(&kb::index_docs(c)?)?;
            let a = answer(c, &idx, "нейтронные звёзды")?;
            assert!(!a.hit);
            assert!(a.text.contains("пока не разбирала"));
            let miss: Option<String> =
                c.query_row("SELECT hit_slug FROM queries ORDER BY id DESC LIMIT 1", [], |r| r.get(0))?;
            assert!(miss.is_none());
            Ok(())
        })
    }
}
