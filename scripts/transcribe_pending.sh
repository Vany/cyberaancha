#!/usr/bin/env bash
# Unattended Whisper worker (SPEC C1/C3): claim transcribe tasks, fetch audio on
# the Mac, transcribe locally, submit segments. No LLM, no agent attention — a
# batch of 50 videos must run itself. Audio never leaves the Mac and is deleted
# after each job.
#
# Env (see PREP.md):
#   AANCHA_SERVER       e.g. https://youtube.serezhkin.com
#   AANCHA_PREP_TOKEN   from `aancha-server gen-token preparer`
#   WHISPER_BIN         default: whisper-cli (whisper.cpp)
#   WHISPER_MODEL       default: ~/models/ggml-large-v3-turbo-q5_0.bin
#   WHISPER_LANG        default: ru
set -euo pipefail

: "${AANCHA_SERVER:?set AANCHA_SERVER}"
: "${AANCHA_PREP_TOKEN:?set AANCHA_PREP_TOKEN}"
WHISPER_BIN="${WHISPER_BIN:-whisper-cli}"
WHISPER_MODEL="${WHISPER_MODEL:-$HOME/models/ggml-large-v3-turbo-q5_0.bin}"
WHISPER_LANG="${WHISPER_LANG:-ru}"
AUTH="Authorization: Bearer ${AANCHA_PREP_TOKEN}"
MODEL_NAME="$(basename "$WHISPER_MODEL")"

for tool in yt-dlp ffmpeg jq curl "$WHISPER_BIN"; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing tool: $tool" >&2; exit 1; }
done
[ -f "$WHISPER_MODEL" ] || { echo "missing model: $WHISPER_MODEL" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

process_one() {
  local claim id yt_id url wav out_json result_json force_whisper
  claim="$(curl -fsS -H "$AUTH" "$AANCHA_SERVER/api/transcribe/claim")" || return 1
  [ "$(jq -r '.task' <<<"$claim")" = "null" ] && return 2   # nothing to do
  id="$(jq -r '.task.id' <<<"$claim")"
  yt_id="$(jq -r '.task.yt_id' <<<"$claim")"
  force_whisper="$(jq -r '.task.whisper // false' <<<"$claim")"  # set when integrate rejected auto-captions
  url="https://www.youtube.com/watch?v=${yt_id}"
  wav="$work/${yt_id}.wav"
  out_json="$work/${yt_id}"          # whisper appends .json
  result_json="$work/${yt_id}.result.json"
  echo "[$(date -u +%H:%M:%S)] transcribe ${yt_id} (task ${id})"

  # Fail loudly to the server so the task requeues/retries instead of vanishing.
  fail() {
    echo "  FAILED: $1" >&2
    curl -fsS -H "$AUTH" -H 'content-type: application/json' \
      -d "$(jq -nc --arg e "$1" '{error:$e}')" \
      "$AANCHA_SERVER/api/transcribe/${id}/fail" >/dev/null || true
    return 1
  }

  # --- Captions-first: yt-dlp auto-captions (fast, free, accurate). YouTube walls
  # browser caption fetching behind poToken, but yt-dlp handles it. Whisper only
  # when there are no captions, OR when integrate judged the captions not good
  # enough (force_whisper) — in which case skip subs and transcribe the audio. ---
  local sub=""
  if [ "$force_whisper" != "true" ]; then
    yt-dlp -q --no-warnings --skip-download --write-auto-subs --write-subs \
      --sub-langs "${WHISPER_LANG},en" --sub-format json3 \
      -o "$work/${yt_id}.%(ext)s" "$url" 2>"$work/sub.err" || true
    sub="$(ls "$work/${yt_id}."*".json3" 2>/dev/null | sort | head -1)"
  else
    echo "  captions rejected upstream → forcing whisper"
  fi
  if [ -n "$sub" ] && [ -s "$sub" ]; then
    local sublang; sublang="$(basename "$sub" | sed -E "s/^${yt_id}\.([^.]+)\.json3$/\1/")"
    jq -c --arg yt "$yt_id" --arg lang "$sublang" \
      '{ yt_id:$yt, lang:$lang, source:"asr",
         segments: [ .events[]? | select(.segs)
           | { t_ms: .tStartMs, d_ms: .dDurationMs,
               text: ([.segs[].utf8] | join("") | gsub("^\\s+|\\s+$";"")) }
           | select(.text != "") ] }' \
      "$sub" >"$result_json" || { fail "parse subs json3"; return 0; }
    if [ "$(jq '.segments | length' "$result_json")" -gt 0 ]; then
      submit_result "$result_json" "captions (${sublang})"
      return $?
    fi
  fi

  # No captions → download audio and transcribe locally with Whisper.
  echo "  no captions; transcribing audio with whisper…"
  if ! yt-dlp -q --no-warnings -f bestaudio -o - "$url" 2>"$work/dl.err" \
       | ffmpeg -hide_banner -loglevel error -i pipe:0 -ar 16000 -ac 1 -c:a pcm_s16le "$wav" 2>"$work/ff.err"; then
    fail "audio fetch/convert: $(tail -c 300 "$work/dl.err" "$work/ff.err" 2>/dev/null | tr '\n' ' ')"
    return 0
  fi
  if ! "$WHISPER_BIN" -m "$WHISPER_MODEL" -f "$wav" -l "$WHISPER_LANG" \
       -oj -of "$out_json" -nt >"$work/w.log" 2>&1; then
    fail "whisper: $(tail -c 300 "$work/w.log" | tr '\n' ' ')"
    return 0
  fi
  jq -c --arg yt "$yt_id" --arg lang "$WHISPER_LANG" --arg model "$MODEL_NAME" \
    '{ yt_id:$yt, lang:$lang, source:"whisper", model:$model,
       segments: [ .transcription[]
         | { t_ms: .offsets.from, d_ms: (.offsets.to - .offsets.from),
             text: (.text | gsub("^\\s+|\\s+$";"")) }
         | select(.text != "") ] }' \
    "${out_json}.json" >"$result_json" || { fail "parse whisper json"; return 0; }
  submit_result "$result_json" "whisper"
}

# Submit a result JSON to the server; log with a source label. Returns 0.
submit_result() {
  local file="$1" label="$2" n
  n="$(jq '.segments | length' "$file")"
  if ! curl -fsS -H "$AUTH" -H 'content-type: application/json' \
       --data-binary @"$file" \
       "$AANCHA_SERVER/api/transcribe/${id}/result" >/dev/null; then
    fail "submit result"; return 0
  fi
  echo "  done via ${label}: ${n} segments"
  rm -f "$work/${yt_id}."* 2>/dev/null
  return 0
}

count=0
while true; do
  set +e
  process_one
  rc=$?
  set -e
  case "$rc" in
    0) count=$((count + 1)) ;;         # processed (ok or task-level fail reported)
    2) echo "no more transcribe tasks; processed ${count}."; break ;;
    *) echo "transient error (rc=$rc); retrying in 10s"; sleep 10 ;;
  esac
done
