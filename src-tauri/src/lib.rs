//! VoiceBridge: local push-to-talk voice input for coding windows.

pub mod app_state;
pub mod asr;
pub mod audio;
pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod history;
pub mod injection;
pub mod logging;
mod overlay;
pub mod pipeline;
pub mod platform;
pub mod prompt;
pub mod providers;
pub mod shortcuts;
pub mod sidecar;
pub mod state_machine;
pub mod target;
pub mod types;
pub mod vocab;

use tauri::Manager;

pub fn run() {
    use commands::{audio::*, injection::*, models::*, settings::*, targets::*};

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| app_state::setup(app.handle()))
        .on_window_event(overlay::on_window_event)
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            get_settings,
            update_settings,
            list_desktop_windows,
            bind_active_window,
            bind_window_to_slot,
            unbind_target,
            rename_target,
            select_target,
            select_relative_target,
            activate_target,
            confirm_rebind,
            list_targets,
            set_target_output,
            set_target_auto_submit,
            set_target_terms,
            start_recording,
            stop_recording,
            cancel_current_operation,
            list_audio_devices,
            transcribe_audio,
            compile_prompt,
            recompile_last_prompt,
            inject_last_raw,
            inject_last_normalized,
            inject_last_prompt,
            inject_result,
            copy_result,
            get_provider_health,
            restart_asr_sidecar,
            restart_prompt_sidecar,
            get_bundled_assets,
            set_project_root,
            refresh_project_terms,
            get_history,
            clear_history,
            clear_temp_files,
            get_permissions,
            request_permission,
            open_settings,
        ])
        .build(tauri::generate_context!())
        .expect("failed to start VoiceBridge");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<app_state::AppState>() {
                state.shutdown();
            }
        }
    });
}
