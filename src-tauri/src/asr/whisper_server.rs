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

/// Reduced encoder context for a clip of `duration_ms`, used only for the
/// live preview while the user is still speaking.
///
/// Whisper normally encodes a fixed 30 s window (1500 frames, 50 per second)
/// no matter how short the clip is. A smaller window is roughly twice as
/// fast, but it is fragile: on large-v3-turbo only multiples of 256 decode at
/// all, and a window that fits the clip too tightly makes the model repeat
/// whole sentences (measured: 40% character error rate on Chinese with a
/// 1.3 s margin, against 7% for the full window). The final transcript
/// therefore always uses the full window; the preview uses a generous 8 s
/// margin. Returns 0 (the full window) for long clips.
pub fn audio_ctx_for(duration_ms: u32) -> u32 {
    const FRAMES_PER_SEC: u32 = 50;
    const MARGIN: u32 = 400;
    const STEP: u32 = 256;
    let needed = duration_ms * FRAMES_PER_SEC / 1000 + MARGIN;
    let rounded = needed.div_ceil(STEP) * STEP;
    if rounded > 1280 {
        0
    } else {
        rounded
    }
}

/// Common Chinese software terms that Whisper otherwise confuses with
/// homophones (重试/重视, 分支/分之, 重构/中构, 判空/判控). Given as part of the
/// initial prompt when the dictation language is Chinese; it also steers the
/// output towards simplified characters.
pub const ZH_DEV_GLOSSARY: &str = "以下是普通话的句子，内容与软件开发有关。常用词：登录、接口、重试、分支、构建、重构、判空、函数、组件、变量、单元测试、数据库迁移、配置文件、端口、缓存、部署。";

/// Initial prompt for Whisper: the language glossary plus project hotwords.
pub fn initial_prompt(context: &AsrContext) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if context
        .language
        .as_deref()
        .is_some_and(|l| l.to_ascii_lowercase().starts_with("zh"))
    {
        parts.push(ZH_DEV_GLOSSARY.to_string());
    }
    if !context.hotwords.is_empty() {
        parts.push(
            context
                .hotwords
                .iter()
                .take(60)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Whisper sometimes emits the same sentence two or more times in a row.
/// When the whole transcript is one unit repeated, keep a single copy.
pub fn collapse_repeats(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    let n = chars.len();
    // Only whole-sentence repeats: the unit must be reasonably long.
    for unit in 6..=n / 2 {
        let head: String = chars[..unit].iter().collect();
        let head = head.trim();
        if head.is_empty() {
            continue;
        }
        let mut rest: &str = text.trim();
        let mut copies = 0;
        while let Some(stripped) = rest.trim_start().strip_prefix(head) {
            rest = stripped;
            copies += 1;
        }
        if copies >= 2 && rest.trim().is_empty() {
            return head.to_string();
        }
    }
    text.trim().to_string()
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
        // The accurate full window for the final transcript; the smaller,
        // faster one only for the live preview.
        let audio_ctx = if context.interim {
            audio_ctx_for(audio.duration_ms())
        } else {
            0
        };
        if audio_ctx > 0 {
            form = form.text("audio_ctx", audio_ctx.to_string());
        }
        // whisper.cpp biases decoding with an initial prompt.
        if let Some(prompt) = initial_prompt(context) {
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
        Ok(collapse_repeats(
            &text.split_whitespace().collect::<Vec<_>>().join(" "),
        ))
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

    fn supports_interim(&self) -> bool {
        true
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
    fn preview_window_keeps_a_generous_margin() {
        assert_eq!(audio_ctx_for(500), 512);
        assert_eq!(audio_ctx_for(5_900), 768);
        assert_eq!(audio_ctx_for(9_600), 1024);
        assert_eq!(audio_ctx_for(17_000), 1280);
        assert_eq!(audio_ctx_for(18_000), 0, "long clips use the full window");
        for ms in (0..30_000).step_by(137) {
            let ctx = audio_ctx_for(ms);
            assert!(ctx.is_multiple_of(256));
            // At least 8 s beyond the end of the clip.
            assert!(ctx == 0 || ctx * 1000 / 50 >= ms + 8_000);
        }
    }

    #[test]
    fn chinese_gets_a_glossary_prompt_and_hotwords_are_appended() {
        let mut ctx = AsrContext {
            language: Some("zh".into()),
            ..Default::default()
        };
        assert_eq!(initial_prompt(&ctx).as_deref(), Some(ZH_DEV_GLOSSARY));
        ctx.hotwords = vec!["useUserQuery".into(), "userId".into()];
        let p = initial_prompt(&ctx).unwrap();
        assert!(p.starts_with(ZH_DEV_GLOSSARY) && p.ends_with("useUserQuery, userId"));
        ctx.language = Some("en".into());
        assert_eq!(
            initial_prompt(&ctx).as_deref(),
            Some("useUserQuery, userId")
        );
        ctx.hotwords.clear();
        assert_eq!(initial_prompt(&ctx), None);
    }

    #[test]
    fn repeated_sentences_are_collapsed() {
        let s = "先跑一下单元测试,如果有失败的就把报错信息贴出来。";
        assert_eq!(collapse_repeats(&format!("{s} {s}")), s);
        assert_eq!(collapse_repeats(&format!("{s}{s}{s}")), s);
        assert_eq!(
            collapse_repeats("Add two tests. Add two tests."),
            "Add two tests."
        );
        // Not a pure repeat: left alone.
        assert_eq!(
            collapse_repeats(&format!("{s} 然后提交。")),
            format!("{s} 然后提交。")
        );
        assert_eq!(
            collapse_repeats("no no no"),
            "no no no",
            "short repeats may be intentional"
        );
        assert_eq!(collapse_repeats("test the test"), "test the test");
        assert_eq!(collapse_repeats(""), "");
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
