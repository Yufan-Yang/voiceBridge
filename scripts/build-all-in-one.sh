#!/bin/bash
# Builds a self-contained VoiceBridge.app that carries llama.cpp, whisper.cpp
# and both models inside the bundle, so nothing else has to be installed.
#
# Sources default to ~/.local/share/voicebridge (override with VB_HOME or the
# individual variables below).
set -euo pipefail
cd "$(dirname "$0")/.."

VB_HOME="${VB_HOME:-$HOME/.local/share/voicebridge}"
LLAMA_DIR="${VB_LLAMA_DIR:-$(dirname "$(ls -d "$VB_HOME"/llama/*/llama-server | head -1)")}"
WHISPER_CLI="${VB_WHISPER_CLI:-$(ls "$VB_HOME"/whisper.cpp-*/build/bin/whisper-cli | head -1)}"
PROMPT_MODEL="${VB_PROMPT_MODEL:-$(ls "$VB_HOME"/models/*.gguf | head -1)}"
WHISPER_MODEL="${VB_WHISPER_MODEL:-$(ls "$VB_HOME"/models/ggml-*.bin | head -1)}"

OUT=src-tauri/bundled
rm -rf "$OUT"
mkdir -p "$OUT/llama" "$OUT/whisper" "$OUT/models"

# llama-server finds its libraries next to itself (@loader_path). Copy real
# files, not symlinks: only the names the binaries actually link against.
cp "$LLAMA_DIR/llama-server" "$OUT/llama/"
cp -L "$LLAMA_DIR"/lib*.0.dylib "$LLAMA_DIR/libllama-server-impl.dylib" "$OUT/llama/"
[ -f "$LLAMA_DIR/LICENSE" ] && cp "$LLAMA_DIR/LICENSE" "$OUT/llama/LICENSE"
cp "$WHISPER_CLI" "$OUT/whisper/"
# Models are large: hard-link when possible instead of copying.
for model in "$PROMPT_MODEL" "$WHISPER_MODEL"; do
  ln "$model" "$OUT/models/" 2>/dev/null || cp "$model" "$OUT/models/"
done

"$OUT/llama/llama-server" --version >/dev/null 2>&1 || { echo "bundled llama-server does not start"; exit 1; }
"$OUT/whisper/whisper-cli" --help >/dev/null 2>&1 || { echo "bundled whisper-cli does not start"; exit 1; }

pnpm tauri build --config src-tauri/tauri.allinone.conf.json
APP=src-tauri/target/release/bundle/macos/VoiceBridge.app
echo
echo "Built $APP ($(du -sh "$APP" | cut -f1))"
find "$APP/Contents/Resources/bundled" -type f | sed "s|$APP/||"
