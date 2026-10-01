use tokio_util::sync::CancellationToken;

use super::{CmdResult, State};
use crate::events::AppEvent;
use crate::pipeline::Pipeline;
use crate::prompt::{PromptCompileInput, PromptCompileResult};
use crate::providers;
use crate::types::{ProvidersHealth, UtteranceResult};

async fn publish_health(pipeline: &Pipeline, emit: impl Fn(AppEvent)) -> ProvidersHealth {
    let health = providers::health(&pipeline.providers()).await;
    emit(AppEvent::ProviderHealthChanged(health.clone()));
    health
}

fn emitter(app: tauri::AppHandle) -> impl Fn(AppEvent) {
    use tauri::Emitter;
    move |event| {
        if let AppEvent::ProviderHealthChanged(h) = &event {
            let _ = app.emit(event.name(), h);
        }
    }
}

#[tauri::command]
pub async fn get_provider_health(
    state: State<'_>,
    app: tauri::AppHandle,
) -> CmdResult<ProvidersHealth> {
    Ok(publish_health(&state.pipeline, emitter(app)).await)
}

/// Stops the ASR sidecar and starts it again (loading the model).
#[tauri::command]
pub async fn restart_asr_sidecar(
    state: State<'_>,
    app: tauri::AppHandle,
) -> CmdResult<ProvidersHealth> {
    state.hub.asr_sidecar.stop().await;
    let started = state.pipeline.providers().asr.start().await;
    let health = publish_health(&state.pipeline, emitter(app)).await;
    started.map(|_| health)
}

/// Stops the prompt sidecar and starts it again (loading the model).
#[tauri::command]
pub async fn restart_prompt_sidecar(
    state: State<'_>,
    app: tauri::AppHandle,
) -> CmdResult<ProvidersHealth> {
    state.hub.prompt_sidecar.stop().await;
    let started = state.pipeline.providers().compiler.start().await;
    let health = publish_health(&state.pipeline, emitter(app)).await;
    started.map(|_| health)
}

/// Compiles arbitrary text with the configured Prompt Compiler without
/// touching utterance state. Used by the "Test model" button.
#[tauri::command]
pub async fn compile_prompt(state: State<'_>, text: String) -> CmdResult<PromptCompileResult> {
    let compiler = state.pipeline.providers().compiler;
    let input = PromptCompileInput {
        raw_transcript: text,
        ..Default::default()
    };
    compiler.compile(input, CancellationToken::new()).await
}

#[tauri::command]
pub async fn recompile_last_prompt(state: State<'_>) -> CmdResult<UtteranceResult> {
    state.pipeline.recompile_last().await
}

/// Which runtimes and models ship inside this build ("all-in-one"). An empty
/// path in the settings uses the bundled file.
#[tauri::command]
pub async fn get_bundled_assets(state: State<'_>) -> CmdResult<Vec<String>> {
    let b = state.hub.bundled();
    let name = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    };
    Ok([
        &b.llama_server,
        &b.prompt_model,
        &b.whisper_cli,
        &b.whisper_model,
    ]
    .into_iter()
    .filter_map(name)
    .collect())
}
