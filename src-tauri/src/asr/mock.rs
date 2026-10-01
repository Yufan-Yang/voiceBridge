//! Mock ASR so the whole pipeline runs without any model installed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::provider::*;
use crate::audio::AudioBuffer;
use crate::error::{AppError, Result};
use crate::types::{ProviderHealth, ProviderStatus};

const SAMPLES: &[&str] = &[
    "change that use user query thing so it takes user id, don't make the request when there is no id, add two tests, and don't change the return structure",
    "um find where the login handler validates the token and add logging for the failure case, keep the existing error messages",
    "rename the config loader function to load settings and update every caller, run the tests after",
];

pub struct MockAsr {
    scripted: Mutex<VecDeque<String>>,
    pub calls: AtomicU32,
    pub delay: Duration,
}

impl Default for MockAsr {
    fn default() -> Self {
        Self {
            scripted: Mutex::new(VecDeque::new()),
            calls: AtomicU32::new(0),
            delay: Duration::from_millis(600),
        }
    }
}

impl MockAsr {
    /// Instant mock returning `texts` in order, then the built-in samples.
    pub fn scripted(texts: &[&str]) -> Self {
        Self {
            scripted: Mutex::new(texts.iter().map(|t| t.to_string()).collect()),
            calls: AtomicU32::new(0),
            delay: Duration::ZERO,
        }
    }
}

#[async_trait]
impl AsrProvider for MockAsr {
    async fn health_check(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth::new(
            "mock",
            ProviderStatus::Ready,
            "Mock ASR: returns sample transcripts, no model required.",
        ))
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        _context: AsrContext,
        cancel: CancellationToken,
    ) -> Result<TranscriptResult> {
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            _ = tokio::time::sleep(self.delay) => {}
        }
        let n = self.calls.fetch_add(1, Ordering::SeqCst) as usize;
        let text = self
            .scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| SAMPLES[n % SAMPLES.len()].to_string());
        Ok(TranscriptResult {
            text,
            duration_ms: audio.duration_ms(),
        })
    }
}
