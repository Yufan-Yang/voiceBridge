# VoiceBridge

Local push-to-talk voice input for coding windows (Claude Code, Codex, Cursor, VS Code, JetBrains IDEs, terminals).

Hold a global shortcut, speak, and VoiceBridge transcribes locally, turns the speech into a structured prompt for a coding agent, and pastes it into the window you pinned. It does not press Enter unless you opt in per target. All inference runs on this machine; there are no cloud calls and no telemetry.

## Status

| Area | State |
| --- | --- |
| Pipeline, state machine, routing rules | Covered by the automated suite (mocks, no models needed) |
| llama.cpp prompt compiler | Verified live with `Qwen3-4B-Instruct-2507` Q4_K_M: compile, loopback-only listener, session key enforced, cancel, crash recovery |
| whisper.cpp ASR | Verified live with `large-v3-turbo` (q5_0) |
| Fun-ASR sidecar | Verified live with `Fun-ASR-Nano-2512` on CPU |
| macOS window pinning, activation, paste, clipboard restore | Verified live against a real TextEdit window |
| Overlay does not take focus; dragging | Verified live with simulated clicks and drags |
| Whole flow inside the app | Verified live: Push-to-Talk in a window → switch to another app → whisper.cpp → llama.cpp → paste back into the first window, with audio replayed from a WAV file |
| Microphone capture | Capture path verified through a virtual input device; **not verified with a physical microphone** (the development machine has none) |
| Windows, Linux | Interfaces only; every platform call returns `UNSUPPORTED` |

The live checks are in `scripts/live-tests.sh`; see [Live tests](#live-tests).

## Platform support

- **macOS (Apple Silicon, developed on macOS 26)**: supported.
- **Windows / Linux**: the code compiles against stub implementations in `src-tauri/src/platform/{windows,linux}.rs` that return explicit `UNSUPPORTED` errors. Window management, clipboard, key simulation and permissions are not implemented there.

## Setup

Requirements: Rust (stable), Node.js 20+, pnpm, Xcode Command Line Tools.

```sh
pnpm install
```

On the machine this was built on, Node lives in `~/.local/node/bin` and cargo in `/usr/local/Homebrew/opt/rustup/bin`; add both to `PATH`:

```sh
export PATH="$HOME/.local/node/bin:/usr/local/Homebrew/opt/rustup/bin:$PATH"
```

## Commands

| Task | Command |
| --- | --- |
| Run in development | `pnpm tauri dev` |
| Build the app bundle | `pnpm tauri build` → `src-tauri/target/release/bundle/macos/VoiceBridge.app` |
| Rust tests | `pnpm test:rust` |
| TypeScript tests | `pnpm test` |
| Type check | `pnpm typecheck` |
| Format Rust | `pnpm fmt:rust` |
| Lint Rust | `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` |
| Regenerate shared TS types | `pnpm gen:types` |

The test suites need no desktop session, microphone or model files.

### Live tests

These run the real runtimes, models and desktop. They are not part of the normal suite.

```sh
scripts/live-tests.sh models    # llama.cpp, whisper.cpp, Fun-ASR
scripts/live-tests.sh desktop   # audio capture + injection into TextEdit
scripts/live-tests.sh overlay   # overlay keeps focus in the other app; dragging
scripts/live-tests.sh e2e       # Push-to-Talk in a window → switch away → paste back, in the app
scripts/live-tests.sh all
```

- Runtimes and models are looked up under `~/.local/share/voicebridge` (override with `VB_HOME` or the `VB_*` variables listed in the script).
- `desktop`, `overlay` and `e2e` need Accessibility permission for your terminal, drive the keyboard and mouse for a few seconds, and close TextEdit when done.
- `e2e` uses the providers configured in the app's settings and replays `VOICEBRIDGE_DEV_WAV` instead of the microphone (debug builds only).

## Using it

1. Launch VoiceBridge. A small bar appears at the top of the screen; drag it anywhere (the position is remembered).
2. Click into the window you want to talk to (your editor, terminal, chat panel).
3. Hold `Ctrl+Option+Space`, speak, release.
4. The overlay shows Listening → Transcribing → Compiling prompt → Ready, then pastes the compiled prompt into that window. Click ▾ to see the raw transcription, normalized transcription and compiled prompt, each with Copy and Inject.

**Hold or tap.** Settings › "How to talk" switches between holding the keys while you speak and tap mode: press once to start, press again to stop. In tap mode recording also stops by itself after 15 seconds without speech (configurable), and always at the maximum recording length.

**Live text.** While you are still speaking, the overlay shows the words recognized so far (the audio captured up to that point is re-transcribed about every 0.7 s). It is a preview only: what gets pasted always comes from the complete recording after you stop. Live text needs the Whisper server; it is off for providers that are too slow to run repeatedly.

**Automatic hints.** The recognizer is given the project name, the current git branch (when a project directory is set for the window) and identifier-like words from the window title, such as file names and CamelCase names, in addition to any vocabulary you added.

**Enter is guarded.** With "Press Enter after paste" enabled for a window, Enter is only pressed when you said at least three words, so a stray word from an accidental press is never submitted. Scripts written without spaces are counted at about two characters per word.

**What gets pasted.** Settings › "What to paste" chooses between the prompt written by the AI model, your words tidied up (*normalized*: spacing, capital letter, final full stop, project identifiers corrected), or exactly what the recognizer heard (*raw*). Raw and normalized skip the prompt model, so they are pasted as soon as recognition finishes; the prompt can still be produced afterwards with "Recompile prompt". For languages without spaces or capitals, such as Chinese, raw and normalized are usually identical.

**Timing.** After each utterance the overlay shows how long you spoke and how long processing took; the expanded view breaks that into recognition and prompt time.

**Where the words go.** The target is the window that has focus at the moment you press Push-to-Talk. It is fixed for that utterance: if you click into another window while the models are working, the text still goes back to the first one. The target only changes when you start speaking in a different window. Windows you have spoken into appear as chips in the overlay (up to nine; the least recently used is replaced), and each keeps its own project directory, output mode and Enter-after-paste setting.

If VoiceBridge's own settings window is in front when you press the shortcut, the previous target is used.

To route by hand instead, turn off Settings › Advanced settings › Behavior › "Send to the window I am in when I start speaking". Then pin windows with `Ctrl+Option+Shift+1…9` (or from the Targets tab) and select one by clicking its chip or with `Ctrl+Option+1…9`.

With the Mock providers the transcription is a canned sample sentence, so the whole flow can be tried without any model.

### Default shortcuts

All configurable in Settings › Shortcuts. `Alt` is Option on macOS. Leave a field empty to disable it.

| Action | Default |
| --- | --- |
| Push-to-Talk (hold) | `Ctrl+Alt+Space` |
| Cancel | `Ctrl+Alt+Escape`, plus plain `Escape` while an utterance is in progress |
| Select target 1–9 | `Ctrl+Alt+1` … `Ctrl+Alt+9` |
| Pin foreground window to slot 1–9 | `Ctrl+Alt+Shift+1` … `9` |
| Previous / next target | `Ctrl+Alt+BracketLeft` / `Ctrl+Alt+BracketRight` (the `[` and `]` keys) |
| Inject previous raw transcription | `Ctrl+Alt+R` |
| Inject previous compiled prompt | `Ctrl+Alt+P` |

Plain `Escape` is registered globally only between pressing Push-to-Talk and the end of processing, so it does not interfere with editors the rest of the time.

### Target chips

- Click: select (does not activate the window).
- Double-click: select and bring the window to the front.
- Right-click: rename, project directory, output mode, Enter-after-paste, rebind, unbind.
- Mouse wheel over the bar: previous / next target.

## Model configuration

Nothing is downloaded by the app, and no download URL exists in the code. Choose runtimes and model files in Settings › Models, press Save, then **Test**.

### Prompt compiler: llama.cpp

- Runtime executable: your `llama-server` binary.
- Model path: a GGUF file. Recommended: `Qwen/Qwen3-4B-Instruct-2507`, quantization `Q4_K_M`.
- Context size 8192 and maximum output 512 tokens by default.

VoiceBridge starts `llama-server` on `127.0.0.1` with a random free port and a random per-session API key (passed through the `LLAMA_API_KEY` environment variable), and requests schema-constrained JSON.

The prompt model is asked to write only the prompt and the two confirmation fields. It no longer repeats the transcript or labels an intent, because output length is what the prompt step's time depends on; on an M2 Pro a typical request now takes 0.6–1.8 s instead of 2–3.5 s. The normalized transcription therefore comes from the local rules.

A smaller model is not a shortcut: `Qwen3-1.7B` (Q4_K_M) generated twice as fast per token but wrote far longer, rambling output, hit the token limit, translated Chinese requests into English and added requirements, ending up slower (3.6–4.7 s) and worse. `Qwen3-4B-Instruct-2507` stays the recommendation.

### ASR: Fun-ASR

- Runtime executable: a Python interpreter that has `funasr` installed.
- Model path: a local model directory. Recommended: `FunAudioLLM/Fun-ASR-Nano-2512`.

VoiceBridge runs `sidecars/funasr_sidecar.py` and talks to it over stdin/stdout (JSON lines); it opens no socket. Set `VOICEBRIDGE_ASR_DEVICE` (for example `mps`) to change the device; the default is `cpu`.

### ASR: whisper.cpp

- Runtime executable: `whisper-server` (recommended) or `whisper-cli`.
- Model path: a ggml model file. Recommended: `large-v3-turbo`.

With `whisper-server` the model stays loaded for as long as the app runs, so an utterance takes under a second on an M2 Pro and there is no reload between utterances. The server binds 127.0.0.1 on a random port; it has no API-key option, so the session token is used as a secret request-path prefix (other paths return 404). Audio is sent in memory, with no temporary file.

The final transcript always uses Whisper's full 30 s encoder window (about 0.9 s per utterance on an M2 Pro). A smaller window is twice as fast but unreliable: fitted tightly to the clip it made the model repeat whole sentences (40% character error rate on a Chinese test set, against 7% for the full window), so it is used only for the live preview, with an 8 s margin. A transcript that is one sentence repeated is also collapsed to a single copy.

**Chinese.** When the language is set to Chinese, the recognizer is primed with a short glossary of common software terms, which fixes homophone mistakes such as 重试/重视, 分支/分之, 重构/中构 and 判空/判控 and steers the output to simplified characters. On ten synthesized coding requests the character error rate is 6.6%, most of which is digits written as "30" instead of "三十". The normalized text uses full-width punctuation. Set the language explicitly rather than leaving it on automatic to get this.

**Microphone quality.** Quiet recordings are boosted to a consistent level, the silence detector adapts to quiet microphones, and audio is low-pass filtered before being reduced to 16 kHz. A Bluetooth headset microphone still limits accuracy: macOS switches the headset to a low-bitrate call mode while it records. A wired or built-in microphone gives noticeably better results.

With `whisper-cli` the program is started once per utterance and reads the audio from a temporary WAV (owner-only permissions, deleted when the request ends). This is slower, and after idle periods the first run can take about 20 s while macOS recompiles the GPU shaders.

### Mock providers

`Mock` ASR returns sample sentences and `Mock` prompt compiler does rule-based formatting. They are the defaults so the app works with no model installed.

## Permissions (macOS)

| Permission | Needed for | Where |
| --- | --- | --- |
| Microphone | Recording | Prompted on first recording; System Settings › Privacy & Security › Microphone |
| Accessibility | Window titles, bringing the target forward, verifying the foreground window, simulating paste | System Settings › Privacy & Security › Accessibility |

Settings › Privacy shows the current state of both and opens the right pane.

macOS attributes permissions to the app that launched the process:

- Started from a terminal (`pnpm tauri dev`, or running the binary directly): the **terminal** needs both permissions.
- Started from Finder as `VoiceBridge.app`: **VoiceBridge** needs them. The build is not signed with a developer identity, so macOS may ask again after each rebuild; remove the old VoiceBridge entry from the Accessibility list and add the new one if activation stops working.

## Privacy defaults

```yaml
network_inference: false   # forced off; there is no network inference path
save_audio: false
save_transcripts: false
save_history: false        # history is in memory only
auto_submit: false         # Enter is never pressed unless enabled per target
telemetry: false           # forced off; no telemetry SDK is included
```

- Logs (`~/Library/Logs/com.voicebridge.desktop/`) contain utterance ids, stage names, timings, error codes and sanitized sidecar stderr. They never contain transcripts, prompts, source code or audio.
- Settings and pinned targets are stored as JSON in `~/Library/Application Support/com.voicebridge.desktop/`.
- A settings file that cannot be parsed is replaced by the secure defaults above.

## How it is put together

```
src-tauri/src/
  pipeline.rs        orchestration: record → ASR → normalize → compile → inject
  state_machine/     pure reducer for the 11 states; rejects stale utterance ids
  target/            slots 1–9, identity matching, rebind suggestions
  injection/         GenericClipboardAdapter: verify → activate → verify → paste → restore clipboard
  asr/               AsrProvider trait; mock, fun-asr, whisper.cpp
  prompt/            PromptCompiler trait; mock, llama.cpp, JSON schema validation, fallback
  sidecar/           process lifecycle, health, crash detection, sanitized stderr log
  audio/             cpal capture on its own thread, 16 kHz resample, VAD, temp-file guard
  vocab/             per-target project vocabulary scanner
  platform/          WindowManager, ClipboardManager, KeySimulator, PermissionManager,
                     GlobalShortcutManager; macos.rs real, windows.rs / linux.rs unsupported, mock.rs
  config/            settings schema, secure defaults, ConfigStore (JSON now, SQLite-ready)
  commands/          Tauri commands
src/                 React overlay and settings UI
sidecars/            funasr_sidecar.py
```

Rust structs are the single source of truth for shared types; `src/types/generated/` is produced by `ts-rs` during `cargo test`.

### Rules the pipeline enforces

- **Frozen target**: the window in focus (or, with focus-following off, the selected target) is captured when Push-to-Talk is pressed. Transcription, compilation and injection all use it; changing the selection meanwhile only affects the next utterance.
- **One recording at a time**: repeated presses while recording are ignored.
- **Stale results are dropped**: each utterance has its own cancellation token and id; a result for anything but the current utterance is ignored.
- **Raw transcript is immutable**: the prompt compiler's result type has no raw-transcript field.
- **Compiler failure never loses the transcription**: invalid JSON gets one local repair attempt, then the normalized transcript becomes the prompt and the overlay shows "Prompt compilation failed. The transcription has been preserved."
- **Injection is verified**: window id, process id and executable must match the pinned target, and the foreground window is re-read after activation and again right before the paste keystroke. Any failed check stops the injection.
- **Clipboard**: the previous clipboard is restored after pasting unless its version changed in the meantime (you copied something), in which case it is left alone.

## Decisions made where the brief was open

- **Auto-inject is skipped** when prompt compilation failed or the model set `needs_confirmation`; inject manually from the expanded overlay.
- **Re-injecting an old utterance** goes to the target it was recorded for. Only an utterance recorded with no target selected uses the current selection.
- **Recording with no target pinned is allowed**; the result is shown and can be copied.
- **Rename** from the chip menu opens Settings › Targets, because the overlay never takes keyboard focus and so cannot host a text field.
- **"Recently opened files"** in the vocabulary are approximated by the most recently modified source files.
- **Application identity** uses the executable path (from `proc_pidpath`) rather than the bundle identifier.
- **"Automatically submit after injection"** in Settings › Behavior sets the default for targets pinned afterwards; existing targets keep their own setting.
- **WAV simulation** (Settings › Audio › Development) exists only in debug builds; the command is rejected in release builds.

## Known limitations

- Only top-level OS windows can be targeted. Browser tabs, terminal tabs, and multiple chat panels inside one IDE window cannot be distinguished; the paste goes wherever that window has keyboard focus.
- Only plain-text clipboard contents are saved and restored. If the clipboard held an image or files before an injection, it is empty afterwards.
- A target window on another Space, or in native full screen on another display, may not be reachable through the Accessibility API; injection then stops with `TARGET_ACTIVATION_FAILED`.
- Window titles are empty until Accessibility permission is granted.
- Window ids do not survive an app or window restart. Targets come back as offline with a suggested match that you must confirm; nothing is rebound automatically.
- Mapping an accessibility window to its window id uses `_AXUIElementGetWindow`, a private macOS API (long-stable, used by many window managers).
- The overlay is opaque with square corners (no transparency, to avoid Tauri's macOS private-API flag).
- The Fun-ASR sidecar loads the model at start, so the first request can take long; press **Test ASR model** once to warm it up. Cancelling a Fun-ASR request kills the sidecar, which restarts on the next request.
- Shortcut fields are free text; an invalid or already-taken shortcut is skipped and noted in the log rather than flagged in the UI.
- The identifier correction in the normalizer handles ASCII identifiers only.
- If the Mac has no usable default input device (for example a Mac mini with only virtual meeting devices), Push-to-Talk reports `MIC_DEVICE_NOT_FOUND`. Connect a microphone, or pick a device in Settings › Audio.
- The prompt model sees the target application's name and may mention it in the prompt (for example "in the TextEdit application"). Setting a project directory and vocabulary terms for the target gives it better context.
- Without vocabulary terms the models keep spoken forms such as "use user query"; add terms or a project directory to get `useUserQuery`.
- Launching the app briefly makes it the frontmost application, as any app launch does; after that the overlay never takes focus.
- If VoiceBridge is force-killed (`kill -9`) a running `llama-server` is left behind; it is detected and stopped the next time the app starts. A normal quit, SIGTERM and SIGINT stop it immediately.
- The non-activating overlay relies on `_setPreventsActivation:`, a private AppKit method (guarded by a `respondsToSelector:` check).
- Not verified: speech from a physical microphone, and behaviour on multiple displays.
