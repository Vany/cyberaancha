# MEMO.md — dev memory

Newest first. One entry per finished task.

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
