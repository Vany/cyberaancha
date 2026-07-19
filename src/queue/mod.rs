//! Task queue engine (SPEC §5): enqueue waves, claim with lease, submit with
//! JSON-Schema validation, apply results transactionally, advance watermarks.
//! The server stays dumb-but-strict: every submission is validated against
//! schemas/ and applied by deterministic code — reject, don't repair.

use crate::config::Config;
use crate::db;
use anyhow::{Context, Result, bail};
use jiff::{Timestamp, ToSpan};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::OnceLock;

const LEASE_MINUTES: i64 = 30;
const MAX_ATTEMPTS: i64 = 5;
/// Cap of per-video task pairs created from one discover submission.
const WAVE_CAP: usize = 60;

/// Schemas are compiled once; adding a task type means adding a schema here.
static VALIDATORS: OnceLock<Vec<(&'static str, jsonschema::Validator)>> = OnceLock::new();

fn validator(task_type: &str) -> Result<&'static jsonschema::Validator> {
    let all = VALIDATORS.get_or_init(|| {
        [
            ("discover", include_str!("../../schemas/discover.json")),
            ("harvest_meta", include_str!("../../schemas/harvest_meta.json")),
            ("harvest_captions", include_str!("../../schemas/harvest_captions.json")),
            ("harvest_comments", include_str!("../../schemas/harvest_comments.json")),
            ("harvest_chat", include_str!("../../schemas/harvest_chat.json")),
        ]
        .into_iter()
        .map(|(name, src)| {
            let schema: Value = serde_json::from_str(src).expect("schema is valid JSON");
            let v = jsonschema::validator_for(&schema).expect("schema compiles");
            (name, v)
        })
        .collect()
    });
    all.iter()
        .find(|(name, _)| *name == task_type)
        .map(|(_, v)| v)
        .with_context(|| format!("no schema for task type {task_type:?}"))
}

pub fn collector_types() -> &'static [&'static str] {
    &["discover", "harvest_meta", "harvest_captions", "harvest_comments", "harvest_chat"]
}

#[derive(Debug, serde::Serialize)]
pub struct ClaimedTask {
    pub id: i64,
    pub r#type: String,
    pub subject: String,
    pub input: Value,
}

/// Admin action: start a harvest wave. Idempotent while a discover task is open.
pub fn enqueue_wave(conn: &Connection, cfg: &Config, direction: &str) -> Result<Value> {
    if !["back", "forward"].contains(&direction) {
        bail!("direction must be 'back' or 'forward'");
    }
    db::meta_set(conn, "wave_direction", direction)?;
    let now = Timestamp::now().to_string();
    let existing: Option<String> = conn
        .query_row(
            "SELECT state FROM tasks WHERE type = 'discover' AND subject = ?1
             AND state IN ('pending', 'claimed')",
            [&cfg.channel.handle],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(state) = existing {
        return Ok(json!({ "discover": state, "note": "wave already open" }));
    }
    conn.execute(
        "INSERT INTO tasks (type, subject, state, created_at) VALUES ('discover', ?1, 'pending', ?2)
         ON CONFLICT(type, subject) DO UPDATE SET
             state = 'pending', error = NULL, lease_until = NULL, claimed_by = NULL,
             attempt = 0, done_at = NULL, created_at = excluded.created_at",
        params![cfg.channel.handle, now],
    )?;
    Ok(json!({ "discover": "enqueued" }))
}

/// Atomic claim with lease; expired leases are reclaimable. Over-attempted
/// tasks fail permanently and loudly.
pub fn claim(conn: &Connection, cfg: &Config, worker: &str, limit: usize) -> Result<Vec<ClaimedTask>> {
    let types: Vec<&str> = match worker {
        "collector" => collector_types().to_vec(),
        other => bail!("unknown worker kind {other:?}"),
    };
    let now = Timestamp::now();
    let lease = now
        .checked_add(LEASE_MINUTES.minutes())
        .context("lease overflow")?
        .to_string();
    let now_s = now.to_string();

    conn.execute(
        "UPDATE tasks SET state = 'failed', error = 'max attempts exceeded'
         WHERE state = 'claimed' AND lease_until < ?1 AND attempt >= ?2",
        params![now_s, MAX_ATTEMPTS],
    )?;

    // Explicitly numbered placeholders — SQLite refuses to mix ?N with bare ?.
    let placeholders = (0..types.len()).map(|i| format!("?{}", i + 4)).collect::<Vec<_>>().join(",");
    let limit_ph = types.len() + 4;
    let sql = format!(
        "UPDATE tasks SET state = 'claimed', lease_until = ?1, claimed_by = ?2, attempt = attempt + 1
         WHERE id IN (
             SELECT id FROM tasks
             WHERE (state = 'pending' OR (state = 'claimed' AND lease_until < ?3))
               AND type IN ({placeholders})
             ORDER BY id LIMIT ?{limit_ph}
         )
         RETURNING id, type, subject"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut bind: Vec<&dyn rusqlite::ToSql> = vec![&lease, &worker, &now_s];
    for t in &types {
        bind.push(t);
    }
    let limit = limit.clamp(1, 20) as i64;
    bind.push(&limit);
    let rows = stmt.query_map(&bind[..], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;

    let mut out = vec![];
    for row in rows {
        let (id, r#type, subject) = row?;
        let input = match r#type.as_str() {
            "discover" => json!({ "handle": cfg.channel.handle, "pace_ms": cfg.harvest.pace_ms }),
            _ => json!({ "yt_id": subject, "pace_ms": cfg.harvest.pace_ms }),
        };
        out.push(ClaimedTask { id, r#type, subject, input });
    }
    Ok(out)
}

/// Validate against the task type's schema, apply in one transaction, mark done.
pub fn submit(conn: &mut Connection, cfg: &Config, task_id: i64, result: &Value) -> Result<Value> {
    let (r#type, subject, state): (String, String, String) = conn
        .query_row(
            "SELECT type, subject, state FROM tasks WHERE id = ?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .with_context(|| format!("no task {task_id}"))?;
    if state != "claimed" {
        bail!("task {task_id} is {state}, not claimed");
    }

    let v = validator(&r#type)?;
    let errors: Vec<String> = v.iter_errors(result).map(|e| format!("{e}")).take(5).collect();
    if !errors.is_empty() {
        bail!("schema validation failed for {}: {}", r#type, errors.join("; "));
    }

    let tx = conn.transaction()?;
    let summary = match r#type.as_str() {
        "discover" => apply_discover(&tx, cfg, result)?,
        "harvest_meta" => apply_meta(&tx, &subject, result)?,
        "harvest_captions" => apply_captions(&tx, &subject, result)?,
        "harvest_comments" => apply_comments(&tx, &subject, result)?,
        "harvest_chat" => apply_chat(&tx, &subject, result)?,
        other => bail!("no apply logic for task type {other:?}"),
    };
    tx.execute(
        "UPDATE tasks SET state = 'done', done_at = ?1, error = NULL WHERE id = ?2",
        params![Timestamp::now().to_string(), task_id],
    )?;
    maybe_finish_wave(&tx)?;
    tx.commit()?;
    Ok(summary)
}

pub fn fail(conn: &Connection, task_id: i64, error: &str) -> Result<()> {
    let updated = conn.execute(
        "UPDATE tasks SET state = CASE WHEN attempt >= ?3 THEN 'failed' ELSE 'pending' END,
                          error = ?2, lease_until = NULL, claimed_by = NULL
         WHERE id = ?1 AND state = 'claimed'",
        params![task_id, error, MAX_ATTEMPTS],
    )?;
    if updated == 0 {
        bail!("no claimed task {task_id}");
    }
    tracing::warn!(task_id, error, "task failed by worker");
    Ok(())
}

pub fn counts(conn: &Connection) -> Result<Value> {
    let mut stmt =
        conn.prepare("SELECT type, state, COUNT(*) FROM tasks GROUP BY type, state")?;
    let mut map = serde_json::Map::new();
    for row in stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })? {
        let (t, s, n) = row?;
        map.entry(t)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("object")
            .insert(s, json!(n));
    }
    let videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))?;
    let transcripts: i64 = conn.query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))?;
    let comments: i64 = conn.query_row("SELECT COUNT(*) FROM comments", [], |r| r.get(0))?;
    let author_replies: i64 =
        conn.query_row("SELECT COUNT(*) FROM comments WHERE is_author = 1", [], |r| r.get(0))?;
    Ok(json!({
        "tasks": map, "videos": videos, "transcripts": transcripts,
        "comments": comments, "author_replies": author_replies,
    }))
}

// ---- apply logic ------------------------------------------------------------

fn apply_discover(conn: &Connection, cfg: &Config, result: &Value) -> Result<Value> {
    let now = Timestamp::now();
    let now_s = now.to_string();
    let channel_id = result["channel_id"].as_str().expect("validated");
    let videos = result["videos"].as_array().expect("validated");

    // The channel's own id: used to flag the professor's authored replies (SPEC §6).
    db::meta_set(conn, "channel_id", channel_id)?;

    for v in videos {
        let kind = v["kind"].as_str().expect("validated");
        // Streams have a chat replay to fetch; plain videos never do.
        let chat_state = if kind == "stream" { "pending" } else { "na" };
        conn.execute(
            "INSERT INTO videos (yt_id, channel_id, kind, title, approx_published, duration_s, chat_state, discovered_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(yt_id) DO UPDATE SET
                 title = excluded.title,
                 approx_published = COALESCE(videos.published_at, excluded.approx_published),
                 duration_s = COALESCE(excluded.duration_s, videos.duration_s)",
            params![
                v["yt_id"].as_str(),
                channel_id,
                kind,
                v["title"].as_str(),
                v["approx_published"].as_str(),
                v["duration_s"].as_i64(),
                chat_state,
                now_s,
            ],
        )?;
    }

    // Window selection: batching heuristic over approximate dates; correctness
    // lives in per-video done-flags, so boundary fuzz is harmless (SPEC §5).
    let direction =
        db::meta_get(conn, "wave_direction")?.unwrap_or_else(|| "back".into());
    let (start, end) = match direction.as_str() {
        "forward" => {
            let start = db::meta_get(conn, "wm_newest")?
                .unwrap_or_else(|| now.to_string());
            (start, now.to_string())
        }
        _ => {
            let anchor = db::meta_get(conn, "wm_oldest")?
                .map(|s| s.parse::<Timestamp>())
                .transpose()
                .context("corrupt wm_oldest")?
                .unwrap_or(now);
            let start = anchor
                .to_zoned(jiff::tz::TimeZone::UTC)
                .checked_sub((cfg.harvest.window_days as i64).days())
                .context("window underflow")?
                .timestamp();
            (start.to_string(), anchor.to_string())
        }
    };
    db::meta_set(conn, "wave_start", &start)?;
    db::meta_set(conn, "wave_end", &end)?;

    let mut stmt = conn.prepare(
        "SELECT yt_id, kind, meta_done, captions_state, comments_state, chat_state FROM videos
         WHERE approx_published >= ?1 AND approx_published < ?2
         ORDER BY approx_published DESC LIMIT ?3",
    )?;
    let picked: Vec<(String, String, i64, String, String, String)> = stmt
        .query_map(params![start, end, WAVE_CAP as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?
        .collect::<std::result::Result<_, _>>()?;

    let mut created = 0;
    for (yt_id, kind, meta_done, captions_state, comments_state, chat_state) in &picked {
        if *meta_done == 0 {
            created += enqueue_video_task(conn, "harvest_meta", yt_id, &now_s)?;
        }
        if captions_state == "pending" {
            created += enqueue_video_task(conn, "harvest_captions", yt_id, &now_s)?;
        }
        if comments_state == "pending" {
            created += enqueue_video_task(conn, "harvest_comments", yt_id, &now_s)?;
        }
        // Live chat replay exists only for streams.
        if kind == "stream" && chat_state == "pending" {
            created += enqueue_video_task(conn, "harvest_chat", yt_id, &now_s)?;
        }
    }
    tracing::info!(discovered = videos.len(), window = %format!("{start}..{end}"), tasks = created, "discover applied");
    Ok(json!({ "discovered": videos.len(), "window": [start, end], "tasks_created": created }))
}

fn enqueue_video_task(conn: &Connection, r#type: &str, yt_id: &str, now: &str) -> Result<i64> {
    // Re-enqueue only genuinely unfinished work; done tasks stay done.
    let n = conn.execute(
        "INSERT INTO tasks (type, subject, state, created_at) VALUES (?1, ?2, 'pending', ?3)
         ON CONFLICT(type, subject) DO UPDATE SET
             state = 'pending', error = NULL, lease_until = NULL, claimed_by = NULL, attempt = 0
         WHERE tasks.state = 'failed'",
        params![r#type, yt_id, now],
    )?;
    Ok(n as i64)
}

fn apply_meta(conn: &Connection, subject: &str, result: &Value) -> Result<Value> {
    let yt_id = result["yt_id"].as_str().expect("validated");
    if yt_id != subject {
        bail!("result yt_id {yt_id} does not match task subject {subject}");
    }
    let updated = conn.execute(
        "UPDATE videos SET title = ?2, description = ?3, published_at = ?4,
                           duration_s = COALESCE(?5, duration_s),
                           channel_id = COALESCE(?6, channel_id),
                           view_count = ?7, meta_done = 1
         WHERE yt_id = ?1",
        params![
            yt_id,
            result["title"].as_str(),
            result["description"].as_str().unwrap_or(""),
            result["published_at"].as_str(),
            result["duration_s"].as_i64(),
            result["channel_id"].as_str(),
            result["view_count"].as_i64(),
        ],
    )?;
    if updated == 0 {
        bail!("meta for unknown video {yt_id} — discover first");
    }
    if let Some(raw) = result["raw_player"].as_str() {
        store_raw(conn, yt_id, "meta_player", raw.as_bytes())?;
    }
    Ok(json!({ "video": yt_id, "meta": "ok" }))
}

fn apply_captions(conn: &Connection, subject: &str, result: &Value) -> Result<Value> {
    let yt_id = result["yt_id"].as_str().expect("validated");
    if yt_id != subject {
        bail!("result yt_id {yt_id} does not match task subject {subject}");
    }
    if result["none"].as_bool() == Some(true) {
        let n = conn.execute(
            "UPDATE videos SET captions_state = 'none' WHERE yt_id = ?1",
            [yt_id],
        )?;
        if n == 0 {
            bail!("captions for unknown video {yt_id}");
        }
        return Ok(json!({ "video": yt_id, "captions": "none" }));
    }
    let segments = &result["segments"];
    let compact = serde_json::to_vec(segments)?;
    let packed = crate::raw::compress(&compact)?;
    let n = conn.execute(
        "INSERT INTO transcripts (video_id, source, lang, segments_zstd, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(video_id) DO UPDATE SET source = excluded.source, lang = excluded.lang,
             segments_zstd = excluded.segments_zstd, updated_at = excluded.updated_at",
        params![
            yt_id,
            result["source"].as_str(),
            result["lang"].as_str(),
            packed,
            Timestamp::now().to_string(),
        ],
    )?;
    debug_assert!(n > 0);
    let updated = conn.execute(
        "UPDATE videos SET captions_state = 'have' WHERE yt_id = ?1",
        [yt_id],
    )?;
    if updated == 0 {
        bail!("captions for unknown video {yt_id} — discover first");
    }
    store_raw(conn, yt_id, "captions_json3", &compact)?;
    let count = segments.as_array().map(|a| a.len()).unwrap_or(0);
    Ok(json!({ "video": yt_id, "captions": "have", "segments": count }))
}

fn apply_comments(conn: &Connection, subject: &str, result: &Value) -> Result<Value> {
    let yt_id = result["yt_id"].as_str().expect("validated");
    if yt_id != subject {
        bail!("result yt_id {yt_id} does not match task subject {subject}");
    }
    if conn.query_row("SELECT 1 FROM videos WHERE yt_id = ?1", [yt_id], |_| Ok(())).optional()?.is_none() {
        bail!("comments for unknown video {yt_id} — discover first");
    }
    if result["disabled"].as_bool() == Some(true) {
        conn.execute("UPDATE videos SET comments_state = 'none' WHERE yt_id = ?1", [yt_id])?;
        return Ok(json!({ "video": yt_id, "comments": "disabled" }));
    }
    let channel_id = db::meta_get(conn, "channel_id")?;
    let now = Timestamp::now().to_string();
    let mut author_replies = 0i64;
    let comments = result["comments"].as_array().expect("validated");
    for c in comments {
        let acid = c["author_channel_id"].as_str();
        let is_author = channel_id.as_deref().is_some() && acid == channel_id.as_deref();
        if is_author {
            author_replies += 1;
        }
        conn.execute(
            "INSERT INTO comments (yt_id, video_id, parent_id, author_channel_id, author_name,
                                   text, like_count, published_at, is_author, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(yt_id) DO UPDATE SET text = excluded.text,
                 like_count = excluded.like_count, is_author = excluded.is_author",
            params![
                c["id"].as_str(),
                yt_id,
                c["parent_id"].as_str(),
                acid,
                c["author_name"].as_str().unwrap_or(""),
                c["text"].as_str().unwrap_or(""),
                c["like_count"].as_i64(),
                c["published_at"].as_str(),
                is_author as i64,
                now,
            ],
        )?;
    }
    conn.execute("UPDATE videos SET comments_state = 'have' WHERE yt_id = ?1", [yt_id])?;
    Ok(json!({ "video": yt_id, "comments": comments.len(), "author_replies": author_replies }))
}

fn apply_chat(conn: &Connection, subject: &str, result: &Value) -> Result<Value> {
    let yt_id = result["yt_id"].as_str().expect("validated");
    if yt_id != subject {
        bail!("result yt_id {yt_id} does not match task subject {subject}");
    }
    if conn.query_row("SELECT 1 FROM videos WHERE yt_id = ?1", [yt_id], |_| Ok(())).optional()?.is_none() {
        bail!("chat for unknown video {yt_id} — discover first");
    }
    if result["unavailable"].as_bool() == Some(true) {
        conn.execute("UPDATE videos SET chat_state = 'none' WHERE yt_id = ?1", [yt_id])?;
        return Ok(json!({ "video": yt_id, "chat": "unavailable" }));
    }
    let channel_id = db::meta_get(conn, "channel_id")?;
    let now = Timestamp::now().to_string();
    let mut author_msgs = 0i64;
    let messages = result["messages"].as_array().expect("validated");
    for m in messages {
        let acid = m["author_channel_id"].as_str();
        let is_author = channel_id.as_deref().is_some() && acid == channel_id.as_deref();
        if is_author {
            author_msgs += 1;
        }
        conn.execute(
            "INSERT INTO chat_messages (yt_id, video_id, offset_ms, author_channel_id,
                                        author_name, text, is_author, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(yt_id) DO UPDATE SET text = excluded.text, is_author = excluded.is_author",
            params![
                m["id"].as_str(),
                yt_id,
                m["offset_ms"].as_i64(),
                acid,
                m["author_name"].as_str().unwrap_or(""),
                m["text"].as_str().unwrap_or(""),
                is_author as i64,
                now,
            ],
        )?;
    }
    conn.execute("UPDATE videos SET chat_state = 'have' WHERE yt_id = ?1", [yt_id])?;
    Ok(json!({ "video": yt_id, "chat_messages": messages.len(), "author_messages": author_msgs }))
}

fn store_raw(conn: &Connection, yt_id: &str, kind: &str, content: &[u8]) -> Result<()> {
    let packed = crate::raw::compress(content)?;
    conn.execute(
        "INSERT INTO raw_docs (video_id, kind, content, fetched_at) VALUES (?1, ?2, ?3, ?4)",
        params![yt_id, kind, packed, Timestamp::now().to_string()],
    )?;
    Ok(())
}

/// When no collector work remains open, the wave is over: advance watermarks
/// and stamp last_gathered_at.
fn maybe_finish_wave(conn: &Connection) -> Result<()> {
    let open: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE state IN ('pending', 'claimed')",
        [],
        |r| r.get(0),
    )?;
    if open > 0 {
        return Ok(());
    }
    let (Some(start), Some(end)) =
        (db::meta_get(conn, "wave_start")?, db::meta_get(conn, "wave_end")?)
    else {
        return Ok(());
    };
    let direction = db::meta_get(conn, "wave_direction")?.unwrap_or_else(|| "back".into());
    if direction == "forward" {
        db::meta_set(conn, "wm_newest", &end)?;
        if db::meta_get(conn, "wm_oldest")?.is_none() {
            db::meta_set(conn, "wm_oldest", &start)?;
        }
    } else {
        db::meta_set(conn, "wm_oldest", &start)?;
        if db::meta_get(conn, "wm_newest")?.is_none() {
            db::meta_set(conn, "wm_newest", &end)?;
        }
    }
    db::meta_set(conn, "last_gathered_at", &Timestamp::now().to_string())?;
    tracing::info!(window = %format!("{start}..{end}"), direction, "harvest wave complete, watermarks advanced");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn cfg() -> Config {
        toml::from_str(
            r#"
            [channel]
            handle = "@test"
            [server]
            bind = "127.0.0.1:0"
            data_dir = "unused"
            "#,
        )
        .unwrap()
    }

    fn discover_result(ids: &[(&str, &str)]) -> Value {
        // Dates one day apart, newest first, inside the current window.
        let now = Timestamp::now();
        let videos: Vec<Value> = ids
            .iter()
            .enumerate()
            .map(|(i, (id, kind))| {
                let at = now
                    .checked_sub(((i as i64 + 1) * 24).hours())
                    .unwrap()
                    .to_string();
                json!({ "yt_id": id, "title": format!("t{i}"), "kind": kind,
                        "approx_published": at, "duration_s": 60 })
            })
            .collect();
        json!({ "channel_id": "UC12345678901234567890AB", "videos": videos })
    }

    #[test]
    fn full_wave_lifecycle() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let cfg = cfg();
        db.with(|c| {
            enqueue_wave(c, &cfg, "back")?;
            // Idempotent while open.
            let again = enqueue_wave(c, &cfg, "back")?;
            assert_eq!(again["note"], "wave already open");

            let claimed = claim(c, &cfg, "collector", 10)?;
            assert_eq!(claimed.len(), 1);
            assert_eq!(claimed[0].r#type, "discover");
            let discover_id = claimed[0].id;

            // Invalid submission is rejected wholesale.
            let bad = json!({ "channel_id": "nope", "videos": [] });
            assert!(submit(c, &cfg, discover_id, &bad).is_err());

            let ok = discover_result(&[("aaaaaaaaaaa", "video"), ("bbbbbbbbbbb", "stream")]);
            let summary = submit(c, &cfg, discover_id, &ok)?;
            // video: meta+captions+comments (3); stream: +chat (4) => 7.
            assert_eq!(summary["tasks_created"], 7);

            // Claim and finish them all; the stream's comments carry an author reply.
            let batch = claim(c, &cfg, "collector", 20)?;
            assert_eq!(batch.len(), 7);
            for t in &batch {
                let result = match t.r#type.as_str() {
                    "harvest_meta" => json!({
                        "yt_id": t.subject, "title": "T", "published_at": "2026-07-01T00:00:00Z",
                        "duration_s": 61, "channel_id": "UC12345678901234567890AB",
                        "view_count": 5, "raw_player": "{}"
                    }),
                    "harvest_captions" => json!({
                        "yt_id": t.subject, "lang": "ru", "source": "asr",
                        "segments": [ { "t_ms": 0, "d_ms": 1000, "text": "привет" } ]
                    }),
                    "harvest_comments" => json!({
                        "yt_id": t.subject, "comments": [
                            { "id": format!("c-{}", t.subject), "text": "вопрос?",
                              "author_channel_id": "UCsomebodyElse0123456789", "author_name": "fan" },
                            { "id": format!("a-{}", t.subject), "text": "ответ", "parent_id": format!("c-{}", t.subject),
                              "author_channel_id": "UC12345678901234567890AB", "author_name": "prof" }
                        ]
                    }),
                    "harvest_chat" => json!({
                        "yt_id": t.subject, "messages": [
                            { "id": "m1", "offset_ms": 5000, "text": "привет из чата",
                              "author_channel_id": "UCviewer0123456789012345", "author_name": "v" }
                        ]
                    }),
                    other => panic!("unexpected {other}"),
                };
                submit(c, &cfg, t.id, &result)?;
            }

            // Wave closed: watermarks set, transcripts stored, videos updated.
            assert!(db::meta_get(c, "wm_oldest")?.is_some());
            assert!(db::meta_get(c, "last_gathered_at")?.is_some());
            let n: i64 = c.query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))?;
            assert_eq!(n, 2);
            // Author-reply detection: exactly the two "prof" comments flagged.
            let author: i64 =
                c.query_row("SELECT COUNT(*) FROM comments WHERE is_author = 1", [], |r| r.get(0))?;
            assert_eq!(author, 2);
            let chat: i64 = c.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))?;
            assert_eq!(chat, 1); // only the stream got a chat task
            let (meta_done, cap): (i64, String) = c.query_row(
                "SELECT meta_done, captions_state FROM videos WHERE yt_id = 'aaaaaaaaaaa'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            assert_eq!((meta_done, cap.as_str()), (1, "have"));

            // Re-discover creates nothing new — everything is done.
            enqueue_wave(c, &cfg, "back")?;
            let re = claim(c, &cfg, "collector", 10)?;
            let summary = submit(c, &cfg, re[0].id, &ok)?;
            assert_eq!(summary["tasks_created"], 0);
            Ok(())
        })
    }

    #[test]
    fn fail_requeues_then_exhausts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let db = Db::open(dir.path())?;
        let cfg = cfg();
        db.with(|c| {
            enqueue_wave(c, &cfg, "back")?;
            for attempt in 1..=MAX_ATTEMPTS {
                let t = claim(c, &cfg, "collector", 1)?;
                assert_eq!(t.len(), 1, "attempt {attempt}");
                fail(c, t[0].id, "boom")?;
            }
            // Attempts exhausted → permanently failed, nothing claimable.
            assert!(claim(c, &cfg, "collector", 1)?.is_empty());
            let state: String =
                c.query_row("SELECT state FROM tasks", [], |r| r.get(0))?;
            assert_eq!(state, "failed");
            Ok(())
        })
    }
}
