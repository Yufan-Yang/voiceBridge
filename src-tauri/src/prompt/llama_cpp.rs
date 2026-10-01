//! Prompt Compiler backed by a local llama.cpp `llama-server` sidecar bound
//! to 127.0.0.1 on a random port and protected by a per-session API key.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::compiler::PromptCompiler;
use super::schema::{
    output_schema, parse_model_output, PromptCompileInput, PromptCompileResult, SYSTEM_PROMPT,
};
use crate::error::{AppError, ErrorCode, Result};
use crate::sidecar::health::{health_from_state, missing_paths};
use crate::sidecar::manager::free_loopback_port;
use crate::sidecar::{SidecarManager, SidecarSpec, SidecarState};
use crate::types::{ProviderHealth, ProviderStatus};

const PROVIDER: &str = "llama.cpp";
const HOST: &str = "127.0.0.1";
const START_TIMEOUT: Duration = Duration::from_secs(180);

pub struct LlamaCppCompiler {
    pub sidecar: Arc<SidecarManager>,
    pub runtime: String,
    pub model: String,
    pub context_size: u32,
    pub max_output_tokens: u32,
    client: reqwest::Client,
    port: AtomicU16,
}

impl LlamaCppCompiler {
    pub fn new(
        sidecar: Arc<SidecarManager>,
        runtime: String,
        model: String,
        context_size: u32,
        max_output_tokens: u32,
    ) -> Self {
        Self {
            sidecar,
            runtime,
            model,
            context_size,
            max_output_tokens,
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
            Some(msg) => Err(AppError::new(ErrorCode::PromptModelNotConfigured).with_details(msg)),
            None => Ok(()),
        }
    }

    /// Command line for `llama-server`. Always binds the loopback interface.
    pub fn server_args(model: &str, port: u16, context_size: u32) -> Vec<String> {
        vec![
            "-m".to_string(),
            model.to_string(),
            "--host".to_string(),
            HOST.to_string(),
            "--port".to_string(),
            port.to_string(),
            "-c".to_string(),
            context_size.to_string(),
        ]
    }

    fn url(&self, path: &str) -> String {
        format!("http://{HOST}:{}{path}", self.port.load(Ordering::SeqCst))
    }

    async fn is_healthy(&self) -> bool {
        if self.port.load(Ordering::SeqCst) == 0 {
            return false;
        }
        matches!(
            self.client
                .get(self.url("/health"))
                .bearer_auth(self.sidecar.token())
                .timeout(Duration::from_secs(2))
                .send()
                .await,
            Ok(r) if r.status().is_success()
        )
    }

    /// Starts the server if it is not running (this is also the crash
    /// recovery path) and waits until the model is loaded.
    async fn ensure_started(&self, cancel: &CancellationToken) -> Result<()> {
        self.check_config()?;
        let _starting = self.sidecar.start_lock.lock().await;
        if self.sidecar.state().await != SidecarState::Running {
            let port = free_loopback_port()?;
            self.port.store(port, Ordering::SeqCst);
            self.sidecar
                .start(SidecarSpec {
                    program: PathBuf::from(&self.runtime),
                    args: Self::server_args(&self.model, port, self.context_size),
                    // Passed by environment so the key is not visible in `ps`.
                    env: vec![(
                        "LLAMA_API_KEY".to_string(),
                        self.sidecar.token().to_string(),
                    )],
                    stdio_protocol: false,
                })
                .await?;
        }
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if cancel.is_cancelled() {
                return Err(AppError::canceled());
            }
            if self.is_healthy().await {
                return Ok(());
            }
            if self.sidecar.state().await != SidecarState::Running {
                return Err(AppError::new(ErrorCode::PromptProcessFailed)
                    .with_details("llama-server exited during startup; see sidecar-prompt.log"));
            }
            if Instant::now() >= deadline {
                return Err(
                    AppError::new(ErrorCode::PromptTimeout).with_details("model load timed out")
                );
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(AppError::canceled()),
                _ = tokio::time::sleep(Duration::from_millis(300)) => {}
            }
        }
    }

    /// Chat-completion request body with schema-constrained output.
    pub fn request_body(input: &PromptCompileInput, max_output_tokens: u32) -> Value {
        json!({
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": serde_json::to_string(input).unwrap_or_default() }
            ],
            "temperature": 0.1,
            "max_tokens": max_output_tokens,
            "stream": false,
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "compiled_prompt", "strict": true, "schema": output_schema(true) }
            }
        })
    }
}

#[async_trait]
impl PromptCompiler for LlamaCppCompiler {
    async fn health_check(&self) -> Result<ProviderHealth> {
        if let Err(e) = self.check_config() {
            return Ok(ProviderHealth::new(
                PROVIDER,
                ProviderStatus::NotConfigured,
                e.details,
            ));
        }
        let state = self.sidecar.state().await;
        if state == SidecarState::Running && self.is_healthy().await {
            return Ok(ProviderHealth::new(
                PROVIDER,
                ProviderStatus::Ready,
                "Model loaded.",
            ));
        }
        Ok(health_from_state(PROVIDER, state))
    }

    async fn start(&self) -> Result<()> {
        self.ensure_started(&CancellationToken::new()).await
    }

    async fn compile(
        &self,
        input: PromptCompileInput,
        cancel: CancellationToken,
    ) -> Result<PromptCompileResult> {
        self.ensure_started(&cancel).await?;
        let request = self
            .client
            .post(self.url("/v1/chat/completions"))
            .bearer_auth(self.sidecar.token())
            .json(&Self::request_body(&input, self.max_output_tokens))
            .send();
        // Dropping the request on cancel closes the connection, which makes
        // llama-server stop generating.
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            r = request => r.map_err(|e| {
                AppError::new(ErrorCode::PromptProcessFailed)
                    .with_details(if e.is_connect() { "connection failed" } else { "request failed" })
            })?,
        };
        if !response.status().is_success() {
            return Err(AppError::new(ErrorCode::PromptProcessFailed)
                .with_details(format!("http status {}", response.status().as_u16())));
        }
        let body: Value = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            b = response.json() => b.map_err(|_| {
                AppError::new(ErrorCode::PromptInvalidJson).with_details("response body was not JSON")
            })?,
        };
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| {
                AppError::new(ErrorCode::PromptInvalidJson).with_details("missing message content")
            })?;
        parse_model_output(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiler(dir: &std::path::Path, runtime: &str, model: &str) -> LlamaCppCompiler {
        LlamaCppCompiler::new(
            Arc::new(SidecarManager::new(
                "prompt-test",
                dir.to_path_buf(),
                ErrorCode::PromptProcessFailed,
            )),
            runtime.into(),
            model.into(),
            8192,
            512,
        )
    }

    #[test]
    fn server_only_binds_loopback() {
        let args = LlamaCppCompiler::server_args("/m.gguf", 4242, 8192);
        assert!(args.windows(2).any(|w| w == ["--host", "127.0.0.1"]));
        assert!(!args.iter().any(|a| a == "0.0.0.0"));
        assert!(args.windows(2).any(|w| w == ["-c", "8192"]));
    }

    #[test]
    fn request_uses_system_prompt_schema_and_token_limit() {
        let input = PromptCompileInput {
            raw_transcript: "do it".into(),
            ..Default::default()
        };
        let body = LlamaCppCompiler::request_body(&input, 512);
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["additionalProperties"],
            false
        );
        assert!(body["messages"][1]["content"]
            .as_str()
            .unwrap()
            .contains("\"raw_transcript\":\"do it\""));
    }

    #[tokio::test]
    async fn unconfigured_model_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let c = compiler(dir.path(), "", "");
        assert_eq!(
            c.health_check().await.unwrap().status,
            ProviderStatus::NotConfigured
        );
        let err = c
            .compile(PromptCompileInput::default(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PromptModelNotConfigured);
    }

    /// A runtime that dies immediately must surface as a process failure and
    /// leave the provider unavailable — not hang.
    #[cfg(unix)]
    #[tokio::test]
    async fn crashed_sidecar_changes_status_to_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        // `/usr/bin/false` ignores its arguments and exits with status 1.
        let c = compiler(dir.path(), "/usr/bin/false", "/bin/sh");
        let err = c
            .compile(PromptCompileInput::default(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::PromptProcessFailed);
        assert_eq!(
            c.health_check().await.unwrap().status,
            ProviderStatus::Unavailable
        );
    }
}
