import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TargetSlot, UtteranceResult } from "../types";
import { ProgressBar } from "./ProgressBar";
import { ResultPreview } from "./ResultPreview";
import { StatusIndicator } from "./StatusIndicator";
import { TargetChip } from "./TargetChip";

const target: TargetSlot = {
  id: "a",
  slot: 2,
  alias: "Codex",
  platform_window_id: "2",
  process_id: 2,
  executable_or_bundle_id: "/app",
  title_hint: "web",
  project_root: "/work/web",
  project_name: "web",
  adapter: "generic_clipboard",
  preferred_output: "prompt",
  auto_submit: false,
  status: "online",
  manual_terms: [],
};

const result: UtteranceResult = {
  id: "u1",
  created_at: "",
  frozen_target_id: "a",
  raw_transcript: "raw words",
  normalized_transcript: "Raw words.",
  compiled_prompt: "Do the raw words.",
  uncertain_identifiers: [],
  needs_confirmation: false,
  status: "compiled",
};

const noop = () => {};

describe("ProgressBar", () => {
  it("never shows a percentage for indeterminate work", () => {
    const html = renderToStaticMarkup(<ProgressBar tone="transcribing" />);
    expect(html).toContain("indeterminate");
    expect(html).not.toContain("aria-valuenow");
    expect(html).not.toContain("%");
  });

  it("shows the real value while recording", () => {
    const html = renderToStaticMarkup(<ProgressBar tone="recording" value={0.25} />);
    expect(html).toContain('aria-valuenow="25"');
    expect(html).not.toContain("indeterminate");
  });
});

describe("StatusIndicator", () => {
  it("renders label, tone and detail", () => {
    const html = renderToStaticMarkup(<StatusIndicator phase="LISTENING" detail="00:06" />);
    expect(html).toContain("Listening");
    expect(html).toContain("tone-recording");
    expect(html).toContain("00:06");
  });
});

describe("TargetChip", () => {
  it("marks the selected, offline and receiving states", () => {
    expect(renderToStaticMarkup(<TargetChip target={target} selected />)).toContain("chip selected");
    const offline = renderToStaticMarkup(<TargetChip target={{ ...target, status: "offline" }} selected={false} />);
    expect(offline).toContain("offline");
    const receiving = renderToStaticMarkup(<TargetChip target={target} selected receiving />);
    expect(receiving).toContain("→");
    expect(receiving).toContain("②");
    expect(receiving).toContain("Codex/web");
  });
});

describe("ResultPreview", () => {
  const props = { busy: false, defaultKind: "prompt" as const, onCopy: noop, onInject: noop, onRecompile: noop, onDismiss: noop };

  it("shows all three text variants with copy and inject actions", () => {
    const html = renderToStaticMarkup(<ResultPreview {...props} result={result} notice={null} error={null} />);
    for (const text of ["raw words", "Raw words.", "Do the raw words."]) expect(html).toContain(text);
    expect(html.match(/>Copy</g)).toHaveLength(3);
    expect(html.match(/>Inject</g)).toHaveLength(3);
  });

  it("shows the fallback notice and the error with its recovery action", () => {
    const html = renderToStaticMarkup(
      <ResultPreview
        {...props}
        result={result}
        notice="Prompt compilation failed. The transcription has been preserved."
        error={{
          code: "TARGET_NOT_FOUND",
          message: "The target window is not available.",
          details: "internal detail",
          retryable: true,
          recovery: "Rebind it.",
        }}
      />,
    );
    expect(html).toContain("Prompt compilation failed. The transcription has been preserved.");
    expect(html).toContain("The target window is not available.");
    expect(html).toContain("Rebind it.");
    expect(html).not.toContain("internal detail");
  });
});
