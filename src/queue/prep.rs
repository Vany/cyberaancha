//! Preparer side of the queue (Mac edge, SPEC §12): `integrate` (a Claude
//! session reads a video's bundle, searches the KB, writes articles) and
//! `transcribe` (the unattended Whisper script fills a missing transcript).
//!
//! integrate is SERIALIZED — at most one active — so concurrent sessions can't
//! race on create-vs-merge decisions. It collapses SPEC's extract+integrate into
//! one pass (MEMO 2026-07-19): simpler for the Claude-session model, and a
//! separate batched extract can be slotted in later without touching this.

use super::{LEASE_MINUTES, MAX_ATTEMPTS, validator};
use crate::db;
use crate::kb::{self, ArticleInput};
use crate::raw;
use anyhow::{Context, Result, bail};
use jiff::{Timestamp, ToSpan};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// Enqueue integrate tasks for harvested-but-unintegrated videos in the window.
/// "Ready" = metadata done and no harvest stage still pending; a captions-less
/// video still integrates (its Claude pass can return needs_transcription).
pub fn enqueue_integrate(conn: &Connection) -> Result<Value> {
    let now = Timestamp::now().to_string();
    let mut stmt = conn.prepare(
        "SELECT yt_id FROM videos
         WHERE integrated = 0 AND meta_done = 1
           AND captions_state != 'pending' AND comments_state != 'pending'
           AND chat_state != 'pending'
           AND yt_id NOT IN (SELECT subject FROM tasks WHERE type = 'integrate' AND state IN ('pending','claimed'))",
    )?;
    let ready: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut created = 0;
    for yt_id in &ready {
        // Re-open a prior done task (e.g. after transcription reopened the video);
        // the ready-query already excluded anything pending/claimed.
        created += conn.execute(
            "INSERT INTO tasks (type, subject, state, created_at) VALUES ('integrate', ?1, 'pending', ?2)
             ON CONFLICT(type, subject) DO UPDATE SET state = 'pending', attempt = 0,
                 error = NULL, lease_until = NULL, claimed_by = NULL, done_at = NULL,
                 created_at = excluded.created_at",
            params![yt_id, now],
        )?;
    }
    Ok(json!({ "integrate_enqueued": created, "ready_videos": ready.len() }))
}

#[derive(Debug, serde::Serialize)]
pub struct ClaimedPrep {
    pub id: i64,
    pub subject: String,
    pub bundle: Value,
}

/// Claim the one active integrate task (serialized). Returns None when idle or
/// when another integrate is still in flight.
pub fn claim_integrate(conn: &mut Connection) -> Result<Option<ClaimedPrep>> {
    let now = Timestamp::now();
    let now_s = now.to_string();
    // Reclaim an expired lease, else refuse while one is genuinely active.
    let active: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, lease_until FROM tasks WHERE type = 'integrate' AND state = 'claimed' LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?;
    if let Some((id, lease)) = active {
        if lease.as_str() > now_s.as_str() {
            return Ok(None); // still leased to another worker
        }
        // expired — fail it if exhausted, otherwise reclaim below
        conn.execute(
            "UPDATE tasks SET state = CASE WHEN attempt >= ?2 THEN 'failed' ELSE 'pending' END
             WHERE id = ?1",
            params![id, MAX_ATTEMPTS],
        )?;
    }

    let lease = now.checked_add(LEASE_MINUTES.minutes()).context("lease overflow")?.to_string();
    let claimed: Option<(i64, String)> = conn
        .query_row(
            "UPDATE tasks SET state = 'claimed', lease_until = ?1, claimed_by = 'preparer', attempt = attempt + 1
             WHERE id = (SELECT id FROM tasks WHERE type = 'integrate' AND state = 'pending' ORDER BY id LIMIT 1)
             RETURNING id, subject",
            [&lease],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((id, subject)) = claimed else {
        return Ok(None);
    };
    let bundle = build_bundle(conn, &subject)?;
    Ok(Some(ClaimedPrep { id, subject, bundle }))
}

/// Everything a Claude session needs to integrate one video: metadata, the
/// transcript with timestamps, comments (the professor's replies flagged), and
/// her chat messages. Sizeable by design.
fn build_bundle(conn: &Connection, yt_id: &str) -> Result<Value> {
    let meta = conn
        .query_row(
            "SELECT kind, title, description, published_at, duration_s, captions_state, transcribe_state
             FROM videos WHERE yt_id = ?1",
            [yt_id],
            |r| {
                Ok(json!({
                    "yt_id": yt_id,
                    "kind": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                    "description": r.get::<_, Option<String>>(2)?,
                    "published_at": r.get::<_, Option<String>>(3)?,
                    "duration_s": r.get::<_, Option<i64>>(4)?,
                    "captions_state": r.get::<_, String>(5)?,
                    "transcribe_state": r.get::<_, String>(6)?,
                }))
            },
        )
        .optional()?
        .with_context(|| format!("integrate bundle: unknown video {yt_id}"))?;

    // Transcript segments (decompressed) — the spine of extraction.
    let transcript = conn
        .query_row(
            "SELECT source, lang, segments_zstd FROM transcripts WHERE video_id = ?1",
            [yt_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?)),
        )
        .optional()?;
    let transcript = match transcript {
        Some((source, lang, zst)) => {
            let segs: Value = serde_json::from_slice(&raw::decompress(&zst)?)?;
            json!({ "source": source, "lang": lang, "segments": segs })
        }
        None => Value::Null,
    };

    let comments = rows_to_json(
        conn,
        "SELECT text, author_name, is_author, like_count, parent_id FROM comments
         WHERE video_id = ?1 ORDER BY is_author DESC, like_count DESC LIMIT 5000",
        yt_id,
        &["text", "author_name", "is_author", "like_count", "parent_id"],
    )?;
    let chat_author = rows_to_json(
        conn,
        "SELECT text, offset_ms, author_name, is_author FROM chat_messages
         WHERE video_id = ?1 AND is_author = 1 ORDER BY offset_ms LIMIT 5000",
        yt_id,
        &["text", "offset_ms", "author_name", "is_author"],
    )?;

    Ok(json!({
        "video": meta,
        "transcript": transcript,
        "comments": comments,
        "professor_chat": chat_author,
        "instructions": "Search the KB (GET /api/prep/search?q=) before creating; merge into existing slugs when the topic matches. Emit inflected/colloquial/latin aliases (RU stemming is imperfect). Raise questions for contradictions.",
    }))
}

fn rows_to_json(conn: &Connection, sql: &str, yt_id: &str, cols: &[&str]) -> Result<Value> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([yt_id], |r| {
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v = r.get_ref(i)?;
            let jv = match v {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => json!(n),
                rusqlite::types::ValueRef::Real(f) => json!(f),
                rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                rusqlite::types::ValueRef::Blob(_) => Value::Null,
            };
            obj.insert((*name).to_string(), jv);
        }
        Ok(Value::Object(obj))
    })?;
    Ok(Value::Array(rows.collect::<std::result::Result<_, _>>()?))
}

pub struct IntegrateOutcome {
    /// The KB changed and the search index should be rebuilt.
    pub reindex: bool,
    pub summary: Value,
}

/// Validate + apply an integrate submission: upsert articles, file questions,
/// mark the video integrated (or spawn a transcribe task), advance the processed
/// clock. The handler rebuilds the index when `reindex` is set.
pub fn submit_integrate(conn: &mut Connection, task_id: i64, result: &Value) -> Result<IntegrateOutcome> {
    let (subject, state): (String, String) = conn
        .query_row("SELECT subject, state FROM tasks WHERE id = ?1 AND type = 'integrate'", [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .with_context(|| format!("no integrate task {task_id}"))?;
    if state != "claimed" {
        bail!("integrate task {task_id} is {state}, not claimed");
    }
    let v = validator("integrate")?;
    let errs: Vec<String> = v.iter_errors(result).map(|e| format!("{e}")).take(5).collect();
    if !errs.is_empty() {
        bail!("integrate schema validation failed: {}", errs.join("; "));
    }

    let tx = conn.transaction()?;

    // needs_transcription: no KB write; spawn a transcribe task and finish.
    if result["needs_transcription"].as_bool() == Some(true) {
        tx.execute("UPDATE videos SET transcribe_state = 'pending' WHERE yt_id = ?1", [&subject])?;
        tx.execute(
            "INSERT INTO tasks (type, subject, state, created_at) VALUES ('transcribe', ?1, 'pending', ?2)
             ON CONFLICT(type, subject) DO UPDATE SET state = 'pending', attempt = 0, error = NULL",
            params![subject, Timestamp::now().to_string()],
        )?;
        tx.execute("UPDATE tasks SET state = 'done', done_at = ?1 WHERE id = ?2",
            params![Timestamp::now().to_string(), task_id])?;
        tx.commit()?;
        return Ok(IntegrateOutcome { reindex: false, summary: json!({ "video": subject, "needs_transcription": true }) });
    }

    let articles = result["articles"].as_array().expect("validated");
    for art in articles {
        let input: ArticleInput = serde_json::from_value(art.clone())
            .with_context(|| "integrate article shape")?;
        kb::upsert_article_tx(&tx, &input)?;
    }
    let mut questions = 0;
    if let Some(qs) = result["questions"].as_array() {
        for q in qs {
            tx.execute(
                "INSERT INTO questions (article_slug, context, question, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    q["article_slug"].as_str(),
                    q["context"].as_str().unwrap_or(""),
                    q["question"].as_str(),
                    Timestamp::now().to_string(),
                ],
            )?;
            questions += 1;
        }
    }
    tx.execute("UPDATE videos SET integrated = 1 WHERE yt_id = ?1", [&subject])?;
    tx.execute("UPDATE tasks SET state = 'done', done_at = ?1 WHERE id = ?2",
        params![Timestamp::now().to_string(), task_id])?;
    db::meta_set(&tx, "last_processed_at", &Timestamp::now().to_string())?;
    tx.commit()?;

    Ok(IntegrateOutcome {
        reindex: true,
        summary: json!({ "video": subject, "articles": articles.len(), "questions": questions }),
    })
}

// ---- transcribe (unattended script worker) ---------------------------------

pub fn claim_transcribe(conn: &Connection) -> Result<Option<Value>> {
    let now = Timestamp::now();
    let lease = now.checked_add(LEASE_MINUTES.minutes()).context("lease overflow")?.to_string();
    let now_s = now.to_string();
    let claimed: Option<(i64, String)> = conn
        .query_row(
            "UPDATE tasks SET state = 'claimed', lease_until = ?1, claimed_by = 'transcriber', attempt = attempt + 1
             WHERE id = (SELECT id FROM tasks WHERE type = 'transcribe'
                         AND (state = 'pending' OR (state = 'claimed' AND lease_until < ?2)) ORDER BY id LIMIT 1)
             RETURNING id, subject",
            params![lease, now_s],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(claimed.map(|(id, yt_id)| json!({ "id": id, "yt_id": yt_id })))
}

pub fn submit_transcribe(conn: &Connection, task_id: i64, result: &Value) -> Result<Value> {
    let (subject, state): (String, String) = conn
        .query_row("SELECT subject, state FROM tasks WHERE id = ?1 AND type = 'transcribe'", [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .with_context(|| format!("no transcribe task {task_id}"))?;
    if state != "claimed" {
        bail!("transcribe task {task_id} is {state}, not claimed");
    }
    let v = validator("transcribe")?;
    let errs: Vec<String> = v.iter_errors(result).map(|e| format!("{e}")).take(5).collect();
    if !errs.is_empty() {
        bail!("transcribe schema validation failed: {}", errs.join("; "));
    }
    if result["yt_id"].as_str() != Some(subject.as_str()) {
        bail!("transcribe yt_id does not match task subject {subject}");
    }
    let compact = serde_json::to_vec(&result["segments"])?;
    let packed = raw::compress(&compact)?;
    conn.execute(
        "INSERT INTO transcripts (video_id, source, lang, segments_zstd, updated_at)
         VALUES (?1, 'whisper', ?2, ?3, ?4)
         ON CONFLICT(video_id) DO UPDATE SET source = 'whisper', lang = excluded.lang,
             segments_zstd = excluded.segments_zstd, updated_at = excluded.updated_at",
        params![subject, result["lang"].as_str(), packed, Timestamp::now().to_string()],
    )?;
    // Transcript now exists → clear the way to re-integrate this video.
    conn.execute(
        "UPDATE videos SET transcribe_state = 'done', captions_state = 'have', integrated = 0 WHERE yt_id = ?1",
        [&subject],
    )?;
    conn.execute("UPDATE tasks SET state = 'done', done_at = ?1 WHERE id = ?2",
        params![Timestamp::now().to_string(), task_id])?;
    let n = result["segments"].as_array().map(|a| a.len()).unwrap_or(0);
    Ok(json!({ "video": subject, "segments": n }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::Db;
    use crate::index::SearchIndex;
    use crate::queue;

    fn cfg() -> Config {
        toml::from_str(r#"[channel]
            handle = "@test"
            [server]
            data_dir = "unused""#).unwrap()
    }

    /// Full spine: harvest one video → enqueue+claim integrate (bundle carries
    /// the transcript) → submit an article → reindex → the answer engine finds it.
    #[test]
    fn integrate_pipeline_end_to_end() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let cfg = cfg();
        let idx = SearchIndex::open(&dir.path().join("idx"), 15)?;

        db.with(|c| {
            // Harvest one stream: discover → meta + captions + comments + chat.
            queue::enqueue_wave(c, &cfg, "back")?;
            let d = queue::claim(c, &cfg, "collector", 5)?;
            let now = Timestamp::now();
            let at = now.checked_sub(24.hours()).unwrap().to_string();
            queue::submit(c, &cfg, d[0].id, &json!({
                "channel_id": "UC12345678901234567890AB",
                "videos": [{ "yt_id": "vid00000001", "title": "Про сон", "kind": "stream",
                             "approx_published": at, "duration_s": 3600 }]
            }))?;
            for t in queue::claim(c, &cfg, "collector", 10)? {
                let r = match t.r#type.as_str() {
                    "harvest_meta" => json!({ "yt_id": t.subject, "title": "Про сон",
                        "published_at": "2026-07-01T00:00:00Z", "duration_s": 3600,
                        "channel_id": "UC12345678901234567890AB", "raw_player": "{}" }),
                    "harvest_captions" => json!({ "yt_id": t.subject, "lang": "ru", "source": "asr",
                        "segments": [{ "t_ms": 60000, "d_ms": 4000, "text": "мелатонин помогает засыпать" }] }),
                    "harvest_comments" => json!({ "yt_id": t.subject, "comments": [
                        { "id": "cx", "text": "а доза?", "author_channel_id": "UCaaaaaaaaaaaaaaaaaaaaaa", "author_name": "fan" },
                        { "id": "ax", "text": "три миллиграмма", "parent_id": "cx",
                          "author_channel_id": "UC12345678901234567890AB", "author_name": "prof" }] }),
                    "harvest_chat" => json!({ "yt_id": t.subject, "messages": [
                        { "id": "gm", "offset_ms": 1000, "text": "спасибо профессор",
                          "author_channel_id": "UC12345678901234567890AB", "author_name": "prof" }] }),
                    other => panic!("unexpected {other}"),
                };
                queue::submit(c, &cfg, t.id, &r)?;
            }

            // Enqueue + claim integrate; the bundle must carry transcript + prof reply.
            let enq = enqueue_integrate(c)?;
            assert_eq!(enq["integrate_enqueued"], 1);
            let task = claim_integrate(c)?.expect("an integrate task");
            assert_eq!(task.subject, "vid00000001");
            let b = &task.bundle;
            assert_eq!(b["transcript"]["segments"][0]["text"], "мелатонин помогает засыпать");
            // Professor's comment reply and chat message are surfaced.
            assert!(b["comments"].as_array().unwrap().iter().any(|c| c["is_author"] == 1));
            assert_eq!(b["professor_chat"].as_array().unwrap().len(), 1);
            // Serialized: no second integrate while one is active.
            assert!(claim_integrate(c)?.is_none());

            // Submit the article the "Claude session" wrote.
            let out = submit_integrate(c, task.id, &json!({
                "articles": [{
                    "slug": "melatonin", "title": "Мелатонин", "status": "published",
                    "paragraph_ru": "Профессор: мелатонин помогает засыпать, обычно три миллиграмма.",
                    "aliases": ["мелатонина", "сон", "бессонница", "melatonin"],
                    "stances": [{ "text": "помогает засыпать", "video_id": "vid00000001",
                        "offset_ms": 60000, "source_kind": "video", "authority": "spoken",
                        "occurred_at": "2026-07-01T00:00:00Z" }]
                }],
                "questions": [{ "context": "доза обсуждалась в комментарии", "question": "Точная доза для пожилых?" }]
            }))?;
            assert!(out.reindex);
            assert_eq!(out.summary["articles"], 1);

            // Video marked integrated; question filed; processed clock set.
            let integrated: i64 = c.query_row("SELECT integrated FROM videos WHERE yt_id = 'vid00000001'", [], |r| r.get(0))?;
            assert_eq!(integrated, 1);
            let open_q: i64 = c.query_row("SELECT COUNT(*) FROM questions WHERE status='open'", [], |r| r.get(0))?;
            assert_eq!(open_q, 1);

            // Reindex + answer: colloquial query finds the article via alias.
            idx.rebuild(&kb::index_docs(c)?)?;
            let a = crate::answer::answer(c, &idx, "бессонница")?;
            assert!(a.hit);
            assert_eq!(a.slug.as_deref(), Some("melatonin"));
            assert!(a.text.contains("https://youtu.be/vid00000001?t=60"));
            Ok(())
        })
    }

    #[test]
    fn needs_transcription_spawns_transcribe_and_reintegrates() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let cfg = cfg();
        db.with(|c| {
            // A video with no captions, harvested.
            queue::enqueue_wave(c, &cfg, "back")?;
            let d = queue::claim(c, &cfg, "collector", 5)?;
            let at = Timestamp::now().checked_sub(24.hours()).unwrap().to_string();
            queue::submit(c, &cfg, d[0].id, &json!({ "channel_id": "UC12345678901234567890AB",
                "videos": [{ "yt_id": "nocapvideo1", "title": "t", "kind": "video", "approx_published": at }] }))?;
            for t in queue::claim(c, &cfg, "collector", 10)? {
                let r = match t.r#type.as_str() {
                    "harvest_meta" => json!({ "yt_id": t.subject, "title": "t",
                        "published_at": "2026-07-01T00:00:00Z", "raw_player": "{}" }),
                    "harvest_captions" => json!({ "yt_id": t.subject, "none": true }),
                    "harvest_comments" => json!({ "yt_id": t.subject, "comments": [] }),
                    other => panic!("unexpected {other}"),
                };
                queue::submit(c, &cfg, t.id, &r)?;
            }
            enqueue_integrate(c)?;
            let task = claim_integrate(c)?.expect("integrate task");

            // Claude says captions are unusable → spawns a transcribe task.
            let out = submit_integrate(c, task.id, &json!({ "needs_transcription": true, "articles": [] }))?;
            assert!(!out.reindex);
            let tj = claim_transcribe(c)?.expect("a transcribe task");
            assert_eq!(tj["yt_id"], "nocapvideo1");

            // Whisper returns segments; video reopens for integration.
            submit_transcribe(c, tj["id"].as_i64().unwrap(), &json!({ "yt_id": "nocapvideo1",
                "lang": "ru", "model": "large-v3-turbo",
                "segments": [{ "t_ms": 0, "text": "восстановленный текст" }] }))?;
            let (cap, integ): (String, i64) = c.query_row(
                "SELECT captions_state, integrated FROM videos WHERE yt_id='nocapvideo1'", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!((cap.as_str(), integ), ("have", 0)); // ready to integrate again
            let re = enqueue_integrate(c)?;
            assert_eq!(re["integrate_enqueued"], 1);
            Ok(())
        })
    }
}
