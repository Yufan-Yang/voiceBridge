//! Settings schema. Every tunable (model names, paths, limits, shortcuts)
//! lives here so nothing is scattered through the codebase.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::OutputKind;

pub const SLOT_COUNT: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AsrProviderKind {
    #[default]
    Mock,
    FunAsr,
    WhisperCpp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PromptProviderKind {
    #[default]
    Mock,
    LlamaCpp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ModelSettings {
    pub asr_provider: AsrProviderKind,
    /// Fun-ASR: Python interpreter. whisper.cpp: `whisper-cli` binary.
    pub asr_runtime_path: String,
    /// Local model directory (Fun-ASR) or model file (whisper.cpp).
    pub asr_model_path: String,
    /// Informational model identifier, e.g. `FunAudioLLM/Fun-ASR-Nano-2512`.
    pub asr_model_id: String,
    /// Optional language hint passed to the ASR runtime. Empty = auto.
    pub asr_language: String,
    pub asr_timeout_ms: u32,
    pub prompt_provider: PromptProviderKind,
    /// llama.cpp `llama-server` binary.
    pub prompt_runtime_path: String,
    /// GGUF model file.
    pub prompt_model_path: String,
    /// Informational model identifier, e.g. `Qwen/Qwen3-4B-Instruct-2507`.
    pub prompt_model_id: String,
    pub prompt_quantization: String,
    pub context_size: u32,
    pub max_output_tokens: u32,
    pub prompt_timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct AudioSettings {
    /// Input device name. `None` = system default.
    pub input_device: Option<String>,
    pub max_recording_secs: u32,
    pub vad_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ShortcutSettings {
    pub push_to_talk: String,
    pub cancel: String,
    pub select_slot: Vec<String>,
    pub bind_slot: Vec<String>,
    pub prev_target: String,
    pub next_target: String,
    pub inject_last_raw: String,
    pub inject_last_prompt: String,
}

/// How the talk shortcut works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TalkMode {
    /// Record while the shortcut is held.
    #[default]
    Hold,
    /// Press once to start, press again to stop.
    Tap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct BehaviorSettings {
    /// Output used for newly bound targets.
    pub default_output: OutputKind,
    pub auto_inject: bool,
    /// Default `auto_submit` for newly bound targets. Always defaults to false.
    pub auto_submit: bool,
    pub save_history: bool,
    pub completion_notice_ms: u32,
    pub wheel_cycles_targets: bool,
    /// Send each utterance to the window that has focus when Push-to-Talk
    /// is pressed. When off, only the manually selected target is used.
    pub follow_focus: bool,
    pub talk_mode: TalkMode,
    /// Tap mode: stop recording after this many seconds without speech.
    pub silence_stop_secs: u32,
    /// Show the words in the overlay while still speaking.
    pub live_text: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct PrivacySettings {
    /// Always false: this build has no network inference path.
    pub network_inference: bool,
    pub save_audio: bool,
    pub save_transcripts: bool,
    /// Always false: no telemetry is integrated.
    pub telemetry: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct OverlaySettings {
    /// Last overlay position in physical pixels.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Settings {
    pub models: ModelSettings,
    pub audio: AudioSettings,
    pub shortcuts: ShortcutSettings,
    pub behavior: BehaviorSettings,
    pub privacy: PrivacySettings,
    pub overlay: OverlaySettings,
}
