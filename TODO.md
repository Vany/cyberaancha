# TODO.md — tasks in order

## P0 — research ✅ 2026-07-19
Findings in `research/p0-findings.md`, summary in MEMO.md.

## P1 — server skeleton + first deploy
- [x] cargo scaffold: CLI (serve/backup/restore/set-password/gen-token), config, tracing, /healthz
- [x] SQLite layer: single-conn mutex + spawn_blocking, migrations via user_version, `auth` + `meta`
- [x] `set-password` / `gen-token` CLI (argon2 / blake3-at-rest; stdin-pipe provisioning)
- [x] basic-auth middleware, owner/admin roles, verify-cache, 250 ms failed-attempt brake (bearer mw → P2/P6 with consumers)
- [x] backup: create/prune/daily scheduler, `backup` CLI + POST /api/backups (admin-only), `restore --latest --yes` with listen-guard + pre-restore safety copy
- [x] deploy tooling: zig + cargo-zigbuild, Makefile (build-linux/deploy/logs), scratch Dockerfile, compose (host net, 256 MB cap, uid 1000)
- [x] first deploy to n1: container up, 640 KiB RSS, healthz ok, migrations ran
- [ ] n1: nginx vhost + certbot — **blocked on Vany**: DNS A-record + sudo commands in deploy/README.md
- [ ] credentials on n1 (deploy/README.md) — after TLS, Vany picks passwords
- [ ] smoke: `https://aancha.serezhkin.com/healthz` green

## P2 — queue + collector (first harvest)
- [x] task engine: claim w/ lease, submit w/ jsonschema validation, fail/retry, wave enqueue (window_days, back/forward)
- [x] schemas: discover, harvest_meta, harvest_captions (single source of truth, compiled once)
- [x] `videos` + `tasks` + `raw_docs` (zstd) + `transcripts` tables (migration 002)
- [x] collector.js: ytcfg reader, discover (lockupViewModel + browse continuations), captions (player→captionTracks→json3), meta, SAPISIDHASH, pacing; token from panel
- [x] bookmarklet builder + snippet copy in minimal /admin; CORS+PNA for youtube.com
- [x] deployed to n1; panel + endpoints live; admin/collector creds provisioned
- [ ] **first real wave on @vanyserezhkin — blocked on public HTTPS** (DNS + nginx/certbot); browser→localhost is gated by Chrome LNA, so needs the real endpoint. Harvest mechanics verified in-browser (research/youtube-structure-2026-07.md)

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
- People tab; TG-group history ingestion; headless scheduled cycles (`claude -p`); local-embeddings recall fallback
- ~~git remote on n1~~ → resolved: github.com/Vany/cyberaancha (private) is origin
