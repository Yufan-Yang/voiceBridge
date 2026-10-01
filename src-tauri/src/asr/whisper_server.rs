//! whisper.cpp provider backed by a persistent `whisper-server` sidecar.
//!
//! The model stays loaded (and its GPU shaders compiled) for as long as the
//! app runs, so an utterance costs only the inference itself. The server
//! binds 127.0.0.1 on a random port. whisper-server has no API-key option,
//! so the session token is used as a secret request-path prefix: requests
//! that do not know it get a 404.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::provider::*;
use crate::audio::AudioBuffer;
use crate::error::{AppError, ErrorCode, Result};
use crate::sidecar::health::{health_from_state, missing_paths};
use crate::sidecar::manager::free_loopback_port;
use crate::sidecar::{SidecarManager, SidecarSpec, SidecarState};
use crate::types::{ProviderHealth, ProviderStatus};

const PROVIDER: &str = "whisper.cpp";
const HOST: &str = "127.0.0.1";
const START_TIMEOUT: Duration = Duration::from_secs(120);

pub struct WhisperServerProvider {
    pub sidecar: Arc<SidecarManager>,
    pub runtime: String,
    pub model: String,
    pub timeout: Duration,
    client: reqwest::Client,
    port: AtomicU16,
}

/// True when the configured runtime is the server flavour of whisper.cpp.
pub fn is_server_runtime(runtime: &str) -> bool {
    std::path::Path::new(runtime)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("server"))
}

/// Encodes mono audio as 16-bit PCM WAV in memory (no temporary file).
pub fn wav_bytes(audio: &AudioBuffer) -> Result<Vec<u8>> {
    let fail =
        |e: hound::Error| AppError::new(ErrorCode::AsrProcessFailed).with_details(e.to_string());
    let mut bytes = Vec::with_capacity(audio.samples.len() * 2 + 44);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: audio.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::new(Cursor::new(&mut bytes), spec).map_err(fail)?;
    for s in &audio.samples {
        writer
            .write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .map_err(fail)?;
    }
    writer.finalize().map_err(fail)?;
    Ok(bytes)
}

/// Encoder context for a clip of `duration_ms`.
///
/// Whisper normally encodes a fixed 30 s window (1500 frames, 50 per second)
/// no matter how short the clip is, which is most of the recognition time.
/// Limiting the window to the clip length makes short utterances several
/// times faster. Measured on large-v3-turbo: only multiples of 256 decode
/// reliably (other sizes produced garbage), so the size is rounded up to one,
/// with a margin of about 1.3 s. Returns 0 (the full window) for long clips.
pub fn audio_ctx_for(duration_ms: u32) -> u32 {
    const FRAMES_PER_SEC: u32 = 50;
    const MARGIN: u32 = 64;
    const STEP: u32 = 256;
    let needed = duration_ms * FRAMES_PER_SEC / 1000 + MARGIN;
    let rounded = needed.div_ceil(STEP) * STEP;
    if rounded > 1280 {
        0
    } else {
        rounded
    }
}

impl WhisperServerProvider {
    pub fn new(
        sidecar: Arc<SidecarManager>,
        runtime: String,
        model: String,
        timeout: Duration,
    ) -> Self {
        Self {
            sidecar,
            runtime,
            model,
            timeout,
            // Loopback only: never route through a system proxy.
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_default(),
            port: AtomicU16::new(0),
        }
    }

    fn check_config(&self) -> Result<()> {
        match missing_paths(&self.runtime, &self.model) {
            Some(msg) => Err(AppError::new(ErrorCode::AsrNotConfigured).with_details(msg)),
            None => Ok(()),
        }
    }

    /// Command line for `whisper-server`. Always binds the loopback interface.
    pub fn server_args(model: &str, port: u16, token: &str) -> Vec<String> {
        vec![
            "-m".to_string(),
            model.to_string(),
            "--host".to_string(),
            HOST.to_string(),
            "--port".to_string(),
            port.to_string(),
            "--request-path".to_string(),
            format!("/{token}"),
            "-l".to_string(),
            "auto".to_string(),
        ]
    }

    async fn is_listening(&self) -> bool {
        let port = self.port.load(Ordering::SeqCst);
        port != 0 && tokio::net::TcpStream::connect((HOST, port)).await.is_ok()
    }

    /// Starts the server if needed (also the crash-recovery path) and waits
    /// until it accepts connections, which it only does once the model is loaded.
    async fn ensure_started(&self, cancel: &CancellationToken) -> Result<()> {
        self.check_config()?;
        let _starting = self.sidecar.start_lock.lock().await;
        if self.sidecar.state().await != SidecarState::Running {
            let port = free_loopback_port()?;
            self.port.store(port, Ordering::SeqCst);
            self.sidecar
                .start(SidecarSpec {
                    program: PathBuf::from(&self.runtime),
                    args: Self::server_args(&self.model, port, self.sidecar.token()),
                    env: vec![],
                    stdio_protocol: false,
                })
                .await?;
        }
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if cancel.is_cancelled() {
                return Err(AppError::canceled());
            }
            if self.is_listening().await {
                return Ok(());
            }
            if self.sidecar.state().await != SidecarState::Running {
                return Err(AppError::new(ErrorCode::AsrProcessFailed)
                    .with_details("whisper-server exited during startup; see sidecar-asr.log"));
            }
            if Instant::now() >= deadline {
                return Err(
                    AppError::new(ErrorCode::AsrTimeout).with_details("model load timed out")
                );
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(AppError::canceled()),
                _ = tokio::time::sleep(Duration::from_millis(150)) => {}
            }
        }
    }

    async fn infer(
        &self,
        audio: &AudioBuffer,
        context: &AsrContext,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let failed = |d: &str| AppError::new(ErrorCode::AsrProcessFailed).with_details(d);
        let file = reqwest::multipart::Part::bytes(wav_bytes(audio)?)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|_| failed("invalid mime type"))?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("response_format", "json")
            .text("temperature", "0.0")
            .text(
                "language",
                context
                    .language
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
        let audio_ctx = audio_ctx_for(audio.duration_ms());
        if audio_ctx > 0 {
            form = form.text("audio_ctx", audio_ctx.to_string());
        }
        if !context.hotwords.is_empty() {
            // whisper.cpp biases decoding with an initial prompt.
            let prompt = context
                .hotwords
                .iter()
                .take(60)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            form = form.text("prompt", prompt);
        }
        let url = format!(
            "http://{HOST}:{}/{}/inference",
            self.port.load(Ordering::SeqCst),
            self.sidecar.token()
        );
        let request = self
            .client
            .post(url)
            .multipart(form)
            .timeout(self.timeout)
            .send();
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            r = request => r.map_err(|e| {
                if e.is_timeout() { AppError::new(ErrorCode::AsrTimeout) } else { failed("request failed") }
            })?,
        };
        if !response.status().is_success() {
            return Err(failed(&format!(
                "http status {}",
                response.status().as_u16()
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| failed("response was not JSON"))?;
        let text = body["text"]
            .as_str()
            .ok_or_else(|| failed("response had no text"))?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

#[async_trait]
impl AsrProvider for WhisperServerProvider {
    async fn health_check(&self) -> Result<ProviderHealth> {
        if let Err(e) = self.check_config() {
            return Ok(ProviderHealth::new(
                PROVIDER,
                ProviderStatus::NotConfigured,
                e.details,
            ));
        }
        let state = self.sidecar.state().await;
        if state == SidecarState::Running && self.is_listening().await {
            return Ok(ProviderHealth::new(
                PROVIDER,
                ProviderStatus::Ready,
                "Model loaded.",
            ));
        }
        Ok(health_from_state(PROVIDER, state))
    }

    /// Loads the model and runs one tiny transcription so the GPU shaders
    /// are compiled before the user's first utterance.
    async fn start(&self) -> Result<()> {
        let cancel = CancellationToken::new();
        self.ensure_started(&cancel).await?;
        let warm_up = AudioBuffer {
            samples: vec![0.0; crate::audio::ASR_SAMPLE_RATE as usize],
            sample_rate: crate::audio::ASR_SAMPLE_RATE,
        };
        self.infer(&warm_up, &AsrContext::default(), &cancel)
            .await
            .map(|_| ())
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        context: AsrContext,
        cancel: CancellationToken,
    ) -> Result<TranscriptResult> {
        self.ensure_started(&cancel).await?;
        let duration_ms = audio.duration_ms();
        let text = self.infer(&audio, &context, &cancel).await?;
        Ok(TranscriptResult { text, duration_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioSource;

    #[test]
    fn detects_server_runtime_and_binds_loopback_behind_a_secret_path() {
        assert!(is_server_runtime("/x/whisper/whisper-server"));
        assert!(!is_server_runtime("/x/whisper/whisper-cli"));
        let args = WhisperServerProvider::server_args("/m.bin", 5000, "secret");
        assert!(args.windows(2).any(|w| w == ["--host", "127.0.0.1"]));
        assert!(args.windows(2).any(|w| w == ["--request-path", "/secret"]));
        assert!(!args.iter().any(|a| a == "0.0.0.0"));
    }

    #[test]
    fn audio_context_scales_with_clip_length_in_safe_steps() {
        assert_eq!(audio_ctx_for(500), 256);
        assert_eq!(audio_ctx_for(2_500), 256);
        assert_eq!(audio_ctx_for(6_900), 512);
        assert_eq!(audio_ctx_for(9_600), 768);
        assert_eq!(audio_ctx_for(20_000), 1280);
        assert_eq!(audio_ctx_for(25_000), 0, "long clips use the full window");
        assert_eq!(audio_ctx_for(60_000), 0);
        for ms in (0..30_000).step_by(137) {
            let ctx = audio_ctx_for(ms);
            assert!(ctx.is_multiple_of(256));
            // The window always covers the whole clip.
            assert!(ctx == 0 || ctx * 1000 / 50 >= ms);
        }
    }

    #[test]
    fn encodes_wav_in_memory() {
        let audio = MockAudioSource::tone(100);
        let bytes = wav_bytes(&audio).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + audio.samples.len() * 2);
    }

    #[tokio::test]
    async fn unconfigured_and_crashing_runtimes_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = || {
            Arc::new(SidecarManager::new(
                "asr-test",
                dir.path().to_path_buf(),
                ErrorCode::AsrProcessFailed,
            ))
        };
        let p = WhisperServerProvider::new(
            sidecar(),
            String::new(),
            String::new(),
            Duration::from_secs(5),
        );
        assert_eq!(
            p.health_check().await.unwrap().status,
            ProviderStatus::NotConfigured
        );

        #[cfg(unix)]
        {
            let p = WhisperServerProvider::new(
                sidecar(),
                "/usr/bin/false".into(),
                "/bin/sh".into(),
                Duration::from_secs(5),
            );
            let err = p
                .transcribe(
                    MockAudioSource::tone(300),
                    AsrContext::default(),
                    CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::AsrProcessFailed);
            assert_eq!(
                p.health_check().await.unwrap().status,
                ProviderStatus::Unavailable
            );
        }
    }
}
