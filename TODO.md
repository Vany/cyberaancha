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

## P3 — harvest completeness ✅
- [x] schemas + collector: harvest_comments (/next continuations), harvest_chat (get_live_chat_replay)
- [x] comment/chat storage (migration 003); professor-authored detection (authorChannelId == channel_id)

## P4 — preparer loop + KB + index
- [x] schemas: integrate (envelope: articles+questions), transcribe
- [x] KB tables (migration 004): articles, aliases, stances, facts, links, questions, queries
- [x] integrate: serialized claim, bundle (transcript+comments+chat), upsert articles, questions, needs_transcription→transcribe spawn, mark integrated (migration 005)
- [x] tantivy: articles index, RU Snowball stemmer, delete-all+refill atomic rebuild, boosts (title×3, aliases×2.5, story×0.7)
- [x] answer engine + /api/test-query (≤5 links, newest-first, disclaimer, honest miss + query log)
- [x] preparer + panel endpoints: prep claim/result/search, transcribe claim/result, articles search/get/put, questions list/answer, process/enqueue
- [x] 12 tests incl. full harvest→integrate→reindex→answer + needs_transcription→transcribe→reintegrate
- [x] prompts/integrate.md (v1) + PREP.md playbook (the Claude-session instructions)
- [x] scripts/transcribe_pending.sh (yt-dlp audio → whisper.cpp; curl+jq, unattended, fail-loud)
- [ ] install whisper-cpp + model on Mac; run full loop on a real harvested week; iterate prompt quality
- Note: extract+integrate collapsed into one `integrate` pass (MEMO); RU stemming imperfect → aliases carry inflections (research/)

## P5 — admin panel ✅
- [x] SPA (web/admin.html, single file, vanilla, XSS-safe): Browse (search+detail+owner edit/delete), Questions, Test, Sources, System (clocks/queue, harvest/process, backups, collector launcher, MCP slot). Built by subagent per web/SPEC.md.
- [x] lossless owner edit: get_article now returns full stances/facts/links (fixed subagent-flagged data-loss)
- [ ] rate limiting + lockout — deferred (250ms auth brake exists; governor not yet added)

## P6 — MCP  ← MVP line ✅
- [x] rmcp 2.2 streamable HTTP at /mcp, bearer(mcp); tools: search_articles, get_article, list_questions, answer_question, kb_stats. Verified live: initialize/tools-list/tools-call all work.
- [x] MCP URL in System tab (token via `gen-token mcp`, shown once by CLI — server stores only the hash)
- Note: resources (article://) + search_transcripts deferred (no transcript index yet); tools cover the MVP research surface.

## P7 — production backfill (Ancha)
- [ ] disk decision on n1 (volume/resize/prune policy)
- [ ] wipe + reconfigure channel → @AnchaBaranovaProf; walk windows backward; questions loop live
- [ ] collector polish for Ancha (Tampermonkey?) if owner-only data needed

## P8 — Telegram bot (post-MVP)
- [ ] teloxide, group mention handling, same answer engine, per-user rate limits

## Later
- People tab; TG-group history ingestion; headless scheduled cycles (`claude -p`); local-embeddings recall fallback
- ~~git remote on n1~~ → resolved: github.com/Vany/cyberaancha (private) is origin
