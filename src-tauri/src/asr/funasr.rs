//! Fun-ASR provider. Talks to a persistent Python sidecar
//! (`sidecars/funasr_sidecar.py`) over stdio using JSON lines.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::provider::*;
use crate::audio::temp::TempAudioFile;
use crate::audio::AudioBuffer;
use crate::error::{AppError, ErrorCode, Result};
use crate::sidecar::health::{health_from_state, missing_paths};
use crate::sidecar::protocol::{SidecarRequest, SidecarResponse};
use crate::sidecar::{SidecarManager, SidecarSpec, SidecarState};
use crate::types::{ProviderHealth, ProviderStatus};

const PROVIDER: &str = "fun-asr";
/// Loading the model on first start can take a while.
const START_TIMEOUT: Duration = Duration::from_secs(300);

pub struct FunAsrProvider {
    pub sidecar: Arc<SidecarManager>,
    /// Python interpreter with `funasr` installed.
    pub runtime: String,
    /// Local model directory.
    pub model: String,
    pub script: PathBuf,
    pub temp_dir: PathBuf,
    pub timeout: Duration,
}

impl FunAsrProvider {
    fn check_config(&self) -> Result<()> {
        if let Some(msg) = missing_paths(&self.runtime, &self.model) {
            return Err(AppError::new(ErrorCode::AsrNotConfigured).with_details(msg));
        }
        if !self.script.is_file() {
            return Err(AppError::new(ErrorCode::AsrNotConfigured)
                .with_details("funasr_sidecar.py not found"));
        }
        Ok(())
    }

    fn spec(&self) -> SidecarSpec {
        SidecarSpec {
            program: PathBuf::from(&self.runtime),
            args: vec![
                "-u".to_string(),
                self.script.to_string_lossy().to_string(),
                "--model".to_string(),
                self.model.clone(),
            ],
            // Keep model hubs from reaching the network.
            env: vec![
                ("HF_HUB_OFFLINE".to_string(), "1".to_string()),
                ("TRANSFORMERS_OFFLINE".to_string(), "1".to_string()),
                ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ],
            stdio_protocol: true,
        }
    }

    async fn request(
        &self,
        request: SidecarRequest,
        cancel: &CancellationToken,
        timeout: Duration,
    ) -> Result<SidecarResponse> {
        let line = serde_json::to_string(&request)
            .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.to_string()))?;
        let reply = self
            .sidecar
            .request_line(&line, cancel, timeout, ErrorCode::AsrTimeout)
            .await?;
        let response: SidecarResponse = serde_json::from_str(&reply).map_err(|_| {
            AppError::new(ErrorCode::AsrProcessFailed).with_details("malformed sidecar response")
        })?;
        if response.id != request.id {
            return Err(
                AppError::new(ErrorCode::AsrProcessFailed).with_details("response id mismatch")
            );
        }
        if !response.ok {
            let category: String = response.error.chars().take(80).collect();
            return Err(AppError::new(ErrorCode::AsrProcessFailed).with_details(category));
        }
        Ok(response)
    }

    /// Starts the sidecar if needed (also recovers from a crash) and waits
    /// until the model is loaded.
    async fn ensure_started(&self, cancel: &CancellationToken) -> Result<()> {
        self.check_config()?;
        let _starting = self.sidecar.start_lock.lock().await;
        if self.sidecar.state().await == SidecarState::Running {
            return Ok(());
        }
        self.sidecar.start(self.spec()).await?;
        let health = SidecarRequest {
            id: uuid::Uuid::new_v4().to_string(),
            token: self.sidecar.token().to_string(),
            op: "health".to_string(),
            audio_path: None,
            hotwords: vec![],
            language: None,
        };
        self.request(health, cancel, START_TIMEOUT)
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl AsrProvider for FunAsrProvider {
    async fn health_check(&self) -> Result<ProviderHealth> {
        if let Err(e) = self.check_config() {
            return Ok(ProviderHealth::new(
                PROVIDER,
                ProviderStatus::NotConfigured,
                e.details,
            ));
        }
        Ok(match self.sidecar.state().await {
            SidecarState::Running => {
                ProviderHealth::new(PROVIDER, ProviderStatus::Ready, "Sidecar is running.")
            }
            other => health_from_state(PROVIDER, other),
        })
    }

    async fn start(&self) -> Result<()> {
        self.ensure_started(&CancellationToken::new()).await
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        context: AsrContext,
        cancel: CancellationToken,
    ) -> Result<TranscriptResult> {
        self.ensure_started(&cancel).await?;
        let duration_ms = audio.duration_ms();
        // Fun-ASR reads audio from a file; the guard removes it afterwards.
        let file = TempAudioFile::write(&self.temp_dir, &audio)?;
        let request = SidecarRequest {
            id: context.utterance_id.clone(),
            token: self.sidecar.token().to_string(),
            op: "transcribe".to_string(),
            audio_path: Some(file.path().to_string_lossy().to_string()),
            hotwords: context.hotwords.iter().take(60).cloned().collect(),
            language: context.language.clone(),
        };
        let response = self.request(request, &cancel, self.timeout).await?;
        Ok(TranscriptResult {
            text: response.text.trim().to_string(),
            duration_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioSource;

    fn provider(dir: &std::path::Path, runtime: &str, script: &std::path::Path) -> FunAsrProvider {
        FunAsrProvider {
            sidecar: Arc::new(SidecarManager::new(
                "funasr-test",
                dir.to_path_buf(),
                ErrorCode::AsrProcessFailed,
            )),
            runtime: runtime.to_string(),
            model: dir.to_string_lossy().to_string(),
            script: script.to_path_buf(),
            temp_dir: dir.join("tmp"),
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn not_configured_without_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let p = provider(dir.path(), "", &dir.path().join("missing.py"));
        assert_eq!(
            p.health_check().await.unwrap().status,
            ProviderStatus::NotConfigured
        );
        let err = p
            .transcribe(
                MockAudioSource::tone(300),
                AsrContext::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::AsrNotConfigured);
    }

    /// A shell script that speaks the sidecar protocol stands in for Python.
    #[cfg(unix)]
    #[tokio::test]
    async fn crashed_sidecar_is_unavailable_then_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake_sidecar.sh");
        // `sh -u <script> --model <dir>`: echo a successful reply per request line.
        std::fs::write(
            &script,
            "while read -r line; do id=$(printf '%s' \"$line\" | sed 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/'); \
             printf '{\"id\":\"%s\",\"ok\":true,\"text\":\"hello world\"}\\n' \"$id\"; done\n",
        )
        .unwrap();
        let p = provider(dir.path(), "/bin/sh", &script);
        let ctx = || AsrContext {
            utterance_id: "utt-1".into(),
            ..Default::default()
        };

        let out = p
            .transcribe(MockAudioSource::tone(300), ctx(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.text, "hello world");
        assert_eq!(
            p.health_check().await.unwrap().status,
            ProviderStatus::Ready
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("tmp")).unwrap().count(),
            0
        );

        // Kill the sidecar behind the manager's back: it must show as unavailable.
        let pid = p.sidecar.pid().await.unwrap();
        std::process::Command::new("/bin/kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap();
        let mut status = ProviderStatus::Ready;
        for _ in 0..100 {
            status = p.health_check().await.unwrap().status;
            if status == ProviderStatus::Unavailable {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(status, ProviderStatus::Unavailable);

        // The next request restarts it.
        let out = p
            .transcribe(MockAudioSource::tone(300), ctx(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(out.text, "hello world");
        p.sidecar.stop().await;
    }
}
