# SPEC.md — cyberaancha

Status: **DRAFT v0.3** (2026-07-19). Architecture settled (hub-and-edges); P0 research done — facts baked in (see `research/p0-findings.md`); remaining opens — §19.

## 1. Mission

Turn the public knowledge of Prof. Ancha Baranova (YouTube [@AnchaBaranovaProf](https://www.youtube.com/@AnchaBaranovaProf) — videos, live streams, stream chats, comments) into a curated, cross-referenced knowledge base ("the wiki"), continuously enriched, exposed through:

1. **Web admin panel** (MVP) — browse, search, edit; answer the open-questions queue; test-bot tab that answers exactly like the future TG bot;
2. **MCP endpoint** — research access from Claude (hers and ours): search tools + `article://` resources;
3. **Telegram bot** (post-MVP) — answers in her community group with YouTube links + timestamps and her distilled opinion.

The base is **scientific reference material** — much of it is information for medical doctors' practice. The system quotes and attributes the professor; it **never synthesizes new medical advice**. Enforced structurally: production has no LLM.

## 2. Parts & actors

| Part | Where | Role |
|---|---|---|
| **Server** `aancha-server` | n1.serezhkin.com, Docker, `~vany/aancha` | Canonical storage (raw + processed), task queue, validation, tantivy index, REST + MCP + SPA, backups. **Never talks to YouTube, never calls an LLM.** |
| **Collector** | browser JS, runs in youtube.com page context | Harvests metadata, captions, comments, chat replays using the browser session; posts JSON to server. Bookmarklet first; console snippet fallback. |
| **Preparer** | Claude Code on Vany's Mac + `prompts/` + `scripts/` | Claims tasks over REST/MCP; transcribes locally (yt-dlp audio + whisper.cpp); extracts/integrates knowledge with agent reasoning guided by versioned prompts. **Not a binary.** |
| Ancha (owner role) | panel + MCP | Browses, edits, answers questions. Later: runs collector for owner-only data. No OAuth from her, ever. |
| Vany + Claude (admin role) | panel, SSH, Claude Code | Operate everything. |
| Community | TG group (post-MVP) | Ask the bot. |

## 3. Hard constraints

- **C1 — No LLM in production.** Server runtime = fulltext + templates. All intelligence precomputed by the preparer.
- **C2 — Server never fetches YouTube.** Datacenter IPs get throttled/blocked; all harvesting happens in browser sessions (collector) or on the Mac (audio). One fetch path per source, at the edge.
- **C3 — Heavy compute on the Mac.** Audio download + Whisper local; audio never uploaded, only transcript JSON.
- **C4 — No owner credentials.** Owner-only data comes only via collector run by Ancha herself in her Chrome.
- **C5 — Polite fetching.** Paced, jittered, resumable, chunked (`chunk_size` config, default 10 videos/wave).
- **C6 — Channel is config.** One channel per instance. Start: `@vanyserezhkin` (test, messier data — good stress test). Production: wipe (restore/drop) and point at `@AnchaBaranovaProf`. `channel_id` stored on rows anyway (cheap multi-channel option later).
- **C7 — Modest host.** n1: 1 vCPU, 457 MB RAM, 8.6 GB disk (1.7 GB free). Container memory-capped, tantivy writer heap small (≤128 MB), reindex niced, raw blobs zstd-compressed. Disk headroom must be revisited before full Ancha backfill (§19).
- **C8 — Security.** TLS via existing host nginx + Let's Encrypt; app binds 127.0.0.1:8087 only. Basic auth (roles owner/admin) for panel; separate bearer tokens for collector, preparer, MCP.

## 4. System shape

```
                 ┌──────────────────────────────────────────┐
                 │ aancha-server @ n1 (127.0.0.1:8087)      │
                 │  SQLite (canonical: raw+processed+queue) │
                 │  tantivy (articles idx, transcripts idx) │
                 │  REST ─ MCP (same handlers) ─ SPA        │
                 │  validation: JSON Schema, ref integrity  │
                 │  daily backup tarball + restore cmd      │
                 └────▲──────────────▲──────────────▲───────┘
        nginx TLS ────┤              │              │
                      │              │              │
        Collector (browser JS   Preparer (Claude    Consumers: panel users,
        in youtube.com context, on Mac: whisper,    her Claude via MCP,
        session auth, paced)    reasoning, prompts) later TG bot
```

Flow: panel button **"Gather"** enqueues a harvest wave → collector (in whoever's browser) claims tasks, posts raw JSON → server stores raw, derives next tasks → preparer claims transcribe/extract/integrate tasks, submits validated results → server updates KB, rebuilds index, advances watermarks. Panel shows two clocks: **last gathered** / **last processed & indexed**, plus per-stage queue counts.

## 5. Task queue (core mechanism)

Table `tasks`: `id, type, subject (video_id|…), state (pending|claimed|done|failed), lease_until, claimed_by, attempt, payload_json, result_ref, error, created_at, done_at`.

- **Claim**: `GET /api/tasks?worker=collector&limit=N` — atomic claim with lease (e.g. 30 min); expired leases return to pending. Idempotent resubmit.
- **Validate**: every submission checked against the task type's JSON Schema (`schemas/*.json` in repo — single source of truth, referenced by prompts too) + referential integrity + size limits. Reject, don't repair.
- **Provenance**: submissions record prompt version (preparer tasks).

| Type | Worker | Granularity | Produces |
|---|---|---|---|
| `discover` | collector | channel (videos+streams tabs) | video list → upsert `videos`, spawn per-video tasks past watermark |
| `harvest_meta` | collector | video | description, chapters, stats |
| `harvest_captions` | collector | video | caption tracks (RU/EN) as timed segments, or `none` |
| `harvest_comments` | collector | video | full comment threads walk |
| `harvest_chat` | collector | stream | live chat replay walk |
| `transcribe` | preparer — **plain script, no LLM** | video | Whisper segments (spawned when captions missing; or by `extract` verdict `needs_transcription` when captions are garbage) |
| `extract` | preparer — Claude | video (transcript + meta + comments + chat) | topics/facts/stances candidates, QA pairs, alias suggestions, style notes — stateless per video |
| `integrate` | preparer — Claude | one extraction | create/merge/link decisions against live KB (agent searches first), final article updates, contradictions → questions. **Serialized** (one active) to avoid merge races |
| *(internal)* index, backup | server | — | tantivy rebuild after integrate batch; daily tarball |

## 6. Knowledge model

### Articles (the wiki; topics)
- `paragraph_ru` — ≤ ~800 chars, the bot answer, professor's voice, contains current distilled opinion;
- `story_md` — full narrative, chronological, sourced; for her reading/verification;
- **stances** — dated opinion timeline: (when, source ref video+t/comment/chat/panel, text, authority). "Переосмыслено в <link>" comes from here; current opinion is recency-weighted;
- **aliases** — build-time recall boosters: medical + colloquial synonyms, misspellings, latin/EN drug & gene names, typical question phrasings. This is what makes «боль в заднице» find «геморрой» in pure BM25. Misses from the test tab / future bot feed alias fixes each cycle;
- **cross-links** — related/parent/contradicts edges;
- links per article: **≤ 5** rendered in answers.

### Facts
Atomic, attached to articles, each with provenance (source ref, date, confidence) and **authority**:
`panel answer/edit (her, explicit) > her comment reply > her spoken words > inferred from chat/comments`.

### Questions queue
Merge-time contradictions, low-confidence gaps, popular unanswered queries → panel "Questions" tab with context + answer field. Her answers become top-authority facts next integrate.

### People (fan service, post-MVP UI)
Per YouTube/chat identity: handle, activity, their questions + her answers (QA pairs), agent-written summary of the relationship. **Collected and stored from day one** (cheap to keep, painful to re-mine); tab later.

### Style profile
Versioned artifact (meta), refreshed by preparer; guides paragraph/story writing. Not user-visible.

### Watermarks
videos/streams: latest processed publish date; comments: latest comment timestamp; chat: per-stream once. Advance transactionally on successful integrate.

## 7. Search & answer engine (production)

- **tantivy**, embedded; RU Snowball stemming + lowercase; EN/latin terms indexed as-is.
- **Articles index** (the bot's world): `title`×3, `aliases`×2.5, `paragraph`, `opinion`, `story`. One doc per article.
- **Transcripts index** (research): segment-level; panel + MCP only.
- Query: normalize → BM25 → threshold. Hit ⇒ answer template; miss ⇒ honest "не разбиралось" + logged to `queries` (feeds questions + aliases).
- Answer template (test tab now, TG later; ≤5 links):
  > про **«тема»** обсуждали в `<link&t>`, `<link&t>`; переосмыслено в `<link&t>`.
  > Мнение профессора: `<paragraph_ru>`
  > _Справочный материал по выступлениям проф. Барановой — не медицинская рекомендация._
- No embeddings in v1 (C1). Phase-2 option if recall disappoints: tiny local ONNX embedder. Noted, not planned.

## 8. REST surface (sketch)

```
auth: basic+role (panel/human) · bearer: collector | preparer | mcp
GET  /api/state                       clocks, watermarks, queue counts
POST /api/harvest/enqueue             admin: start harvest wave (chunked)
GET  /api/tasks?worker=&limit=        claim with lease
POST /api/tasks/{id}/result           schema-validated submit
POST /api/tasks/{id}/fail             error report
GET  /api/articles?q=                 search (panel/test/MCP share it)
GET|PUT /api/articles/{slug}          read | owner-edit (= top-authority fact)
GET  /api/videos, /api/videos/{id}    inventory + processing status
GET  /api/questions, POST /api/questions/{id}/answer
POST /api/test-query                  rendered bot answer (test tab)
GET  /api/backups                     list; scheduler is internal
POST /api/backups                     admin: immediate backup now
```

## 9. MCP

Same binary, HTTP at `/mcp`, bearer token; URL + token shown in panel System tab.
- **Tools**: `search_articles`, `get_article`, `search_transcripts`, `get_video`, `list_questions`, `answer_question`, `next_task`, `submit_result`, `kb_stats`.
- **Resources**: `article://<slug>`, `video://<yt_id>`, `person://<id>`.

## 10. Admin panel (SPA)

No-build: vendored Preact + htm, ES modules, vanilla CSS; embedded into the binary (rust-embed) — single deploy artifact.

Tabs: **Search/Browse** (wiki, cross-links, article view: paragraph/story/timeline/sources/facts; inline edit) · **Questions** (answer fields) · **Test** (query box → exact bot answer) · **Sources** (video inventory, per-stage status) · **System** (admin only: clocks, queue, collector launcher — bookmarklet drag-target + snippet copy + fresh token, MCP URL+token, backups, config view) · *(post-MVP)* **People**.

## 11. Collector design

- **Pure-fetch, zero DOM script injection** (P0-verified: YouTube CSP has no `connect-src`/`default-src` ⇒ page-context `fetch()` to our server is unrestricted; Trusted Types is enforced but only bites script-injection sinks, which we don't use). Console snippet therefore guaranteed; **bookmarklet vs snippet decided in testing** (Chrome CSP quirks).
- Panel System tab generates both (bookmarklet drag-target + snippet copy) with an embedded short-lived collector token; posts cross-origin to `/api/...` (CORS allows youtube.com origin, token-authenticated). Tampermonkey userscript = later polish for Ancha.
- Endpoints (P0-mapped, all same-origin from page context; YouTube serves no CORS headers to outsiders — which is exactly why the collector lives in the page): `get_transcript` → `captionTracks`/timedtext `fmt=json3` → `/player` (Android client) for captions; `ytInitialData` + `/youtubei/v1/next` continuations for comments; `live_chat_replay` continuation + `get_live_chat_replay` for chat; `/youtubei/v1/browse` for channel tabs.
- **Reads live `ytcfg`/`INNERTUBE_CONTEXT` from the page — never hardcodes client version** (rolls weekly); computes SAPISIDHASH when a session is present (logged-out works for public data). Paced with sleep+jitter, chunk-limited, task-resumable.
- MVP: run in **Vany's** browser (public data only). Ancha's session needed only for owner-only extras, later.

## 12. Preparer design

- Two kinds of Mac-side work, split by whether it needs a brain:
  - **Mechanical → standalone scripts** (`scripts/`): `transcribe_pending.sh` loops claim → yt-dlp audio → whisper.cpp (model `large-v3-turbo q5_0` + Metal, P0-verified: RU ≈ full v3 quality, ~15–20× realtime on the M4 Max; RU fine-tunes exist as quality fallback) → submit, fully unattended — a batch of 50 videos must not burn agent attention. curl + jq, no LLM.
  - **Judgment → Claude sessions**: `extract`, `integrate`, guided by `PREP.md` playbook + `prompts/*.md`.
- Interactive for development; **headless** (`claude -p "process next N tasks"`) for routine cycles; queue is worker-agnostic — a Batch-API worker can be added later if bulk demands, without server changes.
- Anthropic usage via subscription sessions; no API-orchestration code.

## 13. Config (TOML, `~vany/aancha/aancha.toml`)

```toml
[channel]  handle = "@vanyserezhkin"          # test; prod: @AnchaBaranovaProf
[server]   bind = "127.0.0.1:8087"  public_url = "https://aancha.serezhkin.com"
[harvest]  chunk_size = 10  pace_ms = 1500
[index]    writer_heap_mb = 96
[backup]   hour_utc = 3  keep = 3             # disk is tight; Vany archives off-box
[auth]     # bcrypt hashes: owner, admin; token hashes: collector, preparer, mcp
```

## 14. Security

- nginx (existing on n1): new vhost `aancha.serezhkin.com` → proxy 127.0.0.1:8087; certbot Let's Encrypt.
- App: basic auth owner/admin (bcrypt), per-purpose bearer tokens (rotatable via CLI), rate-limit + lockout on auth failures, CORS locked to youtube.com for collector endpoints only.
- Secrets in config file (0600), outside the image; nothing in git.

## 15. Deployment & ops (n1 facts, verified 2026-07-19)

- n1.serezhkin.com: Ubuntu 25.04 x86_64, 1 vCPU, 457 MB RAM, 8.6 GB disk (1.7 GB free), Docker 29.2.1 + Compose v5, nginx 1.26.3 (pattern vhost: music.serezhkin.com), certbot 2.11, syncthing running, per-project dirs in `~vany`.
- **Build**: on the Mac — `cargo-zigbuild` → static `x86_64-unknown-linux-musl` binary, SPA embedded; `FROM scratch` image (~15 MB) assembled server-side from scp'd binary (no registry, no server compiles).
- `~vany/aancha/`: `docker-compose.yml`, `aancha.toml`, `data/` (SQLite WAL, zstd raw blobs), `index/` (tantivy, rebuildable), `backups/`.
- Container: `mem_limit` (e.g. 256 MB), restart unless-stopped, binds 127.0.0.1 only.
- **Backups**: internal scheduler (no cron), daily `backups/aancha-YYYY-MM-DD.tar.gz` = SQLite snapshot + config (index excluded), keep 3 pruned; Vany archives off-box. **Immediate backup on demand**: `aancha-server backup` CLI subcommand + System-tab button (`POST /api/backups`) — same tarball format, timestamped, e.g. before risky operations. Restore: `aancha-server restore --latest --yes` (destructive: stop, wipe data, untar, reindex).
- Logs: tracing JSON → stdout → `docker logs`.

## 16. Costs & volumes (P0 facts, 2026-07-19)

- Inventories: **@vanyserezhkin 1300 videos / 1166 h + 41 streams / 155 h** (test channel is *bigger* than prod — chunked testing mandatory); **@AnchaBaranovaProf 767 videos / 494 h + 504 streams / 800 h ≈ 1294 h**.
- **Captions-first is load-bearing**: Whisper fills gaps only (~15 % missing/bad ⇒ ~190 h audio ≈ 10–13 h M4 Max compute); even full re-transcription of a channel ≈ a background weekend, not a blocker.
- Extraction (prod backfill): ~1300 h ≈ 15–20 M input tokens over ~1270 `extract` tasks — Claude subscription sessions, chunked over cycles. Production LLM: $0 by design. YouTube: $0, no API keys (C2). Hosting: existing box.

## 17. Risks

| Risk | Mitigation |
|---|---|
| YouTube CSP blocks bookmarklet | console snippet fallback (decided); Tampermonkey later. P0: snippet guaranteed (pure-fetch design) |
| innertube endpoint / client-version drift (weekly rolls) | collector reads live `ytcfg`, never hardcodes; small versioned collector, easy to patch |
| RU auto-caption quality | `extract` verdict `needs_transcription` → Whisper path |
| FTS recall ceiling | aggressive aliases + miss-log loop; phase-2 local embedder option |
| Over/under-merging of articles | integrate is agentic (searches KB first), serialized; contradicts-edges instead of destructive merges when unsure; panel visibility |
| n1 RAM/disk | C7 limits; **disk decision before Ancha backfill** (§19) |
| Agent slop into KB | server-side JSON Schema + integrity validation, reject-don't-repair |
| Basic auth brute force | strong creds, lockout, HTTPS only |

## 18. Phases

- **P0 — Research** ✅ *(2026-07-19)*: CSP verified (pure-fetch collector viable), innertube endpoints mapped, tantivy 0.26.1 RU ✓, rmcp 2.2.0 ✓, whisper turbo q5_0 ✓, inventories counted. Findings: `research/p0-findings.md`, summarized in MEMO.md. Crate-version picks happen at `cargo add` time (P1).
- **P1 — Server skeleton**: repo layout, axum + SQLite migrations + auth/roles + config + health; nginx vhost + LE; deploy pipeline (zigbuild → scratch image) to n1.
- **P2 — Queue + collector**: task engine, discover + harvest_captions + harvest_meta on @vanyserezhkin, bookmarklet/snippet, System tab minimal.
- **P3 — Harvest rest**: comments + chat replays.
- **P4 — Preparer loop**: PREP.md + prompts + schemas; transcribe fallback; extract + integrate; articles/facts/stances/aliases/questions land; tantivy indexes live.
- **P5 — Panel complete**: Browse/Search, article view+edit, Questions, Test tab, Sources.
- **P6 — MCP**: tools + resources; token in panel. ← **MVP line**
- **P7 — Ancha backfill**: disk decision, wipe/reconfig to her channel, chunked full-history harvest, questions loop live, her collector polish (Tampermonkey?).
- **P8 — Telegram bot**: teloxide, group mention handling, same answer engine. Post-MVP by decision.
- Later: People tab, TG-group history ingestion, headless scheduled cycles, embeddings fallback.

## 19. Open questions

1. **Disk before P7**: DO volume vs droplet resize vs aggressive raw-blob pruning after processing? (No decision needed until Ancha backfill.)
2. Backup retention `keep = 3` OK given 1.7 GB free? (Tarballs at her scale may reach ~100+ MB each.)
3. Port 8087 OK / any conflict I can't see? What is the service on :444?
4. Whisper model: default `large-v3-turbo`, fall back to `large-v3` where turbo quality disappoints — OK?

## 20. Decision log

- 2026-07-19 — Stack: Rust (axum, rusqlite, tantivy, rmcp; teloxide post-MVP), SQLite canonical on VPS, no-build Preact+htm SPA embedded via rust-embed. — *(V+C)*
- 2026-07-19 — **No LLM in production**; all intelligence precomputed. — *(V)*
- 2026-07-19 — **Hub-and-edges**: server = storage+queue+validation+index only; collector in browser page context; preparer = Claude Code + prompts + scripts, **no second binary**. — *(V+C)*
- 2026-07-19 — **Server never fetches YouTube** (no Data API keys at all); all harvest via browser/Mac edges. — *(V+C)*
- 2026-07-19 — MVP = admin panel with Test tab; **all Telegram postponed** past MVP. — *(V)*
- 2026-07-19 — Collector: bookmarklet first, console snippet fallback. — *(V)*
- 2026-07-19 — Two panel roles: owner (Ancha), admin (Vany). — *(V)*
- 2026-07-19 — Channel in config; test on @vanyserezhkin; ≤5 links per article answer. — *(V)*
- 2026-07-19 — Backups: service-internal daily dated tarball (no cron), keep-N, off-box archiving manual; `restore --latest` drop-and-restore command. — *(V+C)*
- 2026-07-19 — Deploy: n1.serezhkin.com `~vany/aancha`, existing nginx + Let's Encrypt, app on 127.0.0.1:8087, scratch image from Mac-built static musl binary. — *(V+C)*
- 2026-07-19 — Anthropic: no API-orchestration code; prompts as artifacts executed by Claude sessions. — *(V)*
- 2026-07-19 — Immediate backup: `aancha-server backup` CLI + System-tab button, same tarball as daily. — *(V)*
- 2026-07-19 — Mac-side split: mechanical tasks (transcribe) = unattended shell scripts; judgment tasks (extract/integrate) = Claude sessions. — *(V+C)*
