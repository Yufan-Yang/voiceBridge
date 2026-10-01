use std::path::Path;

use super::{CmdResult, State};
use crate::error::{AppError, ErrorCode};
use crate::types::AppSnapshot;

#[tauri::command]
pub async fn get_app_state(state: State<'_>) -> CmdResult<AppSnapshot> {
    Ok(state.pipeline.snapshot())
}

#[tauri::command]
pub async fn start_recording(state: State<'_>) -> CmdResult<Option<String>> {
    state.pipeline.ptt_press()
}

#[tauri::command]
pub async fn stop_recording(state: State<'_>) -> CmdResult<()> {
    if let Some(job) = state.pipeline.begin_release() {
        let pipeline = state.pipeline.clone();
        tokio::spawn(async move { pipeline.process(job).await });
    }
    Ok(())
}

/// Cancels the utterance in progress, or dismisses a finished one.
#[tauri::command]
pub async fn cancel_current_operation(state: State<'_>) -> CmdResult<bool> {
    Ok(state.pipeline.cancel())
}

#[tauri::command]
pub async fn list_audio_devices(state: State<'_>) -> CmdResult<Vec<String>> {
    let audio = state.audio.clone();
    tokio::task::spawn_blocking(move || audio.list_devices())
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.to_string()))
}

/// Development only: runs the whole pipeline on a local WAV file instead of
/// the microphone. Rejected in release builds.
#[tauri::command]
pub async fn transcribe_audio(state: State<'_>, path: String) -> CmdResult<()> {
    if !cfg!(debug_assertions) {
        return Err(AppError::new(ErrorCode::Unsupported)
            .with_details("WAV simulation is a development-only feature"));
    }
    let audio = crate::audio::load_wav(Path::new(&path))?;
    let pipeline = state.pipeline.clone();
    tokio::spawn(async move {
        let _ = pipeline.simulate_utterance(audio).await;
    });
    Ok(())
}
