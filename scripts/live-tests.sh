#!/bin/bash
# Live checks against real models and the real desktop. None of these run in
# the normal test suite.
#
#   scripts/live-tests.sh models    llama.cpp, whisper.cpp and Fun-ASR providers
#   scripts/live-tests.sh desktop   microphone capture + injection into TextEdit
#   scripts/live-tests.sh overlay   overlay does not take focus, can be dragged
#   scripts/live-tests.sh e2e       push-to-talk in a window -> switch away -> paste back, in the app
#   scripts/live-tests.sh all
#
# desktop / overlay / e2e need Accessibility (and Microphone) permission for
# the terminal, take over the keyboard and mouse for a few seconds, and close
# TextEdit when they finish. e2e uses the providers configured in the app's
# settings.
#
# Model locations default to ~/.local/share/voicebridge; override with VB_HOME
# or the individual VB_* variables.
set -euo pipefail
cd "$(dirname "$0")/.."

VB_HOME="${VB_HOME:-$HOME/.local/share/voicebridge}"
export VB_LLAMA_SERVER="${VB_LLAMA_SERVER:-$(ls -d "$VB_HOME"/llama/*/llama-server 2>/dev/null | head -1)}"
export VB_PROMPT_MODEL="${VB_PROMPT_MODEL:-$(ls "$VB_HOME"/models/*.gguf 2>/dev/null | head -1)}"
export VB_WHISPER_CLI="${VB_WHISPER_CLI:-$(ls "$VB_HOME"/whisper.cpp-*/build/bin/whisper-cli 2>/dev/null | head -1)}"
export VB_WHISPER_SERVER="${VB_WHISPER_SERVER:-$(ls "$VB_HOME"/whisper.cpp-*/build/bin/whisper-server 2>/dev/null | head -1)}"
export VB_WHISPER_MODEL="${VB_WHISPER_MODEL:-$(ls "$VB_HOME"/models/ggml-*.bin 2>/dev/null | head -1)}"
export VB_FUNASR_PYTHON="${VB_FUNASR_PYTHON:-$VB_HOME/venv/bin/python}"
export VB_FUNASR_MODEL="${VB_FUNASR_MODEL:-$VB_HOME/models/Fun-ASR-Nano-2512}"
export VB_ZH_DIR="${VB_ZH_DIR:-$([ -f "$VB_HOME/zh/refs.json" ] && echo "$VB_HOME/zh")}"
export VB_SAMPLE_WAV="${VB_SAMPLE_WAV:-$VB_HOME/samples/sample-en.wav}"

MANIFEST=src-tauri/Cargo.toml
WORK="$(mktemp -d)"
APP_PID=""
cleanup() {
  [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
  pkill -x TextEdit 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

make_sample() {
  [ -f "$VB_SAMPLE_WAV" ] && return
  mkdir -p "$(dirname "$VB_SAMPLE_WAV")"
  say -o "$WORK/s.aiff" "Change the use user query hook so it takes a user id. Do not send the request when there is no id, and add two tests."
  afconvert -f WAVE -d LEI16@16000 -c 1 "$WORK/s.aiff" "$VB_SAMPLE_WAV"
}

CHECK_DOC="$VB_HOME/samples/voicebridge-check.txt"
SETTINGS="$HOME/Library/Application Support/com.voicebridge.desktop/settings.json"

open_textedit() {
  pkill -x TextEdit 2>/dev/null || true
  sleep 1
  mkdir -p "$(dirname "$CHECK_DOC")"
  : > "$CHECK_DOC"
  open -a TextEdit "$CHECK_DOC"
  sleep 3
}

# The checks drag the overlay around; put it back where the user had it.
save_overlay_position() {
  [ -f "$SETTINGS" ] && cp "$SETTINGS" "$WORK/settings.before.json" || true
}
restore_overlay_position() {
  [ -f "$WORK/settings.before.json" ] || return 0
  sleep 2 # let the app finish writing on exit
  python3 - "$WORK/settings.before.json" "$SETTINGS" <<'PY'
import json, sys
before = json.load(open(sys.argv[1]))
now = json.load(open(sys.argv[2]))
now["overlay"] = before.get("overlay", {"x": None, "y": None})
json.dump(now, open(sys.argv[2], "w"), indent=2)
PY
}

models() {
  make_sample
  cargo test --manifest-path $MANIFEST --test live_models -- --ignored --nocapture --test-threads=1
}

desktop() {
  VB_LIVE_DESKTOP=1 cargo test --manifest-path $MANIFEST --test live_desktop -- --ignored --nocapture --test-threads=1
}

overlay() {
  pnpm tauri build --debug --no-bundle >/dev/null 2>&1
  save_overlay_position
  src-tauri/target/debug/voicebridge >/dev/null 2>&1 &
  APP_PID=$!
  sleep 5
  open_textedit
  local status=0
  swift scripts/overlay_check.swift "$(pgrep -x TextEdit)" || status=$?
  kill "$APP_PID"; APP_PID=""
  restore_overlay_position
  return $status
}

e2e() {
  make_sample
  pnpm tauri build --debug --no-bundle >/dev/null 2>&1
  VOICEBRIDGE_DEV_WAV="$VB_SAMPLE_WAV" src-tauri/target/debug/voicebridge >/dev/null 2>&1 &
  APP_PID=$!
  sleep 5
  open_textedit
  swift scripts/e2e_check.swift "$(pgrep -x TextEdit)" 180
  kill "$APP_PID"; APP_PID=""
}

case "${1:-}" in
  models) models ;;
  desktop) desktop ;;
  overlay) overlay ;;
  e2e) e2e ;;
  all) models; desktop; overlay; e2e ;;
  *) sed -n '2,18p' "$0"; exit 2 ;;
esac
