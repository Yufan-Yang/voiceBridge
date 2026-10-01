//! whisper.cpp provider. Runs `whisper-cli` once per utterance over stdio;
//! nothing listens on any network interface.

use std::path::PathBuf;
use std::process::Stdio;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::provider::*;
use crate::audio::temp::TempAudioFile;
use crate::audio::AudioBuffer;
use crate::error::{AppError, ErrorCode, Result};
use crate::logging::sanitize_line;
use crate::sidecar::health::missing_paths;
use crate::types::{ProviderHealth, ProviderStatus};

pub struct WhisperCppProvider {
    pub runtime: String,
    pub model: String,
    pub temp_dir: PathBuf,
}

/// Arguments for `whisper-cli`. Kept separate so it can be tested.
pub fn whisper_args(model: &str, audio_path: &str, context: &AsrContext) -> Vec<String> {
    let mut args = vec![
        "-m".to_string(),
        model.to_string(),
        "-f".to_string(),
        audio_path.to_string(),
        "--no-timestamps".to_string(),
        "--no-prints".to_string(),
        "-l".to_string(),
        context
            .language
            .clone()
            .unwrap_or_else(|| "auto".to_string()),
    ];
    if !context.hotwords.is_empty() {
        // whisper.cpp biases decoding with an initial prompt.
        let prompt: String = context
            .hotwords
            .iter()
            .take(60)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        args.push("--prompt".to_string());
        args.push(prompt);
    }
    args
}

#[async_trait]
impl AsrProvider for WhisperCppProvider {
    async fn health_check(&self) -> Result<ProviderHealth> {
        Ok(match missing_paths(&self.runtime, &self.model) {
            Some(msg) => ProviderHealth::new("whisper.cpp", ProviderStatus::NotConfigured, msg),
            None => ProviderHealth::new(
                "whisper.cpp",
                ProviderStatus::Ready,
                "whisper-cli runs once per utterance.",
            ),
        })
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        context: AsrContext,
        cancel: CancellationToken,
    ) -> Result<TranscriptResult> {
        if let Some(msg) = missing_paths(&self.runtime, &self.model) {
            return Err(AppError::new(ErrorCode::AsrNotConfigured).with_details(msg));
        }
        let duration_ms = audio.duration_ms();
        // whisper-cli only accepts files; the guard deletes it on every exit path.
        let file = TempAudioFile::write(&self.temp_dir, &audio)?;
        let args = whisper_args(&self.model, &file.path().to_string_lossy(), &context);
        let child = tokio::process::Command::new(&self.runtime)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AppError::new(ErrorCode::AsrProcessFailed)
                    .with_details(format!("spawn: {}", e.kind()))
            })?;

        // Dropping the future on cancel kills the process (kill_on_drop).
        let output = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            out = child.wait_with_output() => out
                .map_err(|e| AppError::new(ErrorCode::AsrProcessFailed).with_details(e.kind().to_string()))?,
        };
        if !output.status.success() {
            let first_line = String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(sanitize_line)
                .unwrap_or_default();
            return Err(AppError::new(ErrorCode::AsrProcessFailed)
                .with_details(format!("exit {:?}: {first_line}", output.status.code())));
        }
        let text = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(TranscriptResult { text, duration_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioSource;

    #[test]
    fn builds_arguments_with_hotwords() {
        let ctx = AsrContext {
            utterance_id: "u".into(),
            hotwords: vec!["useUserQuery".into(), "userId".into()],
            language: Some("en".into()),
        };
        let args = whisper_args("/m.bin", "/tmp/a.wav", &ctx);
        assert!(args.windows(2).any(|w| w == ["-l", "en"]));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--prompt" && w[1] == "useUserQuery, userId"));
        let plain = whisper_args("/m.bin", "/tmp/a.wav", &AsrContext::default());
        assert!(!plain.contains(&"--prompt".to_string()));
    }

    #[tokio::test]
    async fn unconfigured_runtime_is_reported() {
        let p = WhisperCppProvider {
            runtime: String::new(),
            model: String::new(),
            temp_dir: std::env::temp_dir(),
        };
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

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_cli_runtime_and_cleans_up_the_temp_file() {
        // `/bin/echo` stands in for whisper-cli: it prints its arguments.
        let dir = tempfile::tempdir().unwrap();
        let p = WhisperCppProvider {
            runtime: "/bin/echo".into(),
            model: "/bin/sh".into(),
            temp_dir: dir.path().to_path_buf(),
        };
        let out = p
            .transcribe(
                MockAudioSource::tone(300),
                AsrContext::default(),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.text.contains("--no-timestamps"));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "temp audio must be deleted"
        );
    }
}
