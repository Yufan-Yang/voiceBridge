use super::{CmdResult, State};
use crate::error::{AppError, ErrorCode};
use crate::platform::WindowInfo;
use crate::target::{TargetSlot, TargetsSnapshot};
use crate::types::OutputKind;

fn new_target_defaults(state: &State<'_>) -> (OutputKind, bool) {
    let s = state.settings.read().unwrap();
    (s.behavior.default_output, s.behavior.auto_submit)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.to_string()))?
}

#[tauri::command]
pub async fn list_desktop_windows(state: State<'_>) -> CmdResult<Vec<WindowInfo>> {
    let windows = state.windows.clone();
    blocking(move || windows.list_windows()).await
}

#[tauri::command]
pub async fn list_targets(state: State<'_>) -> CmdResult<TargetsSnapshot> {
    Ok(state.registry.snapshot())
}

/// Pins the current foreground window to `slot` (1–9).
#[tauri::command]
pub async fn bind_active_window(state: State<'_>, slot: u8) -> CmdResult<TargetSlot> {
    let (output, auto_submit) = new_target_defaults(&state);
    let registry = state.registry.clone();
    let target = blocking(move || registry.bind_active(slot, output, auto_submit)).await?;
    state.pipeline.emit_targets();
    Ok(target)
}

#[tauri::command]
pub async fn bind_window_to_slot(
    state: State<'_>,
    platform_window_id: String,
    slot: u8,
) -> CmdResult<TargetSlot> {
    let (output, auto_submit) = new_target_defaults(&state);
    let registry = state.registry.clone();
    let target =
        blocking(move || registry.bind_window_id(&platform_window_id, slot, output, auto_submit))
            .await?;
    state.pipeline.emit_targets();
    Ok(target)
}

#[tauri::command]
pub async fn unbind_target(state: State<'_>, target_id: String) -> CmdResult<()> {
    state.registry.unbind(&target_id)?;
    state.vocab.forget(&target_id);
    state.pipeline.emit_targets();
    Ok(())
}

#[tauri::command]
pub async fn rename_target(
    state: State<'_>,
    target_id: String,
    alias: String,
) -> CmdResult<TargetSlot> {
    let target = state.registry.rename(&target_id, &alias)?;
    state.pipeline.emit_targets();
    Ok(target)
}

/// Selects a target without activating its window.
#[tauri::command]
pub async fn select_target(state: State<'_>, target_id: String) -> CmdResult<()> {
    state.registry.select(&target_id)?;
    state.pipeline.emit_targets();
    Ok(())
}

#[tauri::command]
pub async fn select_relative_target(state: State<'_>, step: i32) -> CmdResult<()> {
    state.registry.select_relative(step.signum());
    state.pipeline.emit_targets();
    Ok(())
}

/// Selects a target and brings its window to the front.
#[tauri::command]
pub async fn activate_target(state: State<'_>, target_id: String) -> CmdResult<()> {
    state.registry.select(&target_id)?;
    state.pipeline.emit_targets();
    let target = state
        .registry
        .get(&target_id)
        .ok_or_else(|| AppError::new(ErrorCode::TargetNotFound))?;
    let windows = state.windows.clone();
    blocking(move || {
        let window = windows
            .window_info(&target.platform_window_id)?
            .filter(|w| crate::target::matcher::identity_matches(&target, w))
            .ok_or_else(|| {
                AppError::new(ErrorCode::TargetNotFound).with_details("target window closed")
            })?;
        windows.activate(&window)
    })
    .await
}

#[tauri::command]
pub async fn confirm_rebind(
    state: State<'_>,
    target_id: String,
    platform_window_id: String,
) -> CmdResult<TargetSlot> {
    let registry = state.registry.clone();
    let target = blocking(move || registry.confirm_rebind(&target_id, &platform_window_id)).await?;
    state.pipeline.emit_targets();
    Ok(target)
}

#[tauri::command]
pub async fn set_target_output(
    state: State<'_>,
    target_id: String,
    output: OutputKind,
) -> CmdResult<TargetSlot> {
    let target = state.registry.set_output(&target_id, output)?;
    state.pipeline.emit_targets();
    Ok(target)
}

/// Enter after paste is opt-in per target.
#[tauri::command]
pub async fn set_target_auto_submit(
    state: State<'_>,
    target_id: String,
    auto_submit: bool,
) -> CmdResult<TargetSlot> {
    let target = state.registry.set_auto_submit(&target_id, auto_submit)?;
    state.pipeline.emit_targets();
    Ok(target)
}

#[tauri::command]
pub async fn set_target_terms(
    state: State<'_>,
    target_id: String,
    terms: Vec<String>,
) -> CmdResult<TargetSlot> {
    let target = state.registry.set_manual_terms(&target_id, terms)?;
    state.pipeline.emit_targets();
    Ok(target)
}

/// Sets (or clears) the project directory and rescans its vocabulary.
#[tauri::command]
pub async fn set_project_root(
    state: State<'_>,
    target_id: String,
    project_root: Option<String>,
) -> CmdResult<TargetSlot> {
    let target = state.registry.set_project_root(&target_id, project_root)?;
    let vocab = state.vocab.clone();
    let scan_target = target.clone();
    blocking(move || Ok(vocab.refresh(&scan_target))).await?;
    state.pipeline.emit_targets();
    Ok(target)
}

/// Rescans the project vocabulary. Returns all terms for the target.
#[tauri::command]
pub async fn refresh_project_terms(state: State<'_>, target_id: String) -> CmdResult<Vec<String>> {
    let target = state
        .registry
        .get(&target_id)
        .ok_or_else(|| AppError::new(ErrorCode::TargetNotFound))?;
    let vocab = state.vocab.clone();
    blocking(move || {
        vocab.refresh(&target);
        Ok(vocab.terms_for(&target))
    })
    .await
}
