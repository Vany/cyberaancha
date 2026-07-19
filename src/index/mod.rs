//! tantivy fulltext over articles — the bot's entire world in production (C1:
//! no LLM at runtime). Russian Snowball stemming + build-time aliases give pure
//! BM25 enough recall. Rebuild is delete-all + refill in one writer commit, so
//! searches see the old index atomically until the new one commits.

use crate::kb::IndexDoc;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::RwLock;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value};
use tantivy::tokenizer::{LowerCaser, SimpleTokenizer, Stemmer, Language, TextAnalyzer};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument, doc};

const RU: &str = "ru";

#[derive(Clone, Copy)]
struct Fields {
    slug: Field,
    title: Field,
    aliases: Field,
    paragraph: Field,
    story: Field,
}

pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    heap_bytes: usize,
    // Serialize writers: tantivy allows one at a time; rebuilds are infrequent.
    write_lock: RwLock<()>,
}

#[derive(Debug, serde::Serialize)]
pub struct Hit {
    pub slug: String,
    pub score: f32,
}

fn schema() -> (Schema, Fields) {
    let ru_text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(RU)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let mut b = Schema::builder();
    let fields = Fields {
        slug: b.add_text_field("slug", STRING | STORED),
        title: b.add_text_field("title", ru_text.clone()),
        aliases: b.add_text_field("aliases", ru_text.clone()),
        paragraph: b.add_text_field("paragraph", ru_text.clone()),
        story: b.add_text_field("story", ru_text),
    };
    (b.build(), fields)
}

fn register_ru(index: &Index) {
    let ru = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(Stemmer::new(Language::Russian))
        .build();
    index.tokenizers().register(RU, ru);
}

impl SearchIndex {
    pub fn open(dir: &Path, heap_mb: usize) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("index dir {}", dir.display()))?;
        let (schema, fields) = schema();
        let mmap = tantivy::directory::MmapDirectory::open(dir)?;
        // A schema change across versions would make open_or_create fail; the
        // index is derivable, so the caller can wipe the dir and reopen.
        let index = Index::open_or_create(mmap, schema)?;
        register_ru(&index);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        // n1 has 457 MB; keep the writer heap modest and clamp to tantivy's floor.
        let heap_bytes = (heap_mb * 1024 * 1024).clamp(15_000_000, 200_000_000);
        Ok(Self { index, reader, fields, heap_bytes, write_lock: RwLock::new(()) })
    }

    /// Replace the whole index from the DB's published articles, atomically at commit.
    pub fn rebuild(&self, docs: &[IndexDoc]) -> Result<usize> {
        let _guard = self.write_lock.write().expect("index write lock poisoned");
        let mut writer = self.index.writer(self.heap_bytes)?;
        writer.delete_all_documents()?;
        for d in docs {
            writer.add_document(doc!(
                self.fields.slug => d.slug.clone(),
                self.fields.title => d.title.clone(),
                self.fields.aliases => d.aliases.clone(),
                self.fields.paragraph => d.paragraph.clone(),
                self.fields.story => d.story.clone(),
            ))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        tracing::info!(articles = docs.len(), "search index rebuilt");
        Ok(docs.len())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        let cleaned = sanitize(query);
        if cleaned.is_empty() {
            return Ok(vec![]);
        }
        let mut qp = QueryParser::for_index(
            &self.index,
            vec![self.fields.title, self.fields.aliases, self.fields.paragraph, self.fields.story],
        );
        qp.set_field_boost(self.fields.title, 3.0);
        qp.set_field_boost(self.fields.aliases, 2.5);
        qp.set_field_boost(self.fields.story, 0.7);
        // Default OR keeps recall high; boosts order the results.
        let query = qp.parse_query(&cleaned).context("parsing search query")?;

        let searcher = self.reader.searcher();
        let top = searcher.search(&query, &TopDocs::with_limit(limit.clamp(1, 50)).order_by_score())?;
        let mut hits = Vec::with_capacity(top.len());
        for (score, addr) in top {
            let doc: TantivyDocument = searcher.doc(addr)?;
            if let Some(slug) = doc.get_first(self.fields.slug).and_then(|v| v.as_str()) {
                hits.push(Hit { slug: slug.to_owned(), score });
            }
        }
        Ok(hits)
    }
}

/// Drop query-parser metacharacters; keep letters (incl. Cyrillic), digits, spaces.
/// Users type natural language, not tantivy syntax — this prevents parse errors.
fn sanitize(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(slug: &str, title: &str, aliases: &str, paragraph: &str) -> IndexDoc {
        IndexDoc {
            slug: slug.into(),
            title: title.into(),
            aliases: aliases.into(),
            paragraph: paragraph.into(),
            story: String::new(),
        }
    }

    #[test]
    fn russian_stemming_and_alias_recall() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let idx = SearchIndex::open(dir.path(), 15)?;
        idx.rebuild(&[
            // Aliases carry colloquial synonyms AND morphological variants: the
            // Snowball RU stemmer does NOT unify all cases (геморрой→геморр but
            // геморроя→геморро), so the preparer must emit inflected forms as
            // aliases. Here "геморроя" is an alias so the genitive query hits.
            doc("gemorroj", "Геморрой", "геморроя боль в заднице шишки узлы proctology",
                "Про геморрой профессор говорила…"),
            doc("pechen", "Печень", "гепатопротекторы АЛТ АСТ liver", "Про печень…"),
        ])?;

        // Colloquial query reaches the medical topic via alias — the money case.
        let hits = idx.search("боль в заднице", 5)?;
        assert_eq!(hits.first().map(|h| h.slug.as_str()), Some("gemorroj"));

        // Stemming that DOES work: prepositional «заднице» ~ «задница» share a stem.
        assert_eq!(idx.search("задница", 5)?.first().map(|h| h.slug.as_str()), Some("gemorroj"));

        // Nominative matches the title directly; genitive only via the alias above.
        assert_eq!(idx.search("геморрой", 5)?.first().map(|h| h.slug.as_str()), Some("gemorroj"));
        assert_eq!(idx.search("лечение геморроя", 5)?.first().map(|h| h.slug.as_str()), Some("gemorroj"));

        // Latin/EN alias recall.
        assert_eq!(idx.search("liver", 5)?.first().map(|h| h.slug.as_str()), Some("pechen"));

        // Miss returns nothing; metacharacters must not crash the parser.
        assert!(idx.search("квантовая хромодинамика", 5)?.is_empty());
        assert!(idx.search("что: (это)?? \"боль\"", 5).is_ok());
        Ok(())
    }

    #[test]
    fn rebuild_replaces_contents() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let idx = SearchIndex::open(dir.path(), 15)?;
        idx.rebuild(&[doc("a", "Альфа", "", "текст")])?;
        assert_eq!(idx.search("альфа", 5)?.len(), 1);
        idx.rebuild(&[doc("b", "Бета", "", "текст")])?;
        assert!(idx.search("альфа", 5)?.is_empty());
        assert_eq!(idx.search("бета", 5)?.first().map(|h| h.slug.as_str()), Some("b"));
        Ok(())
    }
}

