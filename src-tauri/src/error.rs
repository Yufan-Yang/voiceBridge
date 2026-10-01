//! Unified application error with stable error codes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(export)]
pub enum ErrorCode {
    MicPermissionDenied,
    MicDeviceNotFound,
    AudioCaptureFailed,
    NoSpeechDetected,
    AsrNotConfigured,
    AsrProcessFailed,
    AsrTimeout,
    PromptModelNotConfigured,
    PromptInvalidJson,
    PromptTimeout,
    PromptProcessFailed,
    TargetNotFound,
    TargetIdentityMismatch,
    TargetActivationFailed,
    ForegroundVerificationFailed,
    ClipboardFailed,
    InputInjectionFailed,
    AccessibilityPermissionDenied,
    OperationCanceled,
    Busy,
    InvalidInput,
    ConfigFailed,
    Unsupported,
    Internal,
}

impl ErrorCode {
    /// (user-facing message, retryable, recommended recovery action)
    fn defaults(self) -> (&'static str, bool, &'static str) {
        use ErrorCode::*;
        match self {
            MicPermissionDenied => (
                "Microphone access is denied.",
                true,
                "Allow microphone access in System Settings › Privacy & Security › Microphone, then try again.",
            ),
            MicDeviceNotFound => (
                "The microphone is not available.",
                true,
                "Reconnect the microphone or pick another input device in Settings › Audio.",
            ),
            AudioCaptureFailed => (
                "Audio capture failed.",
                true,
                "Try again. If it keeps failing, select another input device.",
            ),
            NoSpeechDetected => (
                "No speech was detected.",
                true,
                "Hold the Push-to-Talk shortcut while speaking.",
            ),
            AsrNotConfigured => (
                "The speech recognition runtime is not configured.",
                false,
                "Select the ASR runtime and model in Settings › Models, or switch to the Mock provider.",
            ),
            AsrProcessFailed => (
                "Speech recognition failed.",
                true,
                "Restart the ASR sidecar from Settings › Models and try again.",
            ),
            AsrTimeout => (
                "Speech recognition timed out.",
                true,
                "Try a shorter utterance or restart the ASR sidecar.",
            ),
            PromptModelNotConfigured => (
                "The prompt model is not configured.",
                false,
                "Select the llama.cpp runtime and GGUF model in Settings › Models, or switch to the Mock provider.",
            ),
            PromptInvalidJson => (
                "The prompt model returned an invalid response.",
                true,
                "Recompile the prompt, or inject the transcription instead.",
            ),
            PromptTimeout => (
                "Prompt compilation timed out.",
                true,
                "Recompile the prompt, or inject the transcription instead.",
            ),
            PromptProcessFailed => (
                "The prompt model process failed.",
                true,
                "Restart the prompt sidecar from Settings › Models.",
            ),
            TargetNotFound => (
                "The target window is not available.",
                true,
                "Re-open the window and rebind it, or select another target.",
            ),
            TargetIdentityMismatch => (
                "The target window no longer matches the pinned window.",
                false,
                "Rebind the target to the correct window.",
            ),
            TargetActivationFailed => (
                "The target window could not be brought to the front.",
                true,
                "Bring the window to the front manually and retry the injection.",
            ),
            ForegroundVerificationFailed => (
                "The target window did not become the foreground window, so nothing was pasted.",
                true,
                "Retry the injection without switching windows.",
            ),
            ClipboardFailed => (
                "The clipboard could not be accessed.",
                true,
                "Retry the injection.",
            ),
            InputInjectionFailed => (
                "The paste keystroke could not be simulated.",
                true,
                "Retry, or copy the text and paste it manually.",
            ),
            AccessibilityPermissionDenied => (
                "Accessibility permission is required to activate windows and paste text.",
                true,
                "Enable VoiceBridge in System Settings › Privacy & Security › Accessibility.",
            ),
            OperationCanceled => ("The operation was canceled.", true, "Start again when ready."),
            Busy => (
                "Another operation is in progress.",
                true,
                "Wait for the current operation to finish.",
            ),
            InvalidInput => ("The request is not valid.", false, "Check the input and try again."),
            ConfigFailed => (
                "Settings could not be read or written.",
                true,
                "Check that the application data directory is writable.",
            ),
            Unsupported => (
                "This feature is not supported on this operating system yet.",
                false,
                "Use a supported platform (see the README).",
            ),
            Internal => ("An unexpected internal error occurred.", true, "Try again."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppError {
    /// Stable machine-readable code.
    pub code: ErrorCode,
    /// User-facing message.
    pub message: String,
    /// Internal diagnostic details. Never contains transcripts, prompts or audio.
    pub details: String,
    pub retryable: bool,
    /// Recommended recovery action.
    pub recovery: String,
}

impl AppError {
    pub fn new(code: ErrorCode) -> Self {
        let (message, retryable, recovery) = code.defaults();
        Self {
            code,
            message: message.to_string(),
            details: String::new(),
            retryable,
            recovery: recovery.to_string(),
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = details.into();
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn canceled() -> Self {
        Self::new(ErrorCode::OperationCanceled)
    }

    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorCode::Unsupported).with_details(what)
    }

    pub fn is_canceled(&self) -> bool {
        self.code == ErrorCode::OperationCanceled
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
