# cyberaancha

Distills the public knowledge of **Prof. Ancha Baranova**'s YouTube channel — videos, live streams, stream chats, comments — into a curated, cross-referenced knowledge base with her distilled per-topic opinion, and serves it back as:

- a **web admin panel** where the professor browses, edits, and answers open questions,
- an **MCP endpoint** (`article://` resources + search tools) for research from Claude,
- a **Telegram bot** (post-MVP) answering community questions with YouTube links + timestamps.

## Shape

One deliberately *dumb-but-strict* hub and smart edges:

```
collector (browser JS,      ──►  cyberaancha-server (Rust @ small VPS)   ◄── preparer (Claude on a Mac:
page-context fetch,              SQLite + tantivy + task queue           whisper.cpp, extraction,
session-side harvesting)         REST + MCP + SPA + backups             integration via prompts/)
```

- **No LLM in production** — the server only does BM25 fulltext (Russian stemming + build-time aliases) and templates. All intelligence is precomputed.
- **The server never talks to YouTube** — harvesting runs in a logged-in browser page context (innertube, pure `fetch`, CSP-proof), audio transcription runs locally via whisper.cpp.
- Every fact carries **provenance and authority** (professor's explicit answer > her comment reply > her spoken words > inferred); contradictions become questions the professor answers in the panel.
- The base is scientific reference material: the system **quotes and attributes, never synthesizes medical advice** — enforced structurally by the no-LLM runtime.

## Repo map

| File | What |
|---|---|
| [SPEC.md](SPEC.md) | Requirements, architecture, decisions log — the ground truth |
| [PROG.md](PROG.md) | Programming rules, stack, layout |
| [TODO.md](TODO.md) | Phase plan and progress |
| [MEMO.md](MEMO.md) | Dev memory, newest first |
| `research/` | Verified findings (YouTube CSP/innertube, inventories, benchmarks) |

Status: **P1** — server skeleton up; harvesting, KB compilation, panel, and MCP land phase by phase (see TODO.md).
