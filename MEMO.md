# MEMO.md — dev memory

Newest first. One entry per finished task.

## 2026-07-19 — captions move to the Mac (yt-dlp); collector parallelism

- **Root cause of "captions: none":** YouTube walls browser caption fetching behind poToken (2024+). Verified in-browser: `/youtubei/v1/player` returns UNPLAYABLE + no tracks even with the session; the watch-page `ytInitialPlayerResponse` HAS `["ru(auto)"]` but its timedtext baseUrl returns HTTP 200 **empty** without poToken; `get_transcript` 400s. **yt-dlp gets the auto-captions cleanly** (handles poToken/signatures).
- **Architecture change:** captions leave the browser. Collector now does discover/meta/comments/chat only. discover schedules a **`transcribe`** task (Mac). `scripts/transcribe_pending.sh` = yt-dlp auto-subs (json3, ru→en) first → parse to schema (source=asr); Whisper only when no subs (source=whisper). integrate readiness now requires `transcribe_state='done'`. Harvest wave completes on collector work (maybe_finish_wave counts only collector types); transcribe/integrate run after. `harvest_captions` removed from collector_types (schema/apply left dead-but-harmless).
- **Collector parallelism** (V asked): main loop is now a pool of N=4 concurrent workers (AANCHA_CFG.concurrency, cap 8); each still paces its own requests so net rate ≈ N/pace.
- Collector/panel now brand-aware (console/box/button use AANCHA_CFG.brand). 12 tests updated for the new flow, all green.
- Live-verified against @vanyserezhkin: harvest works (1316 discovered, meta+comments+chat OK); yt-dlp pulled 5464 real RU caption segments for a sample video.

## 2026-07-19 — finished all-but-TG; whisper ready; auth lockout

- **Whisper**: installed whisper-cpp (brew, whisper-cli) + model ~/models/ggml-large-v3-turbo-q5_0.bin (547M, q5_0 quant of turbo). Verified whisper-cli `-oj` JSON = `transcription[].offsets.{from,to}` (ms) + `.text`, which the transcribe script's jq maps to the schema `{t_ms, d_ms, text}` — confirmed valid. Transcribe pipeline (yt-dlp→ffmpeg 16k mono→whisper→submit) ready for real videos.
- **Auth lockout** (src/http/auth_mw.rs): per-IP (nginx X-Real-IP / XFF fallback), 8 failures/60s → 5-min cooldown, guards BOTH basic and bearer auth; cache hits + successes bypass/clear. Keyed on IP so an attacker can't lock out the legit user. `Fails` must be `pub` (exposed via `pub type Lockout`).
- **Decisions**: skipped MCP resources (article://) — the 5 tools already cover the research surface, resources would duplicate get_article; skipped global governor rate-limit — low value for a single-admin tool. Both noted, not blocking.
- Everything except Telegram (P8) is complete. Next: manual testing on @vanyserezhkin (harvest in Vany's browser → transcribe script → integrate Claude session → verify in panel/MCP).

## 2026-07-19 — MVP live end-to-end on youtube.serezhkin.com (HTTPS)

- Full stack verified over HTTPS through nginx: panel /admin (200), /api/test-query hits via alias, and **MCP full handshake** (initialize→tools/list→tools/call). All 5 MCP tools work from a remote client.
- **rmcp gotcha that bit prod:** StreamableHttpService has DNS-rebinding protection — `allowed_hosts` defaults to loopback only, so every nginx-proxied request 403'd with "Host header is not allowed". Fix: `StreamableHttpServerConfig::default().with_allowed_hosts([...])` seeded with the public host derived from `public_url` (`mcp::host_of`). Both config structs (ServerInfo, StreamableHttpServerConfig) are #[non_exhaustive] → builder methods, not struct literals. nginx passes MCP SSE fine (no proxy_buffering change needed).
- Deployed build = MVP complete. Bootstrap creds (admin pw, mcp/collector/preparer tokens) shown in dev session → **Vany must rotate before real use**.

## 2026-07-19 — P5 panel + P6 MCP: MVP line reached

- **P5 panel** (`web/admin.html`, built by a subagent per `web/SPEC.md`): single self-contained vanilla-JS file, XSS-safe via a text-node DOM builder. Tabs: Browse (search + article detail + owner edit/delete), Questions (answer), Test (verbatim bot answer), Sources (video inventory + status chips), System (admin-only: clocks/watermarks/queue, harvest+process buttons, backups, collector launcher, MCP slot). Subagent caught a real bug: `get_article` returned only `citations`, so owner edits would wipe facts/links → **fixed**: ArticleView now returns full stances/facts/links, panel re-sends them verbatim (lossless).
- **P6 MCP** (`src/mcp.rs`): rmcp 2.2 streamable HTTP at `/mcp`, bearer(mcp)-gated. Tools: search_articles, get_article, list_questions, answer_question, kb_stats — return JSON strings. **Verified live**: initialize → tools/list (all 5) → tools/call search_articles finds article via alias, kb_stats returns counts. rmcp notes: `ServerInfo` is #[non_exhaustive] (build from default + set fields); `tool_router` field triggers a false dead-code warning (macro reads it — suppressed); server_info name set explicitly ("aancha"). schemars 1 added for tool param schemas.
- Deferred: MCP resources (article://) + search_transcripts (no transcript index yet); rate-limit/governor (250ms auth brake stands).

## 2026-07-19 — P4 built: KB + tantivy + answer engine + preparer pipeline

- `src/kb` (articles/aliases/stances/facts/links/questions, upsert + tx variant + reads), `src/index` (tantivy 0.26, RU Snowball, delete-all+refill rebuild atomic at commit, boosts), `src/answer` (query→search→templated RU reply, ≤5 links newest-first, disclaimer, honest miss + `queries` log), `src/queue/prep` (serialized integrate with full video bundle, transcribe worker). Migrations 004 (KB) + 005 (integrated/transcribe_state flags).
- **integrate collapses SPEC extract+integrate into one Claude pass** keyed by video — simpler for the session model; separate batched extract can slot in later. needs_transcription verdict spawns a transcribe task (self-healing captions).
- Endpoints: /api/test-query, /api/articles (search/get/put owner-edit), /api/questions (list/answer), /api/process/enqueue, /api/prep/* (claim/result/search), /api/transcribe/* — preparer bearer.
- **Real findings (research/):** (1) tantivy 0.26 needs `TopDocs::with_limit(n).order_by_score()`; (2) Snowball RU does NOT unify all cases (геморрой→геморр vs геморроя→геморро) — aliases MUST carry inflected forms, key instruction for the integrate prompt; (3) BM25 IDF collapses on tiny corpora → absolute score floor isn't robust, set permissive 0.1, tune from miss-log. 12 tests green, 0 warnings.
- Whisper NOT run yet (transcribe script + whisper-cpp install pending). Prompts/PREP playbook pending.

## 2026-07-19 — TLS live on youtube.serezhkin.com

- certbot issued the cert (per-host, HTTP-01) after the DNS negative-cache (900s SOA min) expired. Root cause of the earlier certbot failure was purely that stale NXDOMAIN, not config. HTTPS chain verified: /healthz 200, /admin 401→200. nginx vhost symlinked to ~vany/aancha/nginx-aancha.conf (deploy never clobbers it). Auto-renew armed (certbot.timer). `/` was a bare 404 → added redirect to /admin.

## 2026-07-19 — P2 built and deployed: queue + collector + panel

- Queue engine (`src/queue`): idempotent wave enqueue (7-day windows, back/forward direction), lease-based claim (30 min, 5 attempts), submit validated against `schemas/*.json` (compiled once via OnceLock) and applied in one transaction, watermarks (wm_oldest/wm_newest) advance when the wave drains. Migration 002: videos, tasks (UNIQUE type+subject), raw_docs (zstd), transcripts (zstd).
- Collector (`collector/collector.js`): pure-fetch page-context. **Verified live in Chrome against @vanyserezhkin** — YouTube moved channel tabs to `lockupViewModel` (videoRenderer gone); rewrote parsing (details in research/youtube-structure-2026-07.md). player endpoint + publishDate confirmed. Bookmarklet (fetch source → Trusted-Types policy → eval) and console snippet, both built in /admin from a pasted collector token.
- Gotcha that shaped testing: recent Chrome gates HTTPS-page→127.0.0.1 behind Local Network Access permission → **can't drive browser→localhost POST in dev**; irrelevant in prod (both public HTTPS). Server round-trip proven by unit tests + curl; panel rendered + screenshotted.
- Deployed to n1 (4f92493). Bootstrap creds set (admin pw + collector token) — **rotate before public exposure**. jsonschema pulled default-features=false to stay lean.
- **First real harvest blocked on public HTTPS**: needs Vany's DNS A-record + `sudo` nginx/certbot (deploy/README.md). Until then /admin reachable only via `ssh -L 8087:127.0.0.1:8087 n1`.

## 2026-07-19 — P1 built and deployed (TLS pending Vany)

- Server core: db (single-conn mutex, `call`/`with`, user_version migrations), auth (argon2 0.5, blake3 tokens `aancha-<purpose>-<hex>`, rotation invalidates), basic-auth middleware (username = role; 10-min verify cache; 250 ms brake), backup (VACUUM INTO → tar.gz, prune keep-N, daily tokio loop, restore with listen-guard + pre-restore copy), /api/state + /api/backups.
- Deployed to n1 via zigbuild → 4.6 MB static musl → scratch image: **640 KiB RSS** idle. Gotchas hit: rusqlite 0.40 needs rustc ≥1.95 (`cfg_select` in libsqlite3-sys) → toolchain updated 1.94.1→1.97.1; argon2 0.5 default features lack OsRng → salt via `rand::random` + `SaltString::encode_b64`.
- Compose: host networking (app's 127.0.0.1 bind is the boundary), uid 1000, mem 256 MB. Blocked on Vany: DNS A-record, then sudo nginx+certbot lines, then credentials — all in deploy/README.md.
- Repo: github.com/Vany/cyberaancha (private), origin set.

## 2026-07-19 — P0 research done

- Full findings + sources: `research/p0-findings.md`. Raw channel listings: `research/inv_*.txt`.
- Headlines: collector = **pure-fetch in page context** (YouTube CSP has no connect-src; Trusted Types enforced but irrelevant to fetch); innertube endpoints mapped (get_transcript / captionTracks / next / get_live_chat_replay / browse); **read live `ytcfg`, never hardcode client version** (rolls weekly).
- Inventories: test channel @vanyserezhkin 1341 items / 1321 h — *bigger than prod*; @AnchaBaranovaProf 1271 items / 1294 h. Captions-first strategy is load-bearing; Whisper fills gaps only.
- Stack confirmed: tantivy 0.26.1 (RU stemmer ✓), rmcp 2.2.0 (official, streamable HTTP ✓), whisper.cpp large-v3-turbo q5_0 + Metal (RU ≈ v3 quality; RU fine-tunes exist as fallback).
- Hardware: build/whisper Mac = M4 Max, 48 GB, 16 cores → turbo ≈ 15–20× realtime. n1 server = 1 vCPU / 457 MB / 1.7 GB free disk (see SPEC C7).
- Installed today on Mac: yt-dlp (brew). Deferred installs: whisper-cpp + model (P3), zig + cargo-zigbuild (P1).

## 2026-07-19 — Project started

- SPEC.md v0.1 → v0.2: hub-and-edges (dumb-strict server on n1, browser collector, Claude-as-preparer, no second binary). Git initialized, gitmode=history (commit to main).
