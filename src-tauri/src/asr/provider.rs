use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::audio::AudioBuffer;
use crate::error::Result;
use crate::types::ProviderHealth;

/// Per-utterance context for recognition.
#[derive(Debug, Clone, Default)]
pub struct AsrContext {
    pub utterance_id: String,
    /// Project vocabulary of the frozen target, used as hotwords.
    pub hotwords: Vec<String>,
    /// Optional language hint. `None` lets the runtime decide.
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptResult {
    pub text: String,
    pub duration_ms: u32,
}

#[async_trait]
pub trait AsrProvider: Send + Sync {
    async fn health_check(&self) -> Result<ProviderHealth>;

    /// Starts the underlying runtime if it has one. Used by "Test model".
    async fn start(&self) -> Result<()> {
        Ok(())
    }

    /// True when a transcription is fast enough to run repeatedly while the
    /// user is still speaking (live interim text).
    fn supports_interim(&self) -> bool {
        false
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        context: AsrContext,
        cancel: CancellationToken,
    ) -> Result<TranscriptResult>;
}
