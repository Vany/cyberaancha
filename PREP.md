# PREP.md — the preparer playbook (Mac side)

The preparer is not a binary — it's Claude sessions + shell scripts on Vany's Mac,
driving the server's task queue over REST. The server stores and validates; the Mac
fetches (yt-dlp), transcribes (whisper.cpp), and reasons (Claude). See `../SPEC.md`
§2/§5/§12. Nothing here calls an LLM in production.

## Setup (once)

```sh
export AANCHA_SERVER=https://youtube.serezhkin.com      # test; prod: https://aancha.serezhkin.com
export AANCHA_PREP_TOKEN=<from: aancha-server gen-token preparer>
# tools: yt-dlp (brew), whisper.cpp + a model (see scripts/transcribe_pending.sh), jq, curl
```

Keep the token out of git — a gitignored `.env` on the Mac (`set -a; . ./.env; set +a`).

## The cycle

A harvest wave (browser collector) fills the server with transcripts/comments/chat for a
time window. Then, on the Mac:

1. **Transcribe the gaps** — unattended, no LLM:
   ```sh
   scripts/transcribe_pending.sh          # loops: claim transcribe → yt-dlp audio → whisper → submit
   ```
   Videos whose captions were missing/garbage (integrate returned `needs_transcription`) get a
   real transcript, then reopen for integration.

2. **Integrate** — a Claude session per video, following `prompts/integrate.md`:
   - Interactive during development: run Claude in this repo and tell it
     *"integrate the next video following prompts/integrate.md"*. It claims a task, reads the
     bundle, searches the KB, writes articles, submits.
   - Routine/bulk later: headless `claude -p "process the next N integrate tasks following
     prompts/integrate.md"`. The queue is worker-agnostic — one session or many runs, same result.
   - integrate is **serialized** server-side (one active), so you can't corrupt the KB by running
     two at once; extras just get `{ task: null }`.

3. **Watch progress** — the panel System tab shows the two clocks (last gathered / last processed),
   queue counts, and the questions the professor needs to answer.

## Ordering & idempotency

- Transcribe before integrate when captions are weak, but you don't have to gate it: integrate
  returns `needs_transcription`, which enqueues the transcribe task; the video re-integrates after.
- Everything is idempotent and resumable. A failed task returns to pending (up to 5 attempts) or
  fails loudly. Re-running a cycle re-does only what's unfinished.
- Audio never leaves the Mac; only transcript JSON is submitted. Raw audio is deleted after
  transcription (the script does this).

## Prompt versioning

`prompts/integrate.md` carries a version line. When you change it, bump the version so we can tell
which prompt produced which articles. Keep prompts in git; they are artifacts, not throwaway.

## Endpoints the preparer uses (bearer: preparer)

```
GET  /api/prep/claim              claim the one active integrate task (+ bundle)
GET  /api/prep/search?q=          search existing articles (merge-vs-create)
GET  /api/articles/{slug}         read an article to augment it
POST /api/prep/{id}/result        submit integrate result (schemas/integrate.json)
POST /api/prep/{id}/fail          report failure {error}
GET  /api/transcribe/claim        claim a whisper job {id, yt_id}
POST /api/transcribe/{id}/result  submit {yt_id, lang, model, segments:[{t_ms,d_ms,text}]}
POST /api/transcribe/{id}/fail    report failure {error}
```
