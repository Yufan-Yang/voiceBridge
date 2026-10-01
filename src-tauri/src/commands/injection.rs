use super::{CmdResult, State};
use crate::error::{AppError, ErrorCode};
use crate::types::{InjectionResult, OutputKind};

#[tauri::command]
pub async fn inject_last_raw(state: State<'_>) -> CmdResult<InjectionResult> {
    state.pipeline.inject(None, Some(OutputKind::Raw)).await
}

#[tauri::command]
pub async fn inject_last_normalized(state: State<'_>) -> CmdResult<InjectionResult> {
    state
        .pipeline
        .inject(None, Some(OutputKind::Normalized))
        .await
}

#[tauri::command]
pub async fn inject_last_prompt(state: State<'_>) -> CmdResult<InjectionResult> {
    state.pipeline.inject(None, Some(OutputKind::Prompt)).await
}

/// Injects a specific utterance. `kind = None` uses the target's preferred output.
#[tauri::command]
pub async fn inject_result(
    state: State<'_>,
    utterance_id: String,
    kind: Option<OutputKind>,
) -> CmdResult<InjectionResult> {
    state.pipeline.inject(Some(utterance_id), kind).await
}

/// Copies one text variant of the last utterance to the clipboard.
#[tauri::command]
pub async fn copy_result(state: State<'_>, kind: OutputKind) -> CmdResult<()> {
    let result = state.pipeline.last_result().ok_or_else(|| {
        AppError::new(ErrorCode::InvalidInput).with_message("There is nothing to copy yet.")
    })?;
    state.injector.copy_text(result.text(kind)).await
}
