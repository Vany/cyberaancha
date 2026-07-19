# TODO.md — tasks in order

## P0 — research ✅ 2026-07-19
Findings in `research/p0-findings.md`, summary in MEMO.md.

## P1 — server skeleton + first deploy
- [x] cargo scaffold: CLI (serve/backup/restore/set-password/gen-token), config, tracing, /healthz
- [ ] SQLite layer: pool, migrations runner, `auth` + `meta` tables
- [ ] `set-password` / `gen-token` CLI (argon2 / blake3-at-rest)
- [ ] basic-auth middleware with owner/admin roles; bearer middleware for collector/preparer/mcp
- [ ] backup: tarball create + prune + daily internal scheduler; `backup` CLI; `restore --latest --yes`
- [ ] deploy tooling: zig + cargo-zigbuild install, Makefile (build-linux/deploy), scratch Dockerfile, compose
- [ ] n1: nginx vhost + certbot (**needs DNS A-record — Vany**), `~vany/aancha` layout, first deploy
- [ ] smoke: `https://aancha.serezhkin.com/healthz` green

## P2 — queue + collector (first harvest)
- [ ] task engine: claim w/ lease, submit w/ jsonschema validation, fail/retry, wave enqueue (window_days)
- [ ] schemas: discover, harvest_meta, harvest_captions
- [ ] `videos` + raw storage tables (zstd blobs)
- [ ] collector.js: ytcfg reader, discover (browse tabs), captions (get_transcript→captionTracks→player), meta; pacing; token
- [ ] bookmarklet builder + snippet copy in minimal System tab; **test bookmarklet vs snippet on youtube.com**
- [ ] first wave on @vanyserezhkin (one week window) end-to-end

## P3 — harvest completeness
- [ ] schemas + collector: harvest_comments (/next continuations), harvest_chat (get_live_chat_replay)
- [ ] comment/chat storage; professor-authored detection (authorChannelId)

## P4 — preparer loop + KB + index
- [ ] schemas: transcribe, extract, integrate; prompts/extract.md, prompts/integrate.md; PREP.md playbook
- [ ] scripts/transcribe_pending.sh (yt-dlp audio → whisper.cpp turbo q5_0; install whisper-cpp + model)
- [ ] KB tables: articles, facts, stances, aliases, links, qa_pairs, people, questions
- [ ] integrate path: serialized, contradictions → questions, watermark advance
- [ ] tantivy: articles + transcripts indexes, RU stemmer, atomic swap rebuild
- [ ] answer engine + /api/test-query
- [ ] run full loop on the harvested week; iterate prompt quality

## P5 — admin panel
- [ ] SPA shell, auth, tabs: Search/Browse, Article (view/edit), Questions, Test, Sources, System (clocks, queue, collector launcher, MCP info, backups)
- [ ] rate limiting + lockout

## P6 — MCP  ← MVP line
- [ ] rmcp streamable HTTP at /mcp, bearer; tools: search_articles, get_article, search_transcripts, get_video, list_questions, answer_question, next_task, submit_result, kb_stats; resources article:// video:// person://
- [ ] token + URL surfaced in System tab

## P7 — production backfill (Ancha)
- [ ] disk decision on n1 (volume/resize/prune policy)
- [ ] wipe + reconfigure channel → @AnchaBaranovaProf; walk windows backward; questions loop live
- [ ] collector polish for Ancha (Tampermonkey?) if owner-only data needed

## P8 — Telegram bot (post-MVP)
- [ ] teloxide, group mention handling, same answer engine, per-user rate limits

## Later
- People tab; TG-group history ingestion; headless scheduled cycles (`claude -p`); local-embeddings recall fallback; git remote on n1 (**awaiting Vany's yes/no**)
