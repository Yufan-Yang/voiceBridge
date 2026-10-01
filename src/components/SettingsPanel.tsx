import { useCallback, useEffect, useState, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api, toAppError } from "../services/api";
import { slotGlyph } from "../services/format";
import { useAppStore } from "../stores/appStore";
import type {
  AppError,
  AsrProviderKind,
  OutputKind,
  PermissionKind,
  PermissionsSnapshot,
  PromptProviderKind,
  ProviderHealth,
  Settings,
  TargetSlot,
  WindowInfo,
} from "../types";

type Tab = "targets" | "models" | "audio" | "shortcuts" | "behavior" | "privacy";

const TABS: { id: Tab; label: string }[] = [
  { id: "targets", label: "Targets" },
  { id: "models", label: "Models" },
  { id: "audio", label: "Audio" },
  { id: "shortcuts", label: "Shortcuts" },
  { id: "behavior", label: "Behavior" },
  { id: "privacy", label: "Privacy" },
];

const SLOTS = [1, 2, 3, 4, 5, 6, 7, 8, 9];
const LANGUAGES: { value: string; label: string }[] = [
  { value: "", label: "Detect automatically" },
  { value: "en", label: "English" },
  { value: "zh", label: "Chinese (中文)" },
  { value: "ja", label: "Japanese (日本語)" },
  { value: "ko", label: "Korean (한국어)" },
  { value: "es", label: "Spanish" },
  { value: "fr", label: "French" },
  { value: "de", label: "German" },
];
const OUTPUTS: { value: OutputKind; label: string }[] = [
  { value: "prompt", label: "Compiled prompt" },
  { value: "normalized", label: "Normalized transcription" },
  { value: "raw", label: "Raw transcription" },
];

const ASR_HELP: Record<AsrProviderKind, string> = {
  whisper_cpp: "Listens to your recording and writes down what you said. Works for many languages.",
  fun_asr: "An alternative recognizer that is strong at Chinese. You must install it yourself and set it up under Advanced.",
  mock: "Does not listen at all: it returns a sample sentence so you can try the rest of the app.",
};

const OUTPUT_HELP: Record<OutputKind, string> = {
  prompt: "Your words are rewritten into a clear, structured request. Adds a few seconds for the AI model.",
  normalized: "Your own words with spacing, capital letter and final full stop fixed, and project names corrected. Pasted as soon as recognition finishes.",
  raw: "The recognizer’s output with no changes at all. Pasted as soon as recognition finishes.",
};

const PROMPT_HELP: Record<PromptProviderKind, string> = {
  llama_cpp: "An AI model on this Mac turns what you said into a tidy, structured request for your coding assistant.",
  mock: "No AI model: only fixes punctuation and splits your sentence into a list.",
};

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <span className="field-control">{children}</span>
      {hint ? <span className="field-hint">{hint}</span> : null}
    </label>
  );
}

function PathField(props: {
  label: string;
  hint?: string;
  value: string;
  directory?: boolean;
  placeholder?: string;
  onChange: (value: string) => void;
}) {
  const browse = async () => {
    const picked = await open({ directory: props.directory ?? false, multiple: false, title: props.label });
    if (typeof picked === "string") props.onChange(picked);
  };
  return (
    <Field label={props.label} hint={props.hint}>
      <input
        type="text"
        value={props.value}
        placeholder={props.placeholder}
        spellCheck={false}
        onChange={(e) => props.onChange(e.target.value)}
      />
      <button type="button" onClick={() => void browse()}>
        Browse…
      </button>
    </Field>
  );
}

function Toggle({ label, hint, checked, onChange }: { label: string; hint?: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="toggle">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>
        {label}
        {hint ? <span className="field-hint">{hint}</span> : null}
      </span>
    </label>
  );
}

function NumberField(props: { label: string; hint?: string; value: number; min: number; max: number; onChange: (v: number) => void }) {
  return (
    <Field label={props.label} hint={props.hint}>
      <input
        type="number"
        value={props.value}
        min={props.min}
        max={props.max}
        onChange={(e) => props.onChange(Number(e.target.value) || 0)}
      />
    </Field>
  );
}

const HEALTH_LABELS: Record<ProviderHealth["status"], string> = {
  ready: "Ready",
  starting: "Starting…",
  unavailable: "Not running yet — it starts when first used",
  not_configured: "Not set up — choose the files under Advanced",
};

function HealthBadge({ health }: { health: ProviderHealth | undefined }) {
  if (!health) return <span className="badge">checking…</span>;
  return (
    <span className={`badge badge-${health.status}`} title={`${health.provider}: ${health.message}`}>
      {HEALTH_LABELS[health.status]}
    </span>
  );
}

export function SettingsPanel() {
  const snapshot = useAppStore((s) => s.snapshot);
  const level = useAppStore((s) => s.level);
  const health = useAppStore((s) => s.health);
  const setHealth = useAppStore((s) => s.setHealth);

  const [tab, setTab] = useState<Tab>("targets");
  // The basic view shows only what a user has to decide; everything else
  // keeps its default and lives behind "Advanced settings".
  const [advanced, setAdvanced] = useState(false);
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [status, setStatus] = useState<string>("");
  const [error, setError] = useState<AppError | null>(null);
  const [devices, setDevices] = useState<string[]>([]);
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [permissions, setPermissions] = useState<PermissionsSnapshot | null>(null);
  const [testing, setTesting] = useState<string>("");
  const [bundled, setBundled] = useState<string[]>([]);

  /** Runs a command, reporting success or the AppError in the footer. */
  const act = useCallback(async <T,>(action: () => Promise<T>, done?: string): Promise<T | undefined> => {
    setError(null);
    try {
      const value = await action();
      if (done) setStatus(done);
      return value;
    } catch (e) {
      setStatus("");
      setError(toAppError(e));
      return undefined;
    }
  }, []);

  const refreshWindows = useCallback(() => void act(api.listDesktopWindows).then((w) => w && setWindows(w)), [act]);
  const refreshHealth = useCallback(() => void act(api.getProviderHealth).then((h) => h && setHealth(h)), [act, setHealth]);

  useEffect(() => {
    void act(api.getSettings).then((s) => {
      if (s) {
        setSaved(s);
        setDraft(s);
      }
    });
    void act(api.listAudioDevices).then((d) => d && setDevices(d));
    void act(api.getPermissions).then((p) => p && setPermissions(p));
    void act(api.getBundledAssets).then((b) => b && setBundled(b));
    refreshWindows();
    refreshHealth();
    const off = listen("show_targets_tab", () => {
      setAdvanced(true);
      setTab("targets");
    });
    return () => {
      void off.then((f) => f());
    };
  }, [act, refreshWindows, refreshHealth]);

  if (!draft || !saved) {
    return <div className="settings loading">{error ? error.message : "Loading settings…"}</div>;
  }

  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const patch = <K extends keyof Settings>(section: K, values: Partial<Settings[K]>) =>
    setDraft({ ...draft, [section]: { ...draft[section], ...values } });

  const save = async () => {
    const applied = await act(() => api.updateSettings(draft), "Settings saved.");
    if (applied) {
      setSaved(applied);
      setDraft(applied);
      void emit("settings_updated");
      refreshHealth();
    }
  };

  const testAsr = async () => {
    setTesting("asr");
    const h = await act(api.restartAsrSidecar, "ASR runtime is ready.");
    if (h) setHealth(h);
    else refreshHealth();
    setTesting("");
  };

  const testPrompt = async () => {
    setTesting("prompt");
    const started = performance.now();
    const h = await act(api.restartPromptSidecar);
    if (h) {
      setHealth(h);
      const out = await act(() =>
        api.compilePrompt("rename the config loader function to load settings and update every caller"),
      );
      if (out) setStatus(`The prompt model is working (answered in ${Math.round(performance.now() - started)} ms).`);
    } else {
      refreshHealth();
    }
    setTesting("");
  };

  const requestPermission = async (kind: PermissionKind) => {
    const p = await act(() => api.requestPermission(kind));
    if (p) setPermissions(p);
  };

  const targetAction = (action: () => Promise<unknown>, done?: string) => void act(action, done).then(refreshWindows);

  const renderTarget = (t: TargetSlot) => {
    const suggestion = snapshot.suggestions.find((s) => s.target_id === t.id);
    return (
      <div key={t.id} className={`target-card${t.status === "offline" ? " offline" : ""}`}>
        <div className="target-head">
          <span className="target-slot">{slotGlyph(t.slot)}</span>
          <input
            type="text"
            defaultValue={t.alias}
            key={`${t.id}-${t.alias}`}
            aria-label="Alias"
            onBlur={(e) => {
              const alias = e.target.value.trim();
              if (alias && alias !== t.alias) targetAction(() => api.renameTarget(t.id, alias), "Target renamed.");
            }}
          />
          <span className={`badge badge-${t.status === "online" ? "ready" : "unavailable"}`}>{t.status}</span>
          {t.id === snapshot.selected_target_id ? <span className="badge badge-ready">selected</span> : null}
          <button type="button" onClick={() => targetAction(() => api.selectTarget(t.id))}>
            Select
          </button>
          <button type="button" onClick={() => targetAction(() => api.unbindTarget(t.id), "Target unbound.")}>
            Unbind
          </button>
        </div>
        <div className="target-meta" title={t.executable_or_bundle_id}>
          {t.title_hint || "(untitled window)"} · pid {t.process_id} · window {t.platform_window_id}
        </div>
        {suggestion ? (
          <div className="banner banner-warning">
            The pinned window is closed. Likely match: “{suggestion.window.title || suggestion.window.app_name}”.
            <button
              type="button"
              onClick={() =>
                targetAction(() => api.confirmRebind(t.id, suggestion.window.platform_window_id), "Target rebound.")
              }
            >
              Rebind to this window
            </button>
          </div>
        ) : null}
        <div className="target-row">
          <span className="field-label">Project</span>
          <span className="target-path">{t.project_root ?? "not set"}</span>
          <button
            type="button"
            onClick={() =>
              void open({ directory: true, multiple: false, title: "Project directory" }).then((dir) => {
                if (typeof dir === "string") targetAction(() => api.setProjectRoot(t.id, dir), "Project scanned.");
              })
            }
          >
            Choose…
          </button>
          {t.project_root ? (
            <>
              <button
                type="button"
                onClick={() =>
                  void act(() => api.refreshProjectTerms(t.id)).then((terms) => {
                    if (terms) setStatus(`${terms.length} vocabulary terms for ${t.alias}.`);
                  })
                }
              >
                Rescan
              </button>
              <button type="button" onClick={() => targetAction(() => api.setProjectRoot(t.id, null))}>
                Clear
              </button>
            </>
          ) : null}
        </div>
        <div className="target-row">
          <span className="field-label">Output</span>
          <select
            value={t.preferred_output}
            onChange={(e) => targetAction(() => api.setTargetOutput(t.id, e.target.value as OutputKind))}
          >
            {OUTPUTS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <label className="toggle inline">
            <input
              type="checkbox"
              checked={t.auto_submit}
              onChange={(e) => targetAction(() => api.setTargetAutoSubmit(t.id, e.target.checked))}
            />
            <span>Press Enter after paste</span>
          </label>
        </div>
        <div className="target-row">
          <span className="field-label">Terms</span>
          <input
            type="text"
            className="grow"
            defaultValue={t.manual_terms.join(", ")}
            key={`${t.id}-${t.manual_terms.join("|")}`}
            placeholder="useUserQuery, userId, …"
            spellCheck={false}
            onBlur={(e) => {
              const terms = e.target.value.split(",").map((s) => s.trim()).filter(Boolean);
              if (terms.join("|") !== t.manual_terms.join("|")) {
                targetAction(() => api.setTargetTerms(t.id, terms), "Vocabulary updated.");
              }
            }}
          />
        </div>
      </div>
    );
  };

  const hasBuiltIn = bundled.length > 0;
  const builtInPlaceholder = hasBuiltIn ? "Built in — leave empty" : "Choose a file…";
  const boundIds = new Set(snapshot.targets.map((t) => t.platform_window_id));
  const currentTarget = snapshot.targets.find((t) => t.id === snapshot.selected_target_id) ?? null;
  const missingPermissions = (["microphone", "accessibility"] as const).filter(
    (kind) => permissions !== null && permissions[kind] !== "granted",
  );
  const modelsReady = saved.models.asr_provider !== "mock" && saved.models.prompt_provider !== "mock";
  const { models, audio, shortcuts, behavior, privacy } = draft;

  return (
    <div className="settings">
      <nav className="tabs">
        {advanced ? (
          TABS.map((t) => (
            <button key={t.id} type="button" className={tab === t.id ? "active" : ""} onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))
        ) : (
          <span className="basic-title">VoiceBridge</span>
        )}
        <span className={`offline-pill${snapshot.offline ? "" : " online"}`}>
          {snapshot.offline ? "● Fully offline" : "● Network inference on"}
        </span>
      </nav>

      {!advanced ? (
        <main className="tab-body basic">
          {missingPermissions.length > 0 ? (
            <section>
              <h2>Allow access</h2>
              {missingPermissions.map((kind) => (
                <div key={kind} className="row">
                  <span className="grow">
                    {kind === "microphone"
                      ? "Microphone — to hear you."
                      : "Accessibility — to paste into your coding window."}
                  </span>
                  <button type="button" className="primary" onClick={() => void requestPermission(kind)}>
                    Allow…
                  </button>
                </div>
              ))}
            </section>
          ) : null}

          <section>
            <h2>Where your words go</h2>
            <p className="note">
              Into the window you are typing in when you start speaking. If you click somewhere else while VoiceBridge is
              working, the text still goes to that first window. Nothing to set up.
            </p>
            {currentTarget ? (
              <div className="window-row">
                <span className="window-name">
                  Last used: <strong>{currentTarget.alias}</strong> {currentTarget.title_hint}
                </span>
              </div>
            ) : null}
          </section>

          <section>
            <h2>Speaking</h2>
            <Field label="Microphone">
              <select
                value={audio.input_device ?? ""}
                onChange={(e) => patch("audio", { input_device: e.target.value || null })}
              >
                <option value="">System default</option>
                {devices.map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Language">
              <select value={models.asr_language} onChange={(e) => patch("models", { asr_language: e.target.value })}>
                {LANGUAGES.map((l) => (
                  <option key={l.value} value={l.value}>
                    {l.label}
                  </option>
                ))}
                {LANGUAGES.some((l) => l.value === models.asr_language) ? null : (
                  <option value={models.asr_language}>{models.asr_language}</option>
                )}
              </select>
            </Field>
            <Field label="What to paste" hint={OUTPUT_HELP[behavior.default_output]}>
              <select
                value={behavior.default_output}
                onChange={(e) => patch("behavior", { default_output: e.target.value as OutputKind })}
              >
                <option value="prompt">A prompt written by the AI model</option>
                <option value="normalized">What I said, tidied up (faster)</option>
                <option value="raw">Exactly what was heard (faster)</option>
              </select>
            </Field>
            <Field label="Hold to talk" hint="Hold these keys while you speak, then let go. Alt is the Option key.">
              <input
                type="text"
                value={shortcuts.push_to_talk}
                spellCheck={false}
                onChange={(e) => patch("shortcuts", { push_to_talk: e.target.value })}
              />
            </Field>
          </section>

          {!modelsReady ? (
            <div className="banner banner-warning">
              The AI models are not set up, so VoiceBridge is using demo text.
              <button
                type="button"
                onClick={() => {
                  setAdvanced(true);
                  setTab("models");
                }}
              >
                Set up models
              </button>
            </div>
          ) : null}
        </main>
      ) : null}

      <main className="tab-body" hidden={!advanced}>
        {tab === "targets" ? (
          <>
            <h2>Pinned targets</h2>
            <p className="note">
              Windows you speak into are added here automatically. Pinning by hand is only needed when “Send to the
              window I am in” is turned off (Behavior).
            </p>
            <p className="note">
              Up to nine top-level windows. Tabs or panels inside one window cannot be told apart. A target is never
              rebound automatically.
            </p>
            {snapshot.targets.length === 0 ? <p className="note">Nothing pinned yet.</p> : snapshot.targets.map(renderTarget)}
            <h2>
              Open windows{" "}
              <button type="button" onClick={refreshWindows}>
                Refresh
              </button>
            </h2>
            {permissions && permissions.accessibility !== "granted" ? (
              <p className="note">Window titles need Accessibility permission (see Privacy).</p>
            ) : null}
            <div className="window-list">
              {windows.map((w) => (
                <div key={w.platform_window_id} className="window-row">
                  <span className="window-name">
                    <strong>{w.app_name}</strong> {w.title}
                  </span>
                  {boundIds.has(w.platform_window_id) ? (
                    <span className="badge badge-ready">pinned</span>
                  ) : (
                    <select
                      value=""
                      onChange={(e) => {
                        const slot = Number(e.target.value);
                        if (slot) targetAction(() => api.bindWindowToSlot(w.platform_window_id, slot), `Pinned to slot ${slot}.`);
                      }}
                    >
                      <option value="">Pin to slot…</option>
                      {SLOTS.map((s) => (
                        <option key={s} value={s}>
                          Slot {s}
                          {snapshot.targets.some((t) => t.slot === s) ? " (replace)" : ""}
                        </option>
                      ))}
                    </select>
                  )}
                </div>
              ))}
              {windows.length === 0 ? <p className="note">No windows found.</p> : null}
            </div>
          </>
        ) : null}

        {tab === "models" ? (
          <>
            <div className="banner banner-ok">
              {hasBuiltIn
                ? "Everything needed is built into this app and runs on this Mac. You do not have to change anything on this page."
                : "This build has no built-in AI models. Choose your own files under Advanced, or use the demo options to try the app."}
            </div>

            <h2>Step 1 — Turning your speech into text</h2>
            <Field label="Speech recognition" hint={ASR_HELP[models.asr_provider]}>
              <select
                value={models.asr_provider}
                onChange={(e) => patch("models", { asr_provider: e.target.value as AsrProviderKind })}
              >
                <option value="whisper_cpp">Whisper{hasBuiltIn ? " (built in)" : ""}</option>
                <option value="fun_asr">Fun-ASR (needs your own Python setup)</option>
                <option value="mock">Demo (no real recognition)</option>
              </select>
            </Field>
            <Field label="Language you speak" hint="Leave empty to detect it automatically. Examples: en for English, zh for Chinese.">
              <input
                type="text"
                value={models.asr_language}
                placeholder="automatic"
                onChange={(e) => patch("models", { asr_language: e.target.value })}
              />
            </Field>
            <div className="row">
              <button type="button" disabled={testing !== "" || dirty} onClick={() => void testAsr()}>
                {testing === "asr" ? "Checking…" : "Check that it works"}
              </button>
              <HealthBadge health={health?.asr} />
            </div>

            <h2>Step 2 — Rewriting the text as a clear coding prompt</h2>
            <Field label="Prompt writer" hint={PROMPT_HELP[models.prompt_provider]}>
              <select
                value={models.prompt_provider}
                onChange={(e) => patch("models", { prompt_provider: e.target.value as PromptProviderKind })}
              >
                <option value="llama_cpp">Local AI model{hasBuiltIn ? " (built in)" : ""}</option>
                <option value="mock">Demo (simple tidy-up, no AI model)</option>
              </select>
            </Field>
            <div className="row">
              <button type="button" disabled={testing !== "" || dirty} onClick={() => void testPrompt()}>
                {testing === "prompt" ? "Loading the model…" : "Check that it works"}
              </button>
              <HealthBadge health={health?.prompt} />
            </div>
            {dirty ? <p className="note">Press Save first, then check.</p> : null}

            <details className="advanced">
              <summary>Advanced — use my own programs or model files</summary>
              <p className="note">
                Only needed if you want a different model than the built-in one. An empty box means “use the built-in
                file”.
              </p>
              <PathField
                label="Speech program"
                hint={
                  models.asr_provider === "fun_asr"
                    ? "The Python program (python) that has the funasr package installed."
                    : "The whisper-cli program from whisper.cpp."
                }
                placeholder={builtInPlaceholder}
                value={models.asr_runtime_path}
                onChange={(v) => patch("models", { asr_runtime_path: v })}
              />
              <PathField
                label="Speech model"
                hint={
                  models.asr_provider === "fun_asr"
                    ? "The Fun-ASR model folder, for example Fun-ASR-Nano-2512."
                    : "A Whisper model file (ggml-….bin), for example large-v3-turbo."
                }
                placeholder={builtInPlaceholder}
                value={models.asr_model_path}
                directory={models.asr_provider === "fun_asr"}
                onChange={(v) => patch("models", { asr_model_path: v })}
              />
              <PathField
                label="Prompt program"
                hint="The llama-server program from llama.cpp. VoiceBridge starts and stops it for you."
                placeholder={builtInPlaceholder}
                value={models.prompt_runtime_path}
                onChange={(v) => patch("models", { prompt_runtime_path: v })}
              />
              <PathField
                label="Prompt model"
                hint={`An AI model file ending in .gguf, for example ${models.prompt_model_id} (${models.prompt_quantization}).`}
                placeholder={builtInPlaceholder}
                value={models.prompt_model_path}
                onChange={(v) => patch("models", { prompt_model_path: v })}
              />
              <NumberField
                label="Model memory (context size)"
                hint="How much text the prompt model can consider at once. 8192 is plenty; larger uses more memory."
                value={models.context_size}
                min={512}
                max={131072}
                onChange={(v) => patch("models", { context_size: v })}
              />
              <NumberField
                label="Longest prompt (output tokens)"
                hint="Upper limit on the length of the written prompt. 512 is roughly 350 words."
                value={models.max_output_tokens}
                min={16}
                max={8192}
                onChange={(v) => patch("models", { max_output_tokens: v })}
              />
              {bundled.length > 0 ? <p className="note">Built into this app: {bundled.join(", ")}</p> : null}
            </details>
          </>
        ) : null}

        {tab === "audio" ? (
          <>
            <h2>Audio</h2>
            <Field label="Microphone device">
              <select
                value={audio.input_device ?? ""}
                onChange={(e) => patch("audio", { input_device: e.target.value || null })}
              >
                <option value="">System default</option>
                {devices.map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
                {audio.input_device && !devices.includes(audio.input_device) ? (
                  <option value={audio.input_device}>{audio.input_device} (not connected)</option>
                ) : null}
              </select>
              <button type="button" onClick={() => void act(api.listAudioDevices).then((d) => d && setDevices(d))}>
                Refresh
              </button>
            </Field>
            <NumberField
              label="Maximum recording duration (seconds)"
              value={audio.max_recording_secs}
              min={1}
              max={600}
              onChange={(v) => patch("audio", { max_recording_secs: v })}
            />
            <Field label="Input level" hint="Hold the Push-to-Talk shortcut to see the live level.">
              <span className="level wide">
                <span style={{ width: `${Math.round(level * 100)}%` }} />
              </span>
            </Field>
            <Toggle
              label="Voice activity detection"
              hint="Trims silence before and after speech."
              checked={audio.vad_enabled}
              onChange={(v) => patch("audio", { vad_enabled: v })}
            />
            {snapshot.dev_mode ? (
              <>
                <h2>Development</h2>
                <div className="row">
                  <button
                    type="button"
                    onClick={() =>
                      void open({ multiple: false, filters: [{ name: "WAV audio", extensions: ["wav"] }] }).then((file) => {
                        if (typeof file === "string") void act(() => api.transcribeAudio(file), "Simulated utterance started.");
                      })
                    }
                  >
                    Simulate utterance from WAV…
                  </button>
                  <span className="field-hint">Only available in development builds.</span>
                </div>
              </>
            ) : null}
          </>
        ) : null}

        {tab === "shortcuts" ? (
          <>
            <h2>Shortcuts</h2>
            <p className="note">
              Format: modifiers and a key joined by +, e.g. Ctrl+Alt+Space (Alt is Option on macOS). Leave empty to
              disable. Escape also cancels while an utterance is in progress.
            </p>
            {(
              [
                ["push_to_talk", "Push-to-Talk (hold)"],
                ["cancel", "Cancel"],
                ["prev_target", "Previous target"],
                ["next_target", "Next target"],
                ["inject_last_raw", "Inject previous raw transcription"],
                ["inject_last_prompt", "Inject previous compiled prompt"],
              ] as const
            ).map(([key, label]) => (
              <Field key={key} label={label}>
                <input type="text" value={shortcuts[key]} spellCheck={false} onChange={(e) => patch("shortcuts", { [key]: e.target.value })} />
              </Field>
            ))}
            <h2>Select target 1–9</h2>
            <div className="grid-3">
              {shortcuts.select_slot.map((value, i) => (
                <Field key={i} label={`Slot ${i + 1}`}>
                  <input
                    type="text"
                    value={value}
                    spellCheck={false}
                    onChange={(e) =>
                      patch("shortcuts", { select_slot: shortcuts.select_slot.map((v, j) => (j === i ? e.target.value : v)) })
                    }
                  />
                </Field>
              ))}
            </div>
            <h2>Pin foreground window to slot 1–9</h2>
            <div className="grid-3">
              {shortcuts.bind_slot.map((value, i) => (
                <Field key={i} label={`Slot ${i + 1}`}>
                  <input
                    type="text"
                    value={value}
                    spellCheck={false}
                    onChange={(e) =>
                      patch("shortcuts", { bind_slot: shortcuts.bind_slot.map((v, j) => (j === i ? e.target.value : v)) })
                    }
                  />
                </Field>
              ))}
            </div>
          </>
        ) : null}

        {tab === "behavior" ? (
          <>
            <h2>Behavior</h2>
            <Field label="Default output type" hint="Used for newly pinned targets. Each target can override it.">
              <select
                value={behavior.default_output}
                onChange={(e) => patch("behavior", { default_output: e.target.value as OutputKind })}
              >
                {OUTPUTS.map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </Field>
            <Toggle
              label="Send to the window I am in when I start speaking"
              hint="On: no pinning needed. Off: text always goes to the target you selected by hand."
              checked={behavior.follow_focus}
              onChange={(v) => patch("behavior", { follow_focus: v })}
            />
            <Toggle
              label="Automatically inject after processing"
              hint="Pastes into the target frozen when recording began. Never happens when the prompt failed or needs confirmation."
              checked={behavior.auto_inject}
              onChange={(v) => patch("behavior", { auto_inject: v })}
            />
            <Toggle
              label="Automatically submit after injection (new targets)"
              hint="Presses Enter after pasting. Off by default; applies to targets pinned from now on."
              checked={behavior.auto_submit}
              onChange={(v) => patch("behavior", { auto_submit: v })}
            />
            <Toggle
              label="Save history"
              hint="History is kept in memory only unless this is enabled."
              checked={behavior.save_history}
              onChange={(v) => patch("behavior", { save_history: v })}
            />
            <NumberField
              label="Completion notification duration (ms)"
              value={behavior.completion_notice_ms}
              min={500}
              max={60000}
              onChange={(v) => patch("behavior", { completion_notice_ms: v })}
            />
            <Toggle
              label="Mouse wheel cycles targets"
              checked={behavior.wheel_cycles_targets}
              onChange={(v) => patch("behavior", { wheel_cycles_targets: v })}
            />
          </>
        ) : null}

        {tab === "privacy" ? (
          <>
            <h2>Privacy</h2>
            <div className={`banner ${snapshot.offline ? "banner-ok" : "banner-warning"}`}>
              {snapshot.offline
                ? "Operating fully offline. Audio, transcripts, prompts and window metadata never leave this computer. Model runtimes only listen on 127.0.0.1 or use stdio. No telemetry is included."
                : "Network inference is enabled."}
            </div>
            <Toggle
              label="Save audio"
              hint="Off by default. When on, recordings are written to the application data folder."
              checked={privacy.save_audio}
              onChange={(v) => patch("privacy", { save_audio: v })}
            />
            <Toggle
              label="Save transcripts"
              hint="Off by default. When on, raw transcriptions are appended to a local file."
              checked={privacy.save_transcripts}
              onChange={(v) => patch("privacy", { save_transcripts: v })}
            />
            <div className="row">
              <button type="button" onClick={() => void act(api.clearHistory, "History cleared.")}>
                Clear history
              </button>
              <button
                type="button"
                onClick={() => void act(api.clearTempFiles).then((n) => n !== undefined && setStatus(`${n} temporary files removed.`))}
              >
                Clear temporary files
              </button>
            </div>
            <h2>Permissions</h2>
            {(["microphone", "accessibility"] as const).map((kind) => (
              <div key={kind} className="row">
                <span className="field-label">{kind === "microphone" ? "Microphone" : "Accessibility"}</span>
                <span className={`badge badge-${permissions?.[kind] === "granted" ? "ready" : "unavailable"}`}>
                  {permissions ? permissions[kind].replace("_", " ") : "…"}
                </span>
                <button type="button" onClick={() => void requestPermission(kind)}>
                  Open system settings…
                </button>
              </div>
            ))}
            <p className="note">
              Microphone is needed to record. Accessibility is needed to bring the target window forward, verify it, and
              simulate paste.
            </p>
          </>
        ) : null}
      </main>

      <footer className="settings-footer">
        <span className={error ? "footer-error" : "footer-status"} role={error ? "alert" : undefined}>
          {error ? `${error.message} ${error.recovery}` : status}
        </span>
        <button type="button" className="link" onClick={() => setAdvanced((v) => !v)}>
          {advanced ? "Basic settings" : "Advanced settings"}
        </button>
        {dirty ? (
          <button type="button" onClick={() => setDraft(saved)}>
            Revert
          </button>
        ) : null}
        <button type="button" className="primary" disabled={!dirty} onClick={() => void save()}>
          Save
        </button>
      </footer>
    </div>
  );
}
