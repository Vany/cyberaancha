# SPEC.md — cyberaancha

Status: **DRAFT v0.1** (2026-07-19). Under discussion — see §17 Open questions. Nothing frozen.

## 1. Mission

Turn the public knowledge of Prof. Ancha Baranova (YouTube channel [@AnchaBaranovaProf](https://www.youtube.com/@AnchaBaranovaProf) — videos, live streams, stream chats, comments) into a curated, cross-referenced knowledge base ("the wiki"), continuously enriched, exposed through:

1. **Telegram bot** — answers questions in her community chat with YouTube links + timestamps and her distilled opinion;
2. **Web admin panel** — she browses, searches, edits, and answers the open-questions queue;
3. **MCP endpoint** — she and we research the base from Claude.

The base is **scientific reference material** — much of it is information for medical doctors' practice. The system quotes and attributes the professor; it **never synthesizes new medical advice**. This is enforced structurally: production has no LLM, so the bot can only serve prebaked, attributed content.

## 2. Actors

| Actor | Role |
|---|---|
| Ancha (the professor) | Source of truth. Uses admin panel + MCP. Answers open questions. Can run harvest scripts we send her (logged-in Chrome). **We cannot use her OAuth/credentials directly.** |
| Vany + Claude | Operators: run build cycles on the Mac, maintain the VPS. |
| TG community | Members of the `@anchabaranova` group; ask the bot by mention. |
| Bot | Serves prebaked answers only. |

## 3. Hard constraints

- **C1 — No LLM in production.** VPS runtime = fulltext search + templates only. All intelligence (extraction, topic merging, style, aliases, summaries) is precomputed at build time on the Mac.
- **C2 — Heavy compute on the MacBook.** Audio download and Whisper transcription run locally. Raw audio never leaves the Mac.
- **C3 — No owner auth.** Channel-owner-only data comes via "harvest bundle" scripts Ancha runs herself in Chrome (fallback channel, must be dead simple for her).
- **C4 — Polite fetching.** Respect YouTube throttling: paced sequential downloads, official Data API within quota wherever possible.
- **C5 — Small DO VPS, Docker.** One Rust service + SQLite + tantivy dir. Host: `aancha.serezhkin.com`.
- **C6 — Security.** HTTPS everywhere; admin panel behind basic auth; MCP and import API behind bearer tokens.

## 4. System shape

One Rust binary `aancha`, two runtime contexts:

```
MacBook (builder)                        DO VPS (server, Docker)
─────────────────                        ───────────────────────
aancha ingest / extract / push           aancha serve
  yt-dlp (metadata, subs, comments,        axum: admin SPA + API
          chat replay, audio)              teloxide: TG bot (long polling)
  whisper.cpp (local transcription)        rmcp: MCP over HTTP
  Claude API (extraction, merge,           tantivy: fulltext (RU stemming)
          aliases, style, profiles,        SQLite: canonical DB
          contradictions)                  Caddy: TLS termination
        │                                        ▲
        └── authenticated import API ────────────┘
            (pull state, push results)
```

**Data ownership: the VPS SQLite is canonical.** The builder pulls state (watermarks, professor's answers, unanswered TG questions), computes, pushes structured deltas. SQLite never travels; audio never leaves the Mac. Tantivy index is rebuilt server-side after each import (build into a fresh dir, atomic swap).

## 5. Data sources & acquisition

| Source | Method | Auth | Notes |
|---|---|---|---|
| Video metadata | YouTube Data API v3 (API key) + yt-dlp fallback | none | Both `/videos` and `/streams`. Descriptions included (often contain timecodes/links). |
| Subtitles (SRT/auto) | yt-dlp | none | First pass; RU auto-captions are weak on medical vocabulary. |
| Audio → transcript | yt-dlp audio + whisper.cpp on Mac | none | For videos with missing/bad subs. LLM samples sub quality to decide. |
| Comments | Data API v3 `commentThreads` (channel-wide listing for incremental sync) | API key | Cheap, official. `authorChannelId == channel` ⇒ **professor's authoritative answer**. |
| Live chat replay | scraper (`chat_downloader` or equiv.) | none | In scope for v1 (she actively answers chat). Paced politely. |
| Harvest bundle | JS tool we send Ancha; she runs it in logged-in Chrome; it downloads a JSON file she sends back | her session | Fallback for anything scraping can't reach. Import path in builder. |
| Professor's answers | admin panel "Questions" tab | basic auth | Replaces the earlier MD-file round-trip idea. |
| TG group history | — | — | **Phase 2+** (lots of her answers there; mixed in later). |

## 6. Knowledge model

### Topics (the wiki)
- **`paragraph_ru`** — one paragraph, ≤ ~800 chars: the TG answer, written at build time in the professor's voice. Contains her current distilled opinion.
- **`story_md`** — the full narrative: everything known on the topic, chronological, with sources. For her to read/verify in the panel.
- **`opinion timeline`** — dated *stances*: (when, where — video+timestamp / comment / chat / panel-answer, what she said, authority). The current opinion is synthesized recency-weighted; "was rethought in <link>" comes from here.
- **`aliases`** — build-time generated recall boosters: medical + colloquial synonyms, common misspellings, latin/EN drug and gene names, typical question phrasings. This is what makes pure FTS find «геморрой» from «боль в заднице».
- **cross-links** — related/parent/contradicts edges between topics.

### Facts
Atomic statements attached to topics. Each carries **provenance**: source ref (video+t / comment id / chat msg / panel answer), date, confidence, and **authority**:

`panel answer (her, explicit) > her comment reply > her spoken words (transcript) > inferred from chat/comments`

### Contradictions → Questions queue
At merge time the builder LLM compares incoming facts against the topic's existing facts. Conflicts (and low-confidence gaps, and popular unanswered TG questions) become entries in the **questions queue** with context. She answers them in the panel; answers enter the next cycle as top-authority facts.

### User profiles (fan service)
Per YouTube/chat identity: handle, first/last seen, activity counts, their questions + her answers to them (Q&A pairs), and a build-time LLM summary — "who this person is, history of the relationship". For her recall, panel + MCP only. TG-side profiles: phase 2 (requires reading full group traffic — separate decision).

### Style profile
Build artifact (versioned prompt/notes) describing her voice; used by the builder when writing paragraphs and stories. Not user-visible.

### Watermarks
- videos/streams: latest processed publish date;
- comments: latest comment timestamp (channel-wide);
- chat: per-video, fetched once after stream ends.

## 7. Search (production)

- **tantivy**, embedded, Russian stemming (rust-stemmers Snowball) + lowercase; EN terms indexed as-is (drug/gene names).
- One doc per **topic**: fields `title` (boost ×3), `aliases` (×2.5), `paragraph`, `opinion`, `story`. Bot queries this index only.
- Second index over **transcript segments** — admin panel + MCP research only, not the bot.
- Query pipeline: normalize → stem → BM25 → threshold. Below threshold ⇒ honest "not covered" + log to `tg_queries` (feeds the questions queue and alias improvements).
- **No embeddings in v1** (C1). If FTS recall proves insufficient, phase-2 option: tiny local embedding model (ONNX) on the VPS — noted, not planned.

## 8. Telegram bot

- Lives in the `@anchabaranova` group; triggers on `@aanchabot <question>` mention. (DMs: open question Q2.)
- Interaction language: **Russian**. Sources stored in original language; paragraphs prebaked in Russian.
- Reply template (structure, wording tuned later):
  > `@user` про **«тема»** обсуждали в `<link&t>`, `<link&t>`; переосмыслено в `<link&t>`.
  > Мнение профессора: `<paragraph_ru>`
  > _Справочный материал по выступлениям проф. Барановой — не медицинская рекомендация._
- Link cap per reply (proposal: ≤3 + 1 "rethought" link). Multi-topic hits: best topic + "смежные темы: …" one-liners.
- No hits ⇒ "профессор это ещё не разбирала" + question logged.
- Per-user rate limit; ignore non-mention traffic (privacy mode stays on in v1).

## 9. Admin panel (SPA)

Served by the same binary at `https://aancha.serezhkin.com/`, basic auth.

Tabs:
1. **Search / Browse** — wiki with cross-links; topic view: paragraph, story, opinion timeline, sources (clickable links w/ timestamps), facts with provenance. Inline edit (edits = top-authority facts).
2. **Questions** — open questions with context + answer fields (your Q9 decision). Answered items feed the next cycle.
3. **People** — user profiles, searchable.
4. **Sources** — video/stream inventory, processing status per stage.
5. **System** — watermarks, last cycle stats, **MCP URL + token** (Q10), health.

Frontend: no-build SPA — vendored Preact + htm (~4 KB, ES modules, no toolchain), vanilla CSS. (Q12: my pick; trivially replaceable.)

## 10. MCP

- Same binary, HTTP (streamable) at `/mcp`, bearer token; URL+token displayed in the panel so she can paste it into Claude.
- Tools (read-mostly, no LLM server-side): `search_topics`, `get_topic`, `search_transcripts`, `get_video`, `list_questions`, `answer_question`, `search_people`, `get_person`, `kb_stats`.

## 11. Enrichment cycle

`aancha cycle` on the Mac (also = initial backfill with empty watermarks, chunked politely):

1. **pull** state from server: watermarks, new panel answers, unanswered TG queries;
2. **discover** videos/streams after watermark; fetch metadata, subs, comments, chat replays;
3. **transcribe** missing/bad audio locally (whisper.cpp);
4. **extract** (Claude, Batch API where possible): topics/facts/stances per video; QA pairs from comments & chat; profile updates; style refresh;
5. **merge**: match against existing topics (create vs. merge decision — LLM with alias/title candidates), detect contradictions → questions; regenerate `paragraph_ru`/`story_md`/opinion for touched topics; regenerate aliases;
6. **push** deltas to server; server rebuilds tantivy; watermarks advance **only after successful push**.

Every stage idempotent and resumable; raw fetched artifacts cached on the Mac so re-runs don't re-download.

## 12. Security

- Caddy: TLS (auto-ACME) for `aancha.serezhkin.com`.
- `/` (SPA+API): basic auth (server-side, not proxy). `/mcp`: bearer token. `/api/import`: separate builder bearer token. TG: outbound long polling only — no inbound webhook surface.
- Secrets via env / `.env` outside the image. Tokens long-random, rotatable from CLI.

## 13. Deployment & ops

- `docker-compose.yml`: `app` + `caddy`; volumes: `data/` (SQLite, WAL mode), `index/` (tantivy), `caddy/`.
- Backups: nightly SQLite `.backup` snapshot, keep N; tantivy is derivable — not backed up. Offsite copy: open question Q7.
- Logs: `tracing` JSON to stdout → `docker logs`.

## 14. Cost & load estimates (sanity, verify in Phase 0)

- Transcription: local ⇒ $0, MacBook-hours (est. ~0.1–0.3× realtime with whisper.cpp large-v3-turbo on Apple Silicon).
- Extraction (one-time backfill): rough order $100–300 with Sonnet via Batch API for ~1000 h of transcripts; incremental cycles: single dollars.
- Production: $0 LLM by design. VPS load trivial (thousands of topics, BM25).

## 15. Risks & mitigations

| Risk | Mitigation |
|---|---|
| yt-dlp blocked / throttled | pacing, caching, resume; harvest-bundle fallback via Ancha |
| RU auto-subs quality | Whisper re-transcription path; per-fact confidence |
| Chat replay unavailable for some streams | degrade gracefully; harvest bundle |
| FTS recall ceiling (no embeddings) | aggressive alias generation; `tg_queries` miss-log feeds alias fixes; phase-2 local embeddings option |
| Topic over-merging / fragmentation | merge requires LLM confirmation + panel visibility; contradicts-edges instead of destructive merges when unsure |
| Basic auth brute force | strong creds, rate limit, fail2ban-style lockout in app |
| TG abuse / cost | per-user rate limits (cost is ~0 anyway — no LLM) |

## 16. Phases

- **P0 — Research**: verify current reality: Data API caption/comment endpoints & quotas, yt-dlp & chat scraper state, tantivy RU stemming, rmcp maturity, teloxide state, whisper.cpp turbo on M-series; count channel inventory (videos/streams/hours). Output → MEMO.md, SPEC corrections.
- **P1 — Skeleton**: repo layout, `aancha serve` (axum, auth, SQLite migrations, health), Docker+Caddy deploy to droplet.
- **P2 — Fetchers**: discovery, subs, comments, chat, audio; local cache; import API + push.
- **P3 — Transcription**: whisper.cpp integration, sub-quality sampling.
- **P4 — Extraction & KB compile**: Claude passes, merge logic, aliases, questions queue, index build.
- **P5 — TG bot.**
- **P6 — Admin panel.**
- **P7 — MCP.**
- **P8 — Cycle hardening**: end-to-end enrichment, backfill completion, backups.

(P5–P7 reorderable; bot first since it's "the main part".)

## 17. Open questions

1. **Backfill scope** — everything since channel start? (Inventory count comes from P0.)
2. **Bot in DMs too**, or group-only?
3. **Reply link cap** — ≤3 + 1 "rethought" OK?
4. **Panel auth users** — two separate basic-auth users (her / us) rather than one shared? I propose two.
5. **Anthropic API key & budget** for build passes — whose key, monthly ceiling?
6. **TG profiles (phase 2)** — building them needs the bot to read all group messages (privacy mode off) or a history export. Defer decision, confirm deferral.
7. **Backups offsite** — DO Spaces, or local-only snapshots for now?
8. **Bot username** — is `@aanchabot` registered? (BotFather, your side.)
9. **DNS** — `aancha.serezhkin.com` → droplet A-record, Caddy takes it from there. Confirm you control DNS.

## 18. Decision log

- 2026-07-19 — Stack: Rust single binary (axum, teloxide, rusqlite, tantivy, rmcp), SQLite canonical on VPS, no-build Preact+htm SPA, Docker+Caddy on DO. — *(V+C)*
- 2026-07-19 — **No LLM in production**; all intelligence precomputed. — *(V)*
- 2026-07-19 — Transcription local on MacBook; audio never uploaded. — *(V)*
- 2026-07-19 — No owner OAuth; harvest-bundle scripts run by Ancha as fallback channel. — *(V)*
- 2026-07-19 — Questions round-trip via admin panel tab, not MD files. — *(V, supersedes first idea)*
- 2026-07-19 — Chat replay in v1 scope (she answers chat actively). — *(V)*
- 2026-07-19 — KB stores original language; user-facing output Russian. — *(V)*
- 2026-07-19 — Server-canonical data; builder pushes deltas over authenticated API; index rebuilt server-side. — *(C, veto-checkable)*
- 2026-07-19 — Alias-based recall instead of embeddings (C1); local-embedding fallback noted for phase 2. — *(C, veto-checkable)*
