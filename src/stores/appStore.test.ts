import { beforeEach, describe, expect, it } from "vitest";
import type { AppSnapshot, TargetSlot, UtteranceResult } from "../types";
import { EMPTY_SNAPSHOT, useAppStore } from "./appStore";

const target = (id: string, slot: number): TargetSlot => ({
  id,
  slot,
  alias: `T${slot}`,
  platform_window_id: String(slot),
  process_id: slot,
  executable_or_bundle_id: "/app",
  title_hint: "",
  project_root: null,
  project_name: null,
  adapter: "generic_clipboard",
  preferred_output: "prompt",
  auto_submit: false,
  status: "online",
  manual_terms: [],
});

const utterance = (id: string, raw: string): UtteranceResult => ({
  id,
  created_at: "",
  frozen_target_id: "a",
  raw_transcript: raw,
  normalized_transcript: raw,
  compiled_prompt: raw,
  uncertain_identifiers: [],
  needs_confirmation: false,
  status: "compiled",
  audio_ms: 6900,
  transcribe_ms: 900,
  compile_ms: 2300,
});

const snapshot = (over: Partial<AppSnapshot>): AppSnapshot => ({ ...EMPTY_SNAPSHOT, ...over });

describe("appStore", () => {
  beforeEach(() => {
    useAppStore.setState({ snapshot: EMPTY_SNAPSHOT, level: 0, interim: "", health: null, lastInjection: null, actionError: null });
  });

  it("starts with secure, idle defaults", () => {
    const { snapshot: s } = useAppStore.getState();
    expect(s.phase).toBe("IDLE");
    expect(s.offline).toBe(true);
    expect(s.targets).toEqual([]);
  });

  it("clamps the input level and resets it when recording stops", () => {
    const store = useAppStore.getState();
    store.setSnapshot(snapshot({ phase: "LISTENING" }));
    store.setLevel(4);
    expect(useAppStore.getState().level).toBe(1);
    store.setLevel(-1);
    expect(useAppStore.getState().level).toBe(0);
    store.setLevel(0.5);
    store.setSnapshot(snapshot({ phase: "TRANSCRIBING" }));
    expect(useAppStore.getState().level).toBe(0);
  });

  it("changing the selection does not touch the frozen target", () => {
    const store = useAppStore.getState();
    store.setSnapshot(
      snapshot({
        phase: "TRANSCRIBING",
        utterance_id: "u1",
        frozen_target_id: "a",
        selected_target_id: "a",
        targets: [target("a", 1), target("b", 2)],
      }),
    );
    store.setTargets({ targets: [target("a", 1), target("b", 2)], selected_id: "b", suggestions: [] });
    const s = useAppStore.getState().snapshot;
    expect(s.selected_target_id).toBe("b");
    expect(s.frozen_target_id).toBe("a");
    expect(s.phase).toBe("TRANSCRIBING");
  });

  it("ignores an utterance update for an older utterance", () => {
    const store = useAppStore.getState();
    store.setSnapshot(snapshot({ phase: "READY", utterance_id: "new", last_result: utterance("new", "new text") }));
    store.applyUtterance(utterance("old", "stale text"));
    expect(useAppStore.getState().snapshot.last_result?.raw_transcript).toBe("new text");
    store.applyUtterance({ ...utterance("new", "new text"), status: "injected" });
    expect(useAppStore.getState().snapshot.last_result?.status).toBe("injected");
  });

  it("shows interim text only for the recording in progress", () => {
    const store = useAppStore.getState();
    store.setSnapshot(snapshot({ phase: "LISTENING", utterance_id: "u1" }));
    store.setInterim({ utterance_id: "old", text: "stale words" });
    expect(useAppStore.getState().interim).toBe("");
    store.setInterim({ utterance_id: "u1", text: "fix the login" });
    expect(useAppStore.getState().interim).toBe("fix the login");
    // Kept while the final pass runs, cleared when the result is ready.
    store.setSnapshot(snapshot({ phase: "TRANSCRIBING", utterance_id: "u1" }));
    expect(useAppStore.getState().interim).toBe("fix the login");
    store.setSnapshot(snapshot({ phase: "READY", utterance_id: "u1" }));
    expect(useAppStore.getState().interim).toBe("");
    // A new recording never starts with the previous one's words.
    store.setSnapshot(snapshot({ phase: "LISTENING", utterance_id: "u1" }));
    store.setInterim({ utterance_id: "u1", text: "abc" });
    store.setSnapshot(snapshot({ phase: "LISTENING", utterance_id: "u2" }));
    expect(useAppStore.getState().interim).toBe("");
  });

  it("clears a previous action error when a new recording starts", () => {
    const store = useAppStore.getState();
    store.setActionError({ code: "BUSY", message: "busy", details: "", retryable: true, recovery: "" });
    store.setSnapshot(snapshot({ phase: "READY" }));
    expect(useAppStore.getState().actionError?.code).toBe("BUSY");
    store.setSnapshot(snapshot({ phase: "LISTENING" }));
    expect(useAppStore.getState().actionError).toBeNull();
  });
});
