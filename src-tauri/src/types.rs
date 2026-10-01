//! Shared data structures. These are the single source of truth: the
//! TypeScript definitions in `src/types/generated` are generated from them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::AppError;
use crate::target::model::{RebindSuggestion, TargetSlot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OutputKind {
    Raw,
    Normalized,
    #[default]
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum Phase {
    #[default]
    Idle,
    Listening,
    FinalizingAudio,
    Transcribing,
    Normalizing,
    CompilingPrompt,
    Ready,
    Injecting,
    Done,
    Error,
    Canceled,
}

impl Phase {
    /// Processing phases that a cancel request can interrupt.
    pub fn is_processing(self) -> bool {
        matches!(
            self,
            Phase::Listening
                | Phase::FinalizingAudio
                | Phase::Transcribing
                | Phase::Normalizing
                | Phase::CompilingPrompt
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UtteranceStatus {
    Transcribed,
    Compiled,
    PromptFailed,
    Injected,
    InjectionFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UtteranceResult {
    pub id: String,
    pub created_at: String,
    /// Target selected when Push-to-Talk was pressed. Empty when none was selected.
    pub frozen_target_id: String,
    /// Direct ASR output. No later model may overwrite it.
    pub raw_transcript: String,
    /// Transcript after punctuation, spacing, and project identifier correction.
    pub normalized_transcript: String,
    /// Final prompt intended for a coding agent.
    pub compiled_prompt: String,
    pub uncertain_identifiers: Vec<String>,
    pub needs_confirmation: bool,
    pub status: UtteranceStatus,
    /// Length of the recording in milliseconds.
    #[serde(default)]
    pub audio_ms: u32,
    /// Time spent in speech recognition.
    #[serde(default)]
    pub transcribe_ms: u32,
    /// Time spent in the prompt model; 0 when that step was skipped.
    #[serde(default)]
    pub compile_ms: u32,
}

impl UtteranceResult {
    pub fn text(&self, kind: OutputKind) -> &str {
        match kind {
            OutputKind::Raw => &self.raw_transcript,
            OutputKind::Normalized => &self.normalized_transcript,
            OutputKind::Prompt => &self.compiled_prompt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderStatus {
    Ready,
    Starting,
    Unavailable,
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderHealth {
    pub provider: String,
    pub status: ProviderStatus,
    /// Sanitized diagnostic message.
    pub message: String,
}

impl ProviderHealth {
    pub fn new(provider: &str, status: ProviderStatus, message: impl Into<String>) -> Self {
        Self {
            provider: provider.to_string(),
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProvidersHealth {
    pub asr: ProviderHealth,
    pub prompt: ProviderHealth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InjectionResult {
    pub target_id: String,
    pub chars: u32,
    pub submitted: bool,
    pub clipboard_restored: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InjectionOutcome {
    pub utterance_id: String,
    pub ok: bool,
    pub result: Option<InjectionResult>,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionKind {
    Microphone,
    Accessibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermissionStatus {
    Granted,
    Denied,
    #[default]
    NotDetermined,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionRequired {
    pub kind: PermissionKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionsSnapshot {
    pub microphone: PermissionStatus,
    pub accessibility: PermissionStatus,
}

/// Words recognized so far while the user is still speaking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InterimTranscript {
    pub utterance_id: String,
    pub text: String,
}

/// Complete UI state. Emitted with every `state_changed` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppSnapshot {
    pub phase: Phase,
    pub utterance_id: Option<String>,
    pub frozen_target_id: Option<String>,
    pub selected_target_id: Option<String>,
    pub targets: Vec<TargetSlot>,
    pub suggestions: Vec<RebindSuggestion>,
    pub last_result: Option<UtteranceResult>,
    pub error: Option<AppError>,
    pub notice: Option<String>,
    /// Unix epoch milliseconds when recording started.
    pub recording_started_ms: Option<f64>,
    pub max_recording_secs: u32,
    /// True when no inference leaves this machine (always true in this build).
    pub offline: bool,
    pub dev_mode: bool,
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}
