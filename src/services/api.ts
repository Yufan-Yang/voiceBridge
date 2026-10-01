import { invoke } from "@tauri-apps/api/core";
import type {
  AppError,
  AppSnapshot,
  InjectionResult,
  OutputKind,
  PermissionKind,
  PermissionsSnapshot,
  PromptCompileResult,
  ProvidersHealth,
  Settings,
  TargetSlot,
  TargetsSnapshot,
  UtteranceResult,
  WindowInfo,
} from "../types";

/** Typed wrappers around the Tauri commands. Rejections are `AppError`s. */
export const api = {
  getAppState: () => invoke<AppSnapshot>("get_app_state"),
  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (settings: Settings) => invoke<Settings>("update_settings", { settings }),

  listDesktopWindows: () => invoke<WindowInfo[]>("list_desktop_windows"),
  listTargets: () => invoke<TargetsSnapshot>("list_targets"),
  bindActiveWindow: (slot: number) => invoke<TargetSlot>("bind_active_window", { slot }),
  bindWindowToSlot: (platformWindowId: string, slot: number) =>
    invoke<TargetSlot>("bind_window_to_slot", { platformWindowId, slot }),
  unbindTarget: (targetId: string) => invoke<void>("unbind_target", { targetId }),
  renameTarget: (targetId: string, alias: string) => invoke<TargetSlot>("rename_target", { targetId, alias }),
  selectTarget: (targetId: string) => invoke<void>("select_target", { targetId }),
  selectRelativeTarget: (step: number) => invoke<void>("select_relative_target", { step }),
  activateTarget: (targetId: string) => invoke<void>("activate_target", { targetId }),
  confirmRebind: (targetId: string, platformWindowId: string) =>
    invoke<TargetSlot>("confirm_rebind", { targetId, platformWindowId }),
  setTargetOutput: (targetId: string, output: OutputKind) =>
    invoke<TargetSlot>("set_target_output", { targetId, output }),
  setTargetAutoSubmit: (targetId: string, autoSubmit: boolean) =>
    invoke<TargetSlot>("set_target_auto_submit", { targetId, autoSubmit }),
  setTargetTerms: (targetId: string, terms: string[]) => invoke<TargetSlot>("set_target_terms", { targetId, terms }),
  setProjectRoot: (targetId: string, projectRoot: string | null) =>
    invoke<TargetSlot>("set_project_root", { targetId, projectRoot }),
  refreshProjectTerms: (targetId: string) => invoke<string[]>("refresh_project_terms", { targetId }),

  startRecording: () => invoke<string | null>("start_recording"),
  stopRecording: () => invoke<void>("stop_recording"),
  cancelCurrentOperation: () => invoke<boolean>("cancel_current_operation"),
  listAudioDevices: () => invoke<string[]>("list_audio_devices"),
  /** Development only: run the pipeline on a WAV file. */
  transcribeAudio: (path: string) => invoke<void>("transcribe_audio", { path }),

  compilePrompt: (text: string) => invoke<PromptCompileResult>("compile_prompt", { text }),
  recompileLastPrompt: () => invoke<UtteranceResult>("recompile_last_prompt"),

  injectLastRaw: () => invoke<InjectionResult>("inject_last_raw"),
  injectLastNormalized: () => invoke<InjectionResult>("inject_last_normalized"),
  injectLastPrompt: () => invoke<InjectionResult>("inject_last_prompt"),
  injectResult: (utteranceId: string, kind: OutputKind | null) =>
    invoke<InjectionResult>("inject_result", { utteranceId, kind }),
  copyResult: (kind: OutputKind) => invoke<void>("copy_result", { kind }),

  getProviderHealth: () => invoke<ProvidersHealth>("get_provider_health"),
  restartAsrSidecar: () => invoke<ProvidersHealth>("restart_asr_sidecar"),
  restartPromptSidecar: () => invoke<ProvidersHealth>("restart_prompt_sidecar"),

  /** File names of the runtimes and models shipped inside this build. */
  getBundledAssets: () => invoke<string[]>("get_bundled_assets"),

  getHistory: () => invoke<UtteranceResult[]>("get_history"),
  clearHistory: () => invoke<void>("clear_history"),
  clearTempFiles: () => invoke<number>("clear_temp_files"),
  getPermissions: () => invoke<PermissionsSnapshot>("get_permissions"),
  requestPermission: (kind: PermissionKind) => invoke<PermissionsSnapshot>("request_permission", { kind }),
  openSettings: () => invoke<void>("open_settings"),
};

/** Commands reject with an `AppError`; anything else is wrapped. */
export function toAppError(e: unknown): AppError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    return e as AppError;
  }
  return {
    code: "INTERNAL",
    message: "An unexpected error occurred.",
    details: String(e),
    retryable: true,
    recovery: "Try again.",
  };
}
