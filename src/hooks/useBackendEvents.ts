import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { api, toAppError } from "../services/api";
import { useAppStore } from "../stores/appStore";
import type {
  AppError,
  AppSnapshot,
  InjectionOutcome,
  ProvidersHealth,
  TargetsSnapshot,
  UtteranceResult,
} from "../types";

/** Subscribes to backend events and loads the initial state. */
export function useBackendEvents(): void {
  useEffect(() => {
    const store = useAppStore.getState();
    let disposed = false;
    const unlisten: UnlistenFn[] = [];
    const on = <T,>(name: string, handler: (payload: T) => void) => {
      void listen<T>(name, (event) => handler(event.payload)).then((off) => {
        if (disposed) off();
        else unlisten.push(off);
      });
    };

    on<AppSnapshot>("state_changed", store.setSnapshot);
    on<number>("recording_level", store.setLevel);
    on<TargetsSnapshot>("targets_changed", store.setTargets);
    on<ProvidersHealth>("provider_health_changed", store.setHealth);
    on<UtteranceResult>("utterance_updated", store.applyUtterance);
    on<InjectionOutcome>("injection_result", store.setInjection);
    on<AppError>("fatal_error", store.setActionError);

    api
      .getAppState()
      .then(store.setSnapshot)
      .catch((e) => store.setActionError(toAppError(e)));

    return () => {
      disposed = true;
      unlisten.forEach((off) => off());
    };
  }, []);
}

/** Runs a command and surfaces a rejection in the UI instead of throwing. */
export async function run<T>(action: () => Promise<T>): Promise<T | undefined> {
  try {
    const value = await action();
    return value;
  } catch (e) {
    useAppStore.getState().setActionError(toAppError(e));
    return undefined;
  }
}
