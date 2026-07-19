# web/MEMO.md — panel dev memory (newest first)

## 2026-07-19 — P5 panel built (`web/admin.html`)

Full multi-tab panel replacing the P2 placeholder. One self-contained file:
inline CSS + vanilla JS, no build/CDN/framework. Embedded via `include_str!`,
served at `GET /admin`. XSS-safe by construction: a tiny `h()` DOM builder puts
every server string into a text node — no `innerHTML` ever touches KB content.

Built per tab:
- **Обзор (Browse)** — debounced (250 ms) search → `GET /api/articles?q=`; result
  list (title/slug/score) → article detail at `#browse/<slug>`: status badge,
  paragraph, alias chips, `story_md` rendered as safe block-markdown (headings/
  lists/paragraphs, DOM nodes only), citations as `https://youtu.be/<id>?t=<s>`
  links with authority+date+text. Inline **Edit** (both roles) + **Delete** (confirm).
- **Вопросы (Questions)** — open list (context/question/created_at) + answer
  textarea → `POST …/answer`, removed on success. Empty state: "нет открытых вопросов".
- **Тест (Test)** — query → `POST /api/test-query`, verbatim answer in a monospace
  pre (newlines preserved), hit/miss chip, clickable `related` slugs.
- **Источники (Sources)** — `GET /api/videos` table with meta/captions/comments/
  chat/integrated status chips (have/done/true=green, pending=false=amber, none/na=grey), total count.
- **Система (System, admin only)** — clocks+watermarks, queue table + counts,
  harvest-back/forward + process buttons (JSON shown inline), backups list + make,
  collector launcher (reuses P2 Trusted-Types bookmarklet + console snippet;
  token stays in the page), MCP endpoint placeholder card (token slot for P6).

Behaviors: hash router (`#browse` default), `/api/state` on boot, 10 s System poll
while visible, Enter submits Test/search, buttons disabled in-flight, errors shown
inline (status + body), owner sees no System tab / no admin buttons.

### API mismatches / notes (did NOT change Rust — coded against the documented contract)

1. **Article edit round-trip is lossy for facts/links (structural).** The spec's PUT
   contract says "re-send stances/facts/links unchanged", but `GET /api/articles/{slug}`
   (`kb::ArticleView`) returns **only `citations`** — no `facts`, no `links`, and
   stances only as the citation projection (`video_id, offset_ms, authority,
   occurred_at, text` — `source_kind`/`source_ref` absent). So a panel text-edit
   cannot preserve facts/links (upsert replaces children wholesale → they'd be
   wiped). Mitigation in the panel: on save we **reconstruct stances from
   citations** (`source_kind` inferred: `video` if `video_id` present else `panel`;
   `authority` kept as-is) so the citation timeline survives, and send
   `facts:[]`, `links:[]`. Verified: after an owner PUT the citation (video+offset)
   is preserved. **If facts/links become populated by integrate (P4), owner panel
   edits will drop them.** To fix properly, `get_article` should also return
   `stances`/`facts`/`links` (full round-trip) — a Rust change for Vany to weigh.

2. **`POST /api/test-query` on empty `q` returns HTTP 400**, not `{hit:false,…}`.
   The panel simply doesn't submit an empty query (guarded client-side).

3. `GET /api/backups` is not role-gated server-side (both roles can GET); harmless
   since the System tab is admin-only in the UI. `POST` backups/harvest/process are
   correctly 403 for owner (verified).

4. No "create new article" flow (spec's Browse is search→detail→edit only); slug is
   fixed from the loaded article, so no slug input is needed.

### Verified against a local server (clean DB, seeded `demo` = Мелатонин)
- Search "бессонница" → finds `demo` via alias; detail shows `youtu.be/vid00000001?t=90`.
- Test "бессонница" → `hit:true`, verbatim: `Про «Мелатонин» — https://youtu.be/vid00000001?t=90.\nМнение профессора: …\n— Справочный материал …, не медицинская рекомендация.`
- Owner PUT round-trip → 204, citation preserved. Owner harvest → 403. Admin harvest → `{discover:enqueued}`. Process → `{integrate_enqueued:0,ready_videos:0}`.
- Owner role hides System tab (nav = Обзор/Вопросы/Тест/Источники). Questions empty = "нет открытых вопросов".
- Chrome (chrome-devtools MCP): **no console messages on load**. Note: fetch from a
  credentials-in-URL page is blocked by Chrome — navigate to the credentialed URL
  once to prime the auth cache, then load the plain `http://127.0.0.1:8087/admin`.
  Added `<link rel="icon" href="data:,">` to avoid a `/favicon.ico` 404.

## 2026-07-19 — round-trip issue RESOLVED (main-loop follow-up)

Finding #1 above is fixed: `kb::get_article` / `ArticleView` now returns full
`stances`, `facts`, and `links` (not just the `citations` projection). The panel
save was updated to re-send `art.stances/facts/links` verbatim — owner text-edits
are now lossless (no reconstruction, facts/links preserved). Findings #2 (empty
test-query → 400, guarded) and #3 (backups GET not role-gated, harmless) stand as-is.
