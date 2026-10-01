import { describe, expect, it } from "vitest";
import type { Phase, UtteranceResult } from "../types";
import {
  charCount,
  formatElapsed,
  formatSeconds,
  timingDetail,
  isBusy,
  isIndeterminate,
  phaseLabel,
  phaseTone,
  recordingProgress,
  resultSummary,
  resultText,
  slotGlyph,
  targetLabel,
} from "./format";

const ALL_PHASES: Phase[] = [
  "IDLE",
  "LISTENING",
  "FINALIZING_AUDIO",
  "TRANSCRIBING",
  "NORMALIZING",
  "COMPILING_PROMPT",
  "READY",
  "INJECTING",
  "DONE",
  "ERROR",
  "CANCELED",
];

const result: UtteranceResult = {
  id: "u1",
  created_at: "2026-01-01T00:00:00Z",
  frozen_target_id: "t1",
  raw_transcript: "change the thing",
  normalized_transcript: "Change the thing.",
  compiled_prompt: "Task:\n\n- Change the thing",
  uncertain_identifiers: [],
  needs_confirmation: false,
  status: "compiled",
  audio_ms: 6900,
  transcribe_ms: 900,
  compile_ms: 2300,
};

describe("phase presentation", () => {
  it("has a label and tone for every state", () => {
    for (const phase of ALL_PHASES) {
      expect(phaseLabel(phase)).not.toBe("");
      expect(phaseTone(phase)).toBeTruthy();
    }
  });

  it("uses the suggested status colors", () => {
    expect(phaseTone("IDLE")).toBe("idle");
    expect(phaseTone("LISTENING")).toBe("recording");
    expect(phaseTone("TRANSCRIBING")).toBe("transcribing");
    expect(phaseTone("COMPILING_PROMPT")).toBe("compiling");
    expect(phaseTone("READY")).toBe("success");
    expect(phaseTone("ERROR")).toBe("error");
  });

  it("treats model work as indeterminate and recording as measurable", () => {
    expect(isIndeterminate("TRANSCRIBING")).toBe(true);
    expect(isIndeterminate("COMPILING_PROMPT")).toBe(true);
    expect(isIndeterminate("LISTENING")).toBe(false);
    expect(isIndeterminate("READY")).toBe(false);
    expect(isBusy("LISTENING")).toBe(true);
    expect(isBusy("IDLE")).toBe(false);
  });
});

describe("formatting", () => {
  it("formats elapsed recording time", () => {
    expect(formatElapsed(0)).toBe("00:00");
    expect(formatElapsed(6_400)).toBe("00:06");
    expect(formatElapsed(125_000)).toBe("02:05");
    expect(formatElapsed(-5)).toBe("00:00");
  });

  it("computes real recording progress, clamped", () => {
    expect(recordingProgress(30_000, 60)).toBe(0.5);
    expect(recordingProgress(90_000, 60)).toBe(1);
    expect(recordingProgress(1000, 0)).toBe(0);
  });

  it("labels targets and slots", () => {
    expect(slotGlyph(2)).toBe("②");
    expect(slotGlyph(9)).toBe("⑨");
    expect(slotGlyph(12)).toBe("12");
    expect(targetLabel({ alias: "Codex", project_name: "web" })).toBe("Codex/web");
    expect(targetLabel({ alias: "Codex", project_name: null })).toBe("Codex");
  });

  it("summarizes a result by character count", () => {
    expect(charCount("héllo")).toBe(5);
    expect(charCount("修复🙂")).toBe(3);
    expect(resultSummary(result)).toBe("6.9 s spoken · 3.2 s to process");
    expect(timingDetail(result)).toBe("Audio 6.9 s · recognition 0.9 s · prompt 2.3 s · total 3.2 s");
    expect(timingDetail({ ...result, compile_ms: 0 })).toBe(
      "Audio 6.9 s · recognition 0.9 s · prompt skipped · total 0.9 s",
    );
    expect(formatSeconds(75_400)).toBe("75 s");
  });

  it("returns each text variant independently", () => {
    expect(resultText(result, "raw")).toBe(result.raw_transcript);
    expect(resultText(result, "normalized")).toBe(result.normalized_transcript);
    expect(resultText(result, "prompt")).toBe(result.compiled_prompt);
  });
});
