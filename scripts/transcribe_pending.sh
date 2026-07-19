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
  local claim id yt_id url wav out_json result_json
  claim="$(curl -fsS -H "$AUTH" "$AANCHA_SERVER/api/transcribe/claim")" || return 1
  [ "$(jq -r '.task' <<<"$claim")" = "null" ] && return 2   # nothing to do
  id="$(jq -r '.task.id' <<<"$claim")"
  yt_id="$(jq -r '.task.yt_id' <<<"$claim")"
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

  # yt-dlp → 16 kHz mono WAV (what whisper.cpp wants), piped, no intermediate file.
  if ! yt-dlp -q --no-warnings -f bestaudio -o - "$url" 2>"$work/dl.err" \
       | ffmpeg -hide_banner -loglevel error -i pipe:0 -ar 16000 -ac 1 -c:a pcm_s16le "$wav" 2>"$work/ff.err"; then
    fail "audio fetch/convert: $(tail -c 300 "$work/dl.err" "$work/ff.err" 2>/dev/null | tr '\n' ' ')"
    return 0
  fi

  # whisper.cpp → JSON with millisecond offsets.
  if ! "$WHISPER_BIN" -m "$WHISPER_MODEL" -f "$wav" -l "$WHISPER_LANG" \
       -oj -of "$out_json" -nt >"$work/w.log" 2>&1; then
    fail "whisper: $(tail -c 300 "$work/w.log" | tr '\n' ' ')"
    return 0
  fi

  # Shape into the transcribe schema; drop empty segments.
  jq -c \
    --arg yt "$yt_id" --arg lang "$WHISPER_LANG" --arg model "$MODEL_NAME" \
    '{ yt_id:$yt, lang:$lang, model:$model,
       segments: [ .transcription[]
         | { t_ms: .offsets.from,
             d_ms: (.offsets.to - .offsets.from),
             text: (.text | gsub("^\\s+|\\s+$";"")) }
         | select(.text != "") ] }' \
    "${out_json}.json" >"$result_json" || { fail "parse whisper json"; return 0; }

  local n; n="$(jq '.segments | length' "$result_json")"
  if ! curl -fsS -H "$AUTH" -H 'content-type: application/json' \
       --data-binary @"$result_json" \
       "$AANCHA_SERVER/api/transcribe/${id}/result" >/dev/null; then
    fail "submit result"
    return 0
  fi
  echo "  done: ${n} segments"
  rm -f "$wav" "${out_json}.json" "$result_json"
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
