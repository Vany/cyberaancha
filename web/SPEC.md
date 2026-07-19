# web/SPEC.md — admin panel

The panel is the MVP surface: the professor (owner) and operator (admin) browse,
search, edit the KB, answer questions, and test the bot. Parent context:
`../SPEC.md`, `../PROG.md`. Iron rule inherited: **no LLM, no external network at
runtime** — the panel talks only to this server's REST API.

## Deliverable & constraints

- **One self-contained file: `web/admin.html`** — inline CSS + vanilla JS, no
  build step, no CDN, no external assets, no framework. It is embedded into the
  Rust binary (`include_str!`) and served at **`GET /admin`**. Do **not** add new
  server routes or split into extra files (that would need Rust route changes).
- Served same-origin behind HTTP Basic auth; the browser attaches credentials to
  every `fetch('/api/…')` automatically — the panel never handles the panel
  password or reads it. (The *collector token* is different — see System tab.)
- UTF-8 throughout; content is Russian. Clean, minimal, legible; system font
  stack; must be readable on a laptop. Light theme is enough.
- Fail visibly: show API errors inline (status + body text), never swallow them.
- Keep it small and observable; comments explain *why*, not *what*.

## Audience & help (C9 — load-bearing)

The **owner is Prof. Baranova, who is not technically educated.** Every operation
she can perform must explain itself, in plain Russian, right where she does it.
This is a first-class requirement, not polish.

- **Reusable help mechanism**: a small `?` info affordance next to non-obvious
  controls that reveals a plain-Russian tooltip on hover AND tap/click (works on
  touch; dismiss on outside tap/Esc). Plus expandable **«Как это работает?»**
  blocks for multi-step operations. Build one tiny helper and reuse it — do not
  hand-place bespoke tooltips.
- **No unexplained jargon** in owner-facing copy: avoid "token / bearer / CSP /
  DevTools / API"; if a technical word is unavoidable, gloss it in lay terms in
  the same breath ("закладка — кнопка в браузере").
- **Every unusual operation gets a numbered walkthrough that ends in the visible
  result to expect** ("…дождитесь зелёной надписи «готово»").
- **The collector is the hardest and most important** to explain. Its launcher
  must include a numbered, non-technical guide: what a закладка is, drag the green
  button to the bookmarks bar once, open youtube.com, click it, watch the little
  window until it says готово. Offer the console-snippet path only as a fallback,
  clearly marked "если закладка не сработала", since it's scarier.
- Where a control is admin-only and genuinely technical (tokens, backups), a brief
  tooltip is enough — admin is Vany. The bar is highest on **owner-visible** surfaces
  (Browse/Test/Questions and, when she harvests, the collector).
- Tone: warm, short, concrete. Assume she's smart but new to the tooling.

Tooltip/help copy is Russian (like all owner-facing UI). Keep it accurate to what
the control actually does — wrong help is worse than none.

## Roles

`GET /api/state` returns `role` = `"owner"` or `"admin"`. Show admin-only controls
(System tab: harvest/process/backup/collector) only when `role === "admin"`.
Owner may still browse/search/edit articles, answer questions, and use Test.
Article edit/delete is allowed for both roles (owner is the professor editing her
own KB). If a write returns 403, surface it.

## API contract (all JSON; base = same origin)

Read (both roles):
- `GET /api/state` → `{ version, role, channel, window_days,
   clocks:{last_gathered_at,last_processed_at,last_backup_at,last_backup_status},
   watermarks:{oldest,newest},
   queue:{ tasks:{ <type>:{ <state>:count } }, videos, transcripts, comments, author_replies } }`
   (any field may be null; timestamps are ISO-8601 strings or null.)
- `GET /api/articles?q=<text>` → `{ results:[ { slug, title, score } ] }` (empty q → empty results)
- `GET /api/articles/{slug}` → `{ slug, title, paragraph_ru, story_md, status,
   aliases:[string], citations:[ { video_id, offset_ms, authority, occurred_at, text } ] }`
   404 if absent. `authority` ∈ panel|comment_author|spoken|inferred.
- `GET /api/videos` → `{ videos:[ { yt_id, kind, title, published, duration_s,
   meta_done:bool, captions, comments, chat, integrated:bool } ] }`
   captions ∈ pending|have|none; comments ∈ pending|have|none; chat ∈ pending|have|none|na.
- `GET /api/questions` → `{ questions:[ { id, article_slug, context, question, status, created_at } ] }` (open only)
- `POST /api/test-query` body `{ q }` → `{ hit:bool, text, slug?, related:[slug] }`
   (`text` is the exact rendered bot answer — show it verbatim, preserve newlines.)

Write (owner or admin unless noted):
- `PUT /api/articles/{slug}` body = full article; path slug must equal body slug → 204.
   Body shape: `{ slug, title, paragraph_ru, story_md, status:"draft"|"published",
   aliases:[string], stances:[ { text, video_id?, offset_ms?, source_kind, source_ref?, authority, occurred_at? } ],
   facts:[…], links:[ { to_slug, kind } ] }`. When editing, send back the fields you loaded
   (preserve stances/facts/links you didn't change — re-send them as-is; upsert replaces children wholesale).
   source_kind ∈ video|comment|chat|panel. For an owner text edit, keep existing stances; new manual
   stances use source_kind "panel", authority "panel".
- `DELETE /api/articles/{slug}` → 204 (404 if absent).
- `POST /api/questions/{id}/answer` body `{ answer }` → 204 (then drop it from the list).

Admin-only (System tab; 403 for owner):
- `POST /api/harvest/enqueue?direction=back|forward` → `{ discover: … }`
- `POST /api/process/enqueue` → `{ integrate_enqueued, ready_videos }`
- `GET /api/backups` → `{ backups:[path] }` ; `POST /api/backups` → `{ created: path }`
- `GET /collector.js` → the collector source text (for the launcher).

## Tabs

Top nav switches tabs client-side (hash router, e.g. `#browse`). Default `#browse`.
Header shows channel + version + role from `/api/state`.

1. **Browse** — search box (debounced) → `GET /api/articles?q=`; result list (title,
   slug, score). Click a result → article detail:
   - title + status badge (draft/published), paragraph_ru, aliases as chips,
     story_md (render as preformatted or minimal markdown — headings/paragraphs/lists ok, but
     never fetch remote anything), and citations as a list of clickable
     `https://youtu.be/<video_id>?t=<offset_ms/1000>` links, each showing authority + occurred_at + text.
   - **Edit** (both roles): inline form for title, status (select), paragraph_ru
     (textarea), story_md (textarea), aliases (comma or newline separated). Save →
     PUT (re-send loaded stances/facts/links unchanged). **Delete** with a confirm.
     After save/delete, refresh the view.

2. **Questions** — list open questions (context + question + created_at). Each has an
   answer textarea + Submit → POST answer → remove from list. Empty state: "нет открытых вопросов".

3. **Test** — a query input mimicking Telegram. Submit → POST /api/test-query → show the
   `text` verbatim (monospace block, preserve newlines), plus a hit/miss indicator and
   `related` slugs (clickable → open that article in Browse). This is how the bot will answer.

4. **Sources** — table of `/api/videos`: title (link to `https://youtu.be/<yt_id>`), kind,
   published date, duration (h:mm), and status chips for meta/captions/comments/chat/integrated
   (color: have/done/true = green, pending = amber, none/na = grey). Read-only. Show total count.

5. **System** (admin only) —
   - Clocks: last gathered / last processed / last backup (+status) / watermarks oldest…newest.
   - Queue: a small table of task type → state → count, plus videos/transcripts/comments/author_replies.
   - Buttons: "Собрать прошлую неделю" (harvest back), "Собрать новое" (harvest forward),
     "Обработать готовое" (process/enqueue). Show the JSON result inline.
   - Backups: list existing, "Сделать бэкап" (POST) button.
   - Collector launcher: a password input for the collector token (stays in the page only,
     never sent anywhere but embedded into the bookmarklet/snippet). On input, build:
     (a) a draggable bookmarklet link, and (b) a copyable console snippet, both = config
     `{server: location.origin, token, pace_ms:1500}` + the text of `/collector.js`.
     Snippet = `window.AANCHA_CFG={…};\n<collector source>`. Bookmarklet = a `javascript:` URL
     that fetches `/collector.js` and evals it under a Trusted-Types policy (see the existing
     minimal admin.html for the exact working pattern — reuse it).
   - MCP: a placeholder card "MCP endpoint" showing `${location.origin}/mcp` and a note that the
     token comes from `gen-token mcp` (the token display wiring lands with P6; leave a labeled slot).

## Behaviors & polish

- Auto-load `/api/state` on start; poll it on the System tab every ~10s while visible.
- Debounce search input ~250ms. Enter submits Test and Browse search.
- Loading and empty states everywhere; disable buttons while their request is in flight.
- Escape all interpolated text (no innerHTML with server strings unless escaped) — treat
  KB content as untrusted for XSS even though it's ours.
- No console errors on load.

## Acceptance (how to self-verify)

Build & run the server locally, seed data, drive the API:
```
cargo run -- serve --config aancha.toml   # (create aancha.toml from the example; data/ is scratch)
# provision creds:
printf 'testpass1' | cargo run -- set-password admin
# seed an article via the API and confirm the panel shows it:
curl -u admin:testpass1 -X PUT localhost:8087/api/articles/demo -H 'content-type: application/json' \
  -d '{"slug":"demo","title":"Мелатонин","status":"published","paragraph_ru":"…","aliases":["бессонница","сон"],
       "stances":[{"text":"…","video_id":"vid00000001","offset_ms":90000,"source_kind":"video","authority":"spoken"}]}'
```
Then load `http://admin:testpass1@127.0.0.1:8087/admin` (note: fetch-from-credentialed-URL is
blocked by Chrome, so for *interactive* checks open normally and enter creds; for scripted checks
use curl against the API). Verify each tab against the contract above; confirm Test on "бессонница"
returns a hit rendering the article with a youtu.be link; confirm no JS console errors.

Update `web/MEMO.md` and check the boxes in `web/TODO.md` when done; report what you built and any
API mismatches you hit (do not change the Rust API — report it and I'll adjust).
