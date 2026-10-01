import type { OutputKind, Phase, TargetSlot, UtteranceResult } from "../types";

export type Tone = "idle" | "recording" | "transcribing" | "compiling" | "success" | "warning" | "error";

const LABELS: Record<Phase, string> = {
  IDLE: "Idle",
  LISTENING: "Listening",
  FINALIZING_AUDIO: "Finalizing audio",
  TRANSCRIBING: "Transcribing",
  NORMALIZING: "Normalizing",
  COMPILING_PROMPT: "Compiling prompt",
  READY: "Ready",
  INJECTING: "Injecting",
  DONE: "Done",
  ERROR: "Error",
  CANCELED: "Canceled",
};

const TONES: Record<Phase, Tone> = {
  IDLE: "idle",
  LISTENING: "recording",
  FINALIZING_AUDIO: "transcribing",
  TRANSCRIBING: "transcribing",
  NORMALIZING: "transcribing",
  COMPILING_PROMPT: "compiling",
  READY: "success",
  INJECTING: "compiling",
  DONE: "success",
  ERROR: "error",
  CANCELED: "idle",
};

const GLYPHS: Record<Phase, string> = {
  IDLE: "●",
  LISTENING: "●",
  FINALIZING_AUDIO: "◌",
  TRANSCRIBING: "◌",
  NORMALIZING: "◌",
  COMPILING_PROMPT: "◌",
  READY: "✓",
  INJECTING: "◌",
  DONE: "✓",
  ERROR: "✕",
  CANCELED: "–",
};

export const phaseLabel = (phase: Phase): string => LABELS[phase];
export const phaseTone = (phase: Phase): Tone => TONES[phase];
export const phaseGlyph = (phase: Phase): string => GLYPHS[phase];

/** Phases with no measurable completion: shown as an indeterminate bar. */
export function isIndeterminate(phase: Phase): boolean {
  return ["FINALIZING_AUDIO", "TRANSCRIBING", "NORMALIZING", "COMPILING_PROMPT", "INJECTING"].includes(phase);
}

export function isBusy(phase: Phase): boolean {
  return phase === "LISTENING" || isIndeterminate(phase);
}

const CIRCLED = ["①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨"];

export function slotGlyph(slot: number): string {
  return CIRCLED[slot - 1] ?? String(slot);
}

/** `Claude/backend` when a project is set, otherwise just the alias. */
export function targetLabel(target: Pick<TargetSlot, "alias" | "project_name">): string {
  return target.project_name ? `${target.alias}/${target.project_name}` : target.alias;
}

/** mm:ss */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

/** Real recording progress (elapsed / maximum), clamped to 0..1. */
export function recordingProgress(elapsedMs: number, maxSecs: number): number {
  if (maxSecs <= 0) return 0;
  return Math.min(1, Math.max(0, elapsedMs / (maxSecs * 1000)));
}

export function charCount(text: string): number {
  return Array.from(text).length;
}

export function resultSummary(result: UtteranceResult): string {
  return `Raw ${charCount(result.raw_transcript)} chars · Prompt ${charCount(result.compiled_prompt)} chars`;
}

export const OUTPUT_LABELS: Record<OutputKind, string> = {
  raw: "Raw transcription",
  normalized: "Normalized transcription",
  prompt: "Compiled prompt",
};

export function resultText(result: UtteranceResult, kind: OutputKind): string {
  switch (kind) {
    case "raw":
      return result.raw_transcript;
    case "normalized":
      return result.normalized_transcript;
    case "prompt":
      return result.compiled_prompt;
  }
}
