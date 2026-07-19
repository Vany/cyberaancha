# PROG.md — programming rules

Read SPEC.md first; this file is *how* we build what SPEC describes.

## Stack (versions verified on crates.io 2026-07-19)

| Crate | Ver | Why | When |
|---|---|---|---|
| tokio | 1.53 | async runtime | now |
| axum | 0.8 | HTTP: routing, extractors; rmcp nests as a tower service | now |
| clap | 4.6 | CLI subcommands (`serve`, `backup`, `restore`, `set-password`, `gen-token`) | now |
| serde / toml | 1 / 1.1 | config + JSON everywhere | now |
| anyhow | 1 | error context in binary code | now |
| tracing / tracing-subscriber | 0.1 / 0.3 | structured logs → stdout (docker logs) | now |
| rusqlite (bundled) | 0.40 | sync SQLite, WAL; own migrations (see below) | P1 |
| argon2 | **0.5 stable** (0.6 is RC — not for prod auth) | password hashes | P1 |
| blake3, rand | 1.8 / 0.10 | token generation + hash-at-rest | P1 |
| tar + flate2 | 0.4 / 1.1 | backup tarballs | P1 |
| jiff | 0.2 | dates/times; store UTC ISO-8601 strings in SQLite | P1 |
| jsonschema | 0.48 | validate task submissions against `schemas/*.json` | P2 |
| zstd | 0.13 | compress raw harvest blobs at rest | P2 |
| tantivy | 0.26 | embedded FTS, RU Snowball stemmer | P4 |
| rmcp | 2.2 | official MCP SDK, streamable HTTP server | P6 |
| tower-http | 0.7 | CORS (collector endpoints), static fallback | P2 |
| governor | 0.10 | rate limiting (auth failures, queries) | P5 |
| rust-embed | 8.12 | SPA + collector embedded in the binary | P2 |

Rule: **a dep enters Cargo.toml in the phase that uses it**, not before. Pin by minor (`"0.26"`), upgrade deliberately.

## Layout

```
src/main.rs          CLI dispatch only
src/config.rs        TOML config, serde defaults, no secrets
src/db/              connection pool (spawn_blocking), migrations, repositories
src/db/migrations/   NNN_name.sql, applied via PRAGMA user_version
src/http/            router assembly, auth middleware, api/* handlers (thin)
src/queue/           task engine: claim/lease/submit/validate
src/kb/              articles, facts, stances, questions, people — domain logic
src/index/           tantivy: schema, build, search, atomic swap
src/answer.rs        the answer engine (test tab now, TG later)
src/backup.rs        tarball create/prune/restore + internal scheduler
src/mcp.rs           rmcp service nested at /mcp (thin over kb/queue)
web/                 SPA: index.html, app.js, tabs/*.js, vendor/ (preact+htm pinned), style.css
collector/           collector.js — pure-fetch, reads live ytcfg; bookmarklet builder
schemas/             JSON Schema per task type — single source of truth (server validates, prompts reference)
prompts/             extract.md, integrate.md, … versioned; PREP.md playbook at repo root
scripts/             transcribe_pending.sh etc. (curl+jq, unattended)
deploy/              Dockerfile (scratch), docker-compose.yml, nginx vhost sample, Makefile targets
```

## Rules

- **Fail loudly.** Unimplemented paths `bail!`; no silent log-and-continue; no `unwrap()`/`expect()` outside tests and startup.
- **Handlers thin.** HTTP layer parses/authorizes/serializes; logic lives in `kb/`, `queue/`, `index/`.
- **DB behind repositories.** rusqlite is sync: call via `spawn_blocking` with a small pool; one writer at a time (SQLite reality); WAL on.
- **Migrations**: numbered SQL files + `PRAGMA user_version`; no framework. Forward-only.
- **Times**: UTC everywhere, ISO-8601 strings in SQLite, jiff types in code.
- **Validation at the boundary**: every task submission → JSON Schema check + referential integrity + size caps. Reject, don't repair. Record prompt version on preparer submissions.
- **Secrets in DB** (`auth` table: argon2 PHC strings, blake3 token hashes), never in config or git. Constant-time compares.
- **Secrets vs public repo** (repo is public): no credential, token, hostname-embedded token, or backup tarball ever enters git — real config (`aancha.toml`), `data/`, `backups/`, `.env*` are gitignored; docs and examples use `CHOSEN_…` placeholders only; edge-side tokens (collector/preparer/mcp) live in gitignored `.env` on the machine that uses them. When in doubt, it doesn't get committed.
- **Comments explain why**; each module small, single-purpose, observable.
- **Tests**: unit-test queue engine, validation, answer engine, slug/alias logic; no mocked-HTTP theater. `cargo test` must stay fast.
- **Frontend**: no build step ever. ES modules, vendored pinned preact+htm, one file per tab, fetch wrapper handling auth/errors. Works from `file://`? No — same-origin only, keep it simple.
- **Collector**: single self-contained JS file; no hardcoded innertube client versions (read `ytcfg` live); paced sleep+jitter; every request resumable at task granularity.
- **Deploy**: `make build-linux` (cargo-zigbuild → x86_64-musl static) → `make deploy` (scp binary + compose to n1, build scratch image there, restart). Box never compiles.
- **Commits**: imperative, scoped prefix (`server:`, `spec:`, `collector:`, `deploy:` …), body only when the why isn't obvious.
