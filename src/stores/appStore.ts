import { create } from "zustand";
import type {
  AppError,
  AppSnapshot,
  InjectionOutcome,
  ProvidersHealth,
  TargetsSnapshot,
  UtteranceResult,
} from "../types";

export const EMPTY_SNAPSHOT: AppSnapshot = {
  phase: "IDLE",
  utterance_id: null,
  frozen_target_id: null,
  selected_target_id: null,
  targets: [],
  suggestions: [],
  last_result: null,
  error: null,
  notice: null,
  recording_started_ms: null,
  max_recording_secs: 60,
  offline: true,
  dev_mode: false,
};

export interface AppStore {
  snapshot: AppSnapshot;
  /** Microphone input level, 0..1. */
  level: number;
  health: ProvidersHealth | null;
  lastInjection: InjectionOutcome | null;
  /** Error from a user action (command rejection), shown until dismissed. */
  actionError: AppError | null;

  setSnapshot: (snapshot: AppSnapshot) => void;
  setLevel: (level: number) => void;
  setTargets: (targets: TargetsSnapshot) => void;
  setHealth: (health: ProvidersHealth) => void;
  applyUtterance: (result: UtteranceResult) => void;
  setInjection: (outcome: InjectionOutcome) => void;
  setActionError: (error: AppError | null) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  snapshot: EMPTY_SNAPSHOT,
  level: 0,
  health: null,
  lastInjection: null,
  actionError: null,

  setSnapshot: (snapshot) =>
    set((state) => ({
      snapshot,
      level: snapshot.phase === "LISTENING" ? state.level : 0,
      // A new utterance clears the previous action error.
      actionError: snapshot.phase === "LISTENING" ? null : state.actionError,
    })),
  setLevel: (level) => set({ level: Math.min(1, Math.max(0, level)) }),
  setTargets: (targets) =>
    set((state) => ({
      snapshot: {
        ...state.snapshot,
        targets: targets.targets,
        selected_target_id: targets.selected_id,
        suggestions: targets.suggestions,
      },
    })),
  setHealth: (health) => set({ health }),
  applyUtterance: (result) =>
    set((state) => {
      const current = state.snapshot.last_result;
      // Only the utterance already on screen (or the active one) may update it.
      const accepted = !current || current.id === result.id || state.snapshot.utterance_id === result.id;
      return accepted ? { snapshot: { ...state.snapshot, last_result: result } } : {};
    }),
  setInjection: (lastInjection) => set({ lastInjection }),
  setActionError: (actionError) => set({ actionError }),
}));
