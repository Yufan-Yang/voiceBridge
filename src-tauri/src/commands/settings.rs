use tauri::Manager;

use super::{CmdResult, State};
use crate::config::{self, Settings};
use crate::overlay::SETTINGS_LABEL;
use crate::types::{PermissionKind, PermissionsSnapshot, UtteranceResult};

#[tauri::command]
pub async fn get_settings(state: State<'_>) -> CmdResult<Settings> {
    Ok(state.settings.read().unwrap().clone())
}

/// Validates, stores and applies new settings. Returns what was applied,
/// which may differ from the input after sanitization.
#[tauri::command]
pub async fn update_settings(state: State<'_>, settings: Settings) -> CmdResult<Settings> {
    let mut next = settings.sanitized();
    let (models_changed, shortcuts_changed, history_enabled, history_disabled) = {
        let current = state.settings.read().unwrap();
        // The overlay position is owned by the backend.
        next.overlay = current.overlay.clone();
        (
            current.models != next.models,
            current.shortcuts != next.shortcuts,
            !current.behavior.save_history && next.behavior.save_history,
            current.behavior.save_history && !next.behavior.save_history,
        )
    };
    config::save_settings(state.store.as_ref(), &next)?;
    *state.settings.write().unwrap() = next.clone();

    if models_changed {
        state.hub.stop_all().await;
        state.pipeline.set_providers(state.hub.build(&next.models));
        state.warm_up_providers();
    }
    if shortcuts_changed {
        state.apply_shortcuts();
    }
    if history_enabled {
        for item in state.history.list() {
            state.history.upsert(&item, true);
        }
    }
    if history_disabled {
        // Turning persistence off removes what was written to disk.
        let _ = state.store.remove(config::HISTORY_KEY);
    }
    state.pipeline.emit_state();
    Ok(next)
}

#[tauri::command]
pub async fn get_history(state: State<'_>) -> CmdResult<Vec<UtteranceResult>> {
    Ok(state.history.list())
}

/// Clears history in memory and on disk, plus opt-in transcript and audio files.
#[tauri::command]
pub async fn clear_history(state: State<'_>) -> CmdResult<()> {
    state.history.clear();
    state.pipeline.clear_last_result();
    let _ = std::fs::remove_file(state.data_dir.join("transcripts.jsonl"));
    let _ = std::fs::remove_dir_all(state.data_dir.join("recordings"));
    Ok(())
}

/// Deletes temporary audio files. Returns how many were removed.
#[tauri::command]
pub async fn clear_temp_files(state: State<'_>) -> CmdResult<u32> {
    Ok(crate::audio::temp::cleanup_stale(state.hub.temp_dir()) as u32)
}

#[tauri::command]
pub async fn get_permissions(state: State<'_>) -> CmdResult<PermissionsSnapshot> {
    Ok(PermissionsSnapshot {
        microphone: state.permissions.status(PermissionKind::Microphone),
        accessibility: state.permissions.status(PermissionKind::Accessibility),
    })
}

/// Shows the OS prompt where one exists and opens the relevant settings pane.
#[tauri::command]
pub async fn request_permission(
    state: State<'_>,
    kind: PermissionKind,
) -> CmdResult<PermissionsSnapshot> {
    state.permissions.request(kind);
    let _ = state.permissions.open_settings(kind);
    get_permissions(state).await
}

#[tauri::command]
pub async fn open_settings(app: tauri::AppHandle) -> CmdResult<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}
