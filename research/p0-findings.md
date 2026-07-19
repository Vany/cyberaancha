# P0 research findings — 2026-07-19

Everything below verified today (curl, yt-dlp, docs, web). Corrections applied to SPEC.md.

## 1. YouTube CSP vs collector (verified via curl, homepage + watch page)

Three CSP policies, identical on both pages:
1. allowlist `script-src` (`unsafe-inline`, `unsafe-eval`, Google domains);
2. `require-trusted-types-for 'script'` — **Trusted Types enforced**: DOM script-injection sinks need TrustedScript[URL]. No `trusted-types` allowlist directive ⇒ we may `trustedTypes.createPolicy()` ourselves if ever needed;
3. strict nonce + `strict-dynamic` `script-src`.

**No `default-src`, no `connect-src`** ⇒ `fetch()` from page context to any destination is CSP-unrestricted. Collector therefore is **pure-fetch, zero DOM script injection**: console snippet guaranteed to work; bookmarklet plausible (Chrome CSP-vs-bookmarklet behavior is quirky — decide in testing, as planned).

## 2. Innertube endpoints (page context, same-origin; no CORS for outsiders — which is exactly why the collector lives in the page)

- **Transcripts**: `POST /youtubei/v1/get_transcript` (params from watch-page HTML `getTranscriptEndpoint`); fallback: `ytInitialPlayerResponse.captions.playerCaptionsTracklistRenderer.captionTracks[]` → timedtext URLs, `fmt=json3`; second fallback: `POST /youtubei/v1/player` with Android client context (better success rate).
- **Comments**: initial batch + continuation token in `ytInitialData`; loop `POST /youtubei/v1/next` with token → batch + next token, until exhausted.
- **Live chat replay**: continuation in `ytInitialData` (`live_chat_replay?continuation=…`); then `POST /youtubei/v1/live_chat/get_live_chat_replay`. Entries in `continuationContents.liveChatContinuation.actions`.
- **Channel listing**: `POST /youtubei/v1/browse` (videos/streams tabs) — same continuation pattern.
- **Gotchas**: client version (e.g. `2.20260416.01.00`) rolls weekly — collector must read live `ytcfg`/`INNERTUBE_CONTEXT` from the page, never hardcode. Logged-in requests want `SAPISIDHASH` Authorization — computable in page JS (SAPISID cookie is not HttpOnly); logged-out works without it for public data.

Sources: [summarize.sh YouTube docs](https://summarize.sh/docs/youtube.html), [ytranscript reverse-engineering writeup](https://nadimtuhin.com/blog/ytranscript-how-it-works), [jdepoix/youtube-transcript-api](https://github.com/jdepoix/youtube-transcript-api), [ScrapeCreators guide](https://scrapecreators.com/blog/youtube-video-transcripts-guide), [SkipTheWatch 2026 guide](https://skipthewatch.com/blog/youtube-transcript-api-guide), [Youtube-dl-chat endpoints.md](https://github.com/mmis1000/Youtube-dl-chat/blob/master/endpoints.md), [NewPipeExtractor #469 chat replay findings](https://github.com/TeamNewPipe/NewPipeExtractor/issues/469), [Kuang: decoding YouTube's API design](https://kuangbyte.medium.com/peeking-behind-the-curtain-decoding-youtubes-api-design-through-network-traffic-e3a68463df05), [Scrapfly: scraping YouTube 2026](https://scrapfly.io/blog/posts/how-to-scrape-youtube-in-2025).

## 3. Channel inventories (yt-dlp flat listing, 2026-07-19; raw lists in research/inv_*.txt)

| Channel | Videos | Streams | Total hours |
|---|---|---|---|
| @vanyserezhkin (test) | 1300 (1166 h) | 41 (155 h) | **1321 h** |
| @AnchaBaranovaProf (prod) | 767 (494 h) | 504 (800 h) | **1294 h** |

Surprises: the test channel is *bigger* than production — chunked testing is mandatory, "process whole test channel" is not a thing. **Captions-first is load-bearing**: Whisper-everything at ~1300 h is not viable; Whisper only fills gaps (if ~15 % need it → ~190 h audio → ~13–25 Mac-hours at turbo speeds). yt-dlp works logged-out from the Mac's residential IP.

## 4. Stack confirmations

- **tantivy 0.26.1** (2026-07-10): stemmer supports Russian ✓ ([docs](https://docs.rs/tantivy/latest/tantivy/tokenizer/enum.Language.html)).
- **rmcp 2.2.0** (2026-07-08): official Rust MCP SDK; stdio + **streamable HTTP** server transport, `#[tool]`/`#[prompt]` macros, optional OAuth ([docs](https://docs.rs/rmcp/latest/rmcp/)).
- **whisper.cpp large-v3-turbo**: 809M params, 1.6 GB model, ~5× faster than large-v3 at ~95 % of its quality; Russian stays close to full v3; `q5_0` quant + Metal = sweet spot; turbo transcribes source language only (no translate — fine for us). If RU quality disappoints: fine-tunes exist ([antony66 large-v3 RU WER 9.84→6.39 %](https://www.aimodels.fyi/models/huggingFace/whisper-large-v3-russian-antony66), [turbo-russian](https://huggingface.co/dvislobokov/whisper-large-v3-turbo-russian)) — need GGML conversion. Benchmarks: [whispernotes](https://whispernotes.app/blog/introducing-whisper-large-v3-turbo), [mac-whisper-speedtest](https://github.com/anvanvan/mac-whisper-speedtest), [voicci](https://www.voicci.com/blog/apple-silicon-whisper-performance.html).
- Local Mac: ffmpeg 8.1.2 ✓, jq 1.8.1 ✓, cargo/rustc 1.94.1 ✓, yt-dlp installed today ✓. Still to install (later phases): whisper-cpp + model (~1.6 GB), zig + cargo-zigbuild.

## 5. Consequences already applied to SPEC

- Collector: pure-fetch design; reads live `ytcfg`; computes SAPISIDHASH when logged in; client-version drift is a maintenance risk (small, versioned collector).
- Whisper: default `large-v3-turbo q5_0` + Metal; RU fine-tune as quality fallback.
- Extraction volume (prod): ~1300 h ≈ 9–13 M words ≈ 15–20 M input tokens across ~1270 extract tasks — chunked over cycles.
