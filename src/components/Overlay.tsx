import { useEffect, useRef, useState } from "react";
import { emit } from "@tauri-apps/api/event";
import { LogicalSize, getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import { run } from "../hooks/useBackendEvents";
import { api } from "../services/api";
import {
  formatElapsed,
  isBusy,
  isIndeterminate,
  phaseTone,
  recordingProgress,
  resultSummary,
  slotGlyph,
  targetLabel,
} from "../services/format";
import { useAppStore } from "../stores/appStore";
import type { OutputKind } from "../types";
import { ProgressBar } from "./ProgressBar";
import { ResultPreview } from "./ResultPreview";
import { StatusIndicator } from "./StatusIndicator";
import { TargetChip } from "./TargetChip";

const WIDTH = 420;
const BAR_HEIGHT = 46;
const EXPANDED_HEIGHT = 380;
const OUTPUTS: OutputKind[] = ["raw", "normalized", "prompt"];

/** Re-renders while recording so the elapsed time advances. */
function useElapsed(startedMs: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedMs === null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 200);
    return () => window.clearInterval(timer);
  }, [startedMs]);
  return startedMs === null ? 0 : Math.max(0, now - startedMs);
}

export function Overlay() {
  const snapshot = useAppStore((s) => s.snapshot);
  const level = useAppStore((s) => s.level);
  const interim = useAppStore((s) => s.interim);
  const actionError = useAppStore((s) => s.actionError);
  const setActionError = useAppStore((s) => s.setActionError);
  const [pinnedOpen, setPinnedOpen] = useState(false);
  const [menuId, setMenuId] = useState<string | null>(null);
  const [wheelCycles, setWheelCycles] = useState(true);
  const lastWheel = useRef(0);

  const { phase, targets, last_result: result } = snapshot;
  const elapsed = useElapsed(phase === "LISTENING" ? snapshot.recording_started_ms : null);
  const busy = isBusy(phase);
  const error = snapshot.error ?? actionError;
  // Expand temporarily after completion or on problems; otherwise stay minimal.
  const menuOpen = menuId !== null && !busy && targets.some((t) => t.id === menuId);
  const expanded = menuOpen || pinnedOpen || phase === "READY" || phase === "ERROR" || (!busy && actionError !== null);

  useEffect(() => {
    void getCurrentWindow().setSize(new LogicalSize(WIDTH, expanded ? EXPANDED_HEIGHT : BAR_HEIGHT));
  }, [expanded]);

  useEffect(() => {
    void api.getSettings().then((s) => setWheelCycles(s.behavior.wheel_cycles_targets));
  }, [phase]);

  const frozen = targets.find((t) => t.id === snapshot.frozen_target_id) ?? null;
  const routed = frozen ?? targets.find((t) => t.id === result?.frozen_target_id) ?? null;
  const showChips = phase === "IDLE" || phase === "CANCELED";
  const defaultKind: OutputKind =
    targets.find((t) => t.id === result?.frozen_target_id)?.preferred_output ?? "prompt";

  // Target options live in the overlay itself: items of a native popup menu
  // never reached the page from this non-activating panel.
  const menuTarget = targets.find((t) => t.id === menuId) ?? null;
  const menuSuggestion = menuTarget ? snapshot.suggestions.find((s) => s.target_id === menuTarget.id) : undefined;

  const onWheel = (deltaY: number) => {
    if (!wheelCycles || !showChips || targets.length < 2) return;
    const now = Date.now();
    if (now - lastWheel.current < 180 || deltaY === 0) return;
    lastWheel.current = now;
    void run(() => api.selectRelativeTarget(deltaY > 0 ? 1 : -1));
  };

  let detail: string | undefined;
  if (phase === "LISTENING") detail = formatElapsed(elapsed);
  else if ((phase === "READY" || phase === "DONE") && result) detail = resultSummary(result);
  else if (phase === "ERROR" && error) detail = error.message;

  return (
    <div className={`overlay tone-${phaseTone(phase)}${expanded ? " expanded" : ""}`} onContextMenu={(e) => e.preventDefault()}>
      <div className="bar" data-tauri-drag-region onWheel={(e) => onWheel(e.deltaY)}>
        <StatusIndicator phase={phase} detail={detail} />

        {phase === "LISTENING" ? (
          <>
            <ProgressBar tone="recording" value={recordingProgress(elapsed, snapshot.max_recording_secs)} />
            <span className="level" aria-label="Input level" data-tauri-drag-region>
              <span style={{ width: `${Math.round(level * 100)}%` }} />
            </span>
          </>
        ) : null}
        {isIndeterminate(phase) ? <ProgressBar tone={phaseTone(phase)} /> : null}

        {interim && !showChips ? (
          // Live words while speaking; the newest words stay visible.
          <span className="interim" data-tauri-drag-region title={interim}>
            <bdi>{interim}</bdi>
          </span>
        ) : null}

        {showChips ? (
          <div className="chips" data-tauri-drag-region>
            {targets.length === 0 ? (
              <span className="hint" data-tauri-drag-region>
                Speak into any window
              </span>
            ) : (
              targets.map((t) => (
                <TargetChip
                  key={t.id}
                  target={t}
                  selected={t.id === snapshot.selected_target_id}
                  hasSuggestion={snapshot.suggestions.some((s) => s.target_id === t.id)}
                  onSelect={(target) => void run(() => api.selectTarget(target.id))}
                  onActivate={(target) => void run(() => api.activateTarget(target.id))}
                  onContextMenu={(target) => setMenuId((id) => (id === target.id ? null : target.id))}
                  onRemove={(target) => void run(() => api.unbindTarget(target.id))}
                />
              ))
            )}
          </div>
        ) : interim ? null : (
          <span className="spacer" data-tauri-drag-region />
        )}

        {!showChips && routed && !interim ? (
          <span className={`route${routed.status === "offline" ? " offline" : ""}`} data-tauri-drag-region>
            → {slotGlyph(routed.slot)} {targetLabel(routed)}
          </span>
        ) : null}

        <span className="bar-actions">
          {busy ? (
            <button type="button" title="Cancel (Esc)" onMouseDown={(e) => e.preventDefault()} onClick={() => void run(api.cancelCurrentOperation)}>
              ✕
            </button>
          ) : (
            <button
              type="button"
              title={pinnedOpen ? "Hide details" : "Show transcription and prompt"}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setPinnedOpen((v) => !v)}
            >
              {expanded ? "▴" : "▾"}
            </button>
          )}
          <button type="button" title="Settings" onMouseDown={(e) => e.preventDefault()} onClick={() => void run(api.openSettings)}>
            ⚙
          </button>
        </span>
      </div>

      {menuOpen && menuTarget ? (
        <div className="preview target-menu">
          <header>
            <span className="variant-title">
              {slotGlyph(menuTarget.slot)} {targetLabel(menuTarget)}
              {menuTarget.status === "offline" ? " — window closed" : ""}
            </span>
            <button type="button" onMouseDown={(e) => e.preventDefault()} onClick={() => setMenuId(null)}>
              Close
            </button>
          </header>
          <div className="row">
            <span className="field-label">Paste</span>
            {OUTPUTS.map((kind) => (
              <button
                key={kind}
                type="button"
                className={menuTarget.preferred_output === kind ? "primary" : ""}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void run(() => api.setTargetOutput(menuTarget.id, kind))}
              >
                {kind}
              </button>
            ))}
          </div>
          <div className="row">
            <button
              type="button"
              className={menuTarget.auto_submit ? "primary" : ""}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => void run(() => api.setTargetAutoSubmit(menuTarget.id, !menuTarget.auto_submit))}
            >
              {menuTarget.auto_submit ? "✓ " : ""}Press Enter after paste
            </button>
          </div>
          <div className="row">
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                setMenuId(null);
                void api.openSettings().then(() => emit("show_targets_tab", menuTarget.id));
              }}
            >
              Rename…
            </button>
            <button
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() =>
                void open({ directory: true, multiple: false, title: "Project directory" }).then((dir) => {
                  if (typeof dir === "string") void run(() => api.setProjectRoot(menuTarget.id, dir));
                })
              }
            >
              {menuTarget.project_root ? `Project: ${menuTarget.project_name ?? menuTarget.project_root}…` : "Set project directory…"}
            </button>
          </div>
          {menuSuggestion ? (
            <div className="row">
              <button
                type="button"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => void run(() => api.confirmRebind(menuTarget.id, menuSuggestion.window.platform_window_id))}
              >
                Rebind to “{menuSuggestion.window.title || menuSuggestion.window.app_name}”
              </button>
            </div>
          ) : null}
          <div className="row">
            <button
              type="button"
              className="danger"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                setMenuId(null);
                void run(() => api.unbindTarget(menuTarget.id));
              }}
            >
              Remove this window
            </button>
          </div>
        </div>
      ) : expanded ? (
        <ResultPreview
          result={result}
          notice={snapshot.notice}
          error={error}
          busy={busy || phase === "INJECTING"}
          defaultKind={defaultKind}
          onCopy={(kind) => void run(() => api.copyResult(kind))}
          onInject={(kind) => {
            if (result) void run(() => api.injectResult(result.id, kind));
          }}
          onRecompile={() => void run(api.recompileLastPrompt)}
          onDismiss={() => {
            setPinnedOpen(false);
            setActionError(null);
            if (phase === "READY" || phase === "ERROR" || phase === "DONE") void run(api.cancelCurrentOperation);
          }}
        />
      ) : null}
    </div>
  );
}
