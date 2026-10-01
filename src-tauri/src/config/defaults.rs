//! Secure defaults and sanitization.

use super::model::*;
use crate::types::OutputKind;

pub const DEFAULT_ASR_MODEL_ID: &str = "FunAudioLLM/Fun-ASR-Nano-2512";
pub const DEFAULT_WHISPER_MODEL_ID: &str = "large-v3-turbo";
pub const DEFAULT_PROMPT_MODEL_ID: &str = "Qwen/Qwen3-4B-Instruct-2507";
pub const DEFAULT_CONTEXT_SIZE: u32 = 8192;
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 512;
pub const DEFAULT_MAX_RECORDING_SECS: u32 = 60;

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            asr_provider: AsrProviderKind::Mock,
            asr_runtime_path: String::new(),
            asr_model_path: String::new(),
            asr_model_id: DEFAULT_ASR_MODEL_ID.to_string(),
            asr_language: String::new(),
            asr_timeout_ms: 60_000,
            prompt_provider: PromptProviderKind::Mock,
            prompt_runtime_path: String::new(),
            prompt_model_path: String::new(),
            prompt_model_id: DEFAULT_PROMPT_MODEL_ID.to_string(),
            prompt_quantization: "Q4_K_M".to_string(),
            context_size: DEFAULT_CONTEXT_SIZE,
            max_output_tokens: DEFAULT_MAX_OUTPUT_TOKENS,
            prompt_timeout_ms: 45_000,
        }
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            max_recording_secs: DEFAULT_MAX_RECORDING_SECS,
            vad_enabled: true,
        }
    }
}

fn default_select_slots() -> Vec<String> {
    (1..=SLOT_COUNT).map(|n| format!("Ctrl+Alt+{n}")).collect()
}

fn default_bind_slots() -> Vec<String> {
    (1..=SLOT_COUNT)
        .map(|n| format!("Ctrl+Alt+Shift+{n}"))
        .collect()
}

impl Default for ShortcutSettings {
    fn default() -> Self {
        // Ctrl+Alt (Control+Option) combinations are not used by common editors.
        Self {
            push_to_talk: "Ctrl+Alt+Space".to_string(),
            cancel: "Ctrl+Alt+Escape".to_string(),
            select_slot: default_select_slots(),
            bind_slot: default_bind_slots(),
            prev_target: "Ctrl+Alt+BracketLeft".to_string(),
            next_target: "Ctrl+Alt+BracketRight".to_string(),
            inject_last_raw: "Ctrl+Alt+R".to_string(),
            inject_last_prompt: "Ctrl+Alt+P".to_string(),
        }
    }
}

impl Default for BehaviorSettings {
    fn default() -> Self {
        Self {
            default_output: OutputKind::Prompt,
            auto_inject: true,
            auto_submit: false,
            save_history: false,
            completion_notice_ms: 4000,
            wheel_cycles_targets: true,
        }
    }
}

fn clamp_or(value: u32, min: u32, max: u32, default: u32) -> u32 {
    if (min..=max).contains(&value) {
        value
    } else {
        default
    }
}

impl Settings {
    /// Parse settings; anything unreadable falls back to secure defaults.
    pub fn from_json_or_default(json: &str) -> Self {
        serde_json::from_str::<Settings>(json)
            .unwrap_or_default()
            .sanitized()
    }

    /// Enforce invariants and replace out-of-range values with defaults.
    pub fn sanitized(mut self) -> Self {
        // Hard invariants: this build has no network inference or telemetry.
        self.privacy.network_inference = false;
        self.privacy.telemetry = false;

        let m = ModelSettings::default();
        self.models.context_size = clamp_or(self.models.context_size, 512, 131_072, m.context_size);
        self.models.max_output_tokens =
            clamp_or(self.models.max_output_tokens, 16, 8192, m.max_output_tokens);
        self.models.asr_timeout_ms =
            clamp_or(self.models.asr_timeout_ms, 1000, 600_000, m.asr_timeout_ms);
        self.models.prompt_timeout_ms = clamp_or(
            self.models.prompt_timeout_ms,
            1000,
            600_000,
            m.prompt_timeout_ms,
        );

        self.audio.max_recording_secs = clamp_or(
            self.audio.max_recording_secs,
            1,
            600,
            DEFAULT_MAX_RECORDING_SECS,
        );
        if matches!(self.audio.input_device.as_deref(), Some("")) {
            self.audio.input_device = None;
        }

        self.behavior.completion_notice_ms =
            clamp_or(self.behavior.completion_notice_ms, 500, 60_000, 4000);

        let d = ShortcutSettings::default();
        if self.shortcuts.select_slot.len() != SLOT_COUNT {
            self.shortcuts.select_slot = d.select_slot;
        }
        if self.shortcuts.bind_slot.len() != SLOT_COUNT {
            self.shortcuts.bind_slot = d.bind_slot;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_defaults() {
        let s = Settings::default();
        assert!(!s.privacy.network_inference);
        assert!(!s.privacy.save_audio);
        assert!(!s.privacy.save_transcripts);
        assert!(!s.privacy.telemetry);
        assert!(!s.behavior.save_history);
        assert!(!s.behavior.auto_submit);
        assert_eq!(s.models.context_size, 8192);
        assert_eq!(s.models.max_output_tokens, 512);
        assert_eq!(s.models.asr_provider, AsrProviderKind::Mock);
    }

    #[test]
    fn invalid_configuration_falls_back_to_secure_defaults() {
        // Garbage, wrong types, and truncated JSON all yield defaults.
        for bad in [
            "",
            "not json",
            "{\"privacy\": 42}",
            "{\"behavior\": {\"auto_submit\": \"yes\"}}",
            "{\"models\":",
        ] {
            assert_eq!(
                Settings::from_json_or_default(bad),
                Settings::default(),
                "{bad}"
            );
        }
    }

    #[test]
    fn sanitize_enforces_invariants_and_ranges() {
        let json = r#"{
            "privacy": {"network_inference": true, "telemetry": true},
            "models": {"context_size": 0, "max_output_tokens": 999999},
            "audio": {"max_recording_secs": 0},
            "shortcuts": {"select_slot": ["A"]}
        }"#;
        let s = Settings::from_json_or_default(json);
        assert!(!s.privacy.network_inference);
        assert!(!s.privacy.telemetry);
        assert_eq!(s.models.context_size, 8192);
        assert_eq!(s.models.max_output_tokens, 512);
        assert_eq!(s.audio.max_recording_secs, 60);
        assert_eq!(s.shortcuts.select_slot.len(), SLOT_COUNT);
        // Missing sections are filled from defaults.
        assert!(!s.behavior.auto_submit);
    }

    #[test]
    fn default_shortcuts_avoid_plain_editor_keys() {
        let s = ShortcutSettings::default();
        let mut all = vec![
            s.push_to_talk,
            s.cancel,
            s.prev_target,
            s.next_target,
            s.inject_last_raw,
            s.inject_last_prompt,
        ];
        all.extend(s.select_slot);
        all.extend(s.bind_slot);
        for sc in &all {
            assert!(sc.starts_with("Ctrl+Alt+"), "{sc}");
        }
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), all.len());
    }
}
