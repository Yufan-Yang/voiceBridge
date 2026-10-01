//! Builds ASR and Prompt Compiler providers from settings and owns their
//! sidecar managers.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::asr::funasr::FunAsrProvider;
use crate::asr::mock::MockAsr;
use crate::asr::whisper_cpp::WhisperCppProvider;
use crate::asr::AsrProvider;
use crate::config::{AsrProviderKind, ModelSettings, PromptProviderKind};
use crate::error::ErrorCode;
use crate::pipeline::Providers;
use crate::prompt::llama_cpp::LlamaCppCompiler;
use crate::prompt::{MockPromptCompiler, PromptCompiler};
use crate::sidecar::SidecarManager;
use crate::types::{ProviderHealth, ProviderStatus, ProvidersHealth};

/// Runtimes and models shipped inside the application bundle (the
/// "all-in-one" build). Each is used when the matching settings path is
/// empty, so an explicit path chosen by the user always wins.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BundledAssets {
    pub llama_server: Option<PathBuf>,
    pub prompt_model: Option<PathBuf>,
    pub whisper_cli: Option<PathBuf>,
    pub whisper_model: Option<PathBuf>,
}

impl BundledAssets {
    /// Looks for `llama/llama-server`, `whisper/whisper-cli`, the first
    /// `models/*.gguf` and the first `models/ggml-*.bin` under `dir`.
    pub fn discover(dir: &std::path::Path) -> Self {
        let file = |p: PathBuf| p.is_file().then_some(p);
        let model = |matches: fn(&str) -> bool| {
            let mut found: Vec<PathBuf> = std::fs::read_dir(dir.join("models"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(matches))
                .collect();
            found.sort();
            found.into_iter().next()
        };
        Self {
            llama_server: file(dir.join("llama").join("llama-server")),
            prompt_model: model(|n| n.ends_with(".gguf")),
            whisper_cli: file(dir.join("whisper").join("whisper-cli")),
            whisper_model: model(|n| n.starts_with("ggml-") && n.ends_with(".bin")),
        }
    }

    /// True when the bundle can run the whole pipeline with no configuration.
    pub fn is_complete(&self) -> bool {
        self.llama_server.is_some()
            && self.prompt_model.is_some()
            && self.whisper_cli.is_some()
            && self.whisper_model.is_some()
    }
}

/// The configured path, or the bundled one when the setting is empty.
fn resolve(configured: &str, bundled: &Option<PathBuf>) -> String {
    if !configured.trim().is_empty() {
        return configured.to_string();
    }
    bundled
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub struct ProviderHub {
    bundled: BundledAssets,
    pub asr_sidecar: Arc<SidecarManager>,
    pub prompt_sidecar: Arc<SidecarManager>,
    temp_dir: PathBuf,
    funasr_script: PathBuf,
}

impl ProviderHub {
    pub fn new(log_dir: PathBuf, temp_dir: PathBuf, funasr_script: PathBuf) -> Self {
        Self {
            asr_sidecar: Arc::new(SidecarManager::new(
                "asr",
                log_dir.clone(),
                ErrorCode::AsrProcessFailed,
            )),
            prompt_sidecar: Arc::new(SidecarManager::new(
                "prompt",
                log_dir,
                ErrorCode::PromptProcessFailed,
            )),
            temp_dir,
            funasr_script,
            bundled: BundledAssets::default(),
        }
    }

    pub fn with_bundled(mut self, bundled: BundledAssets) -> Self {
        self.bundled = bundled;
        self
    }

    pub fn bundled(&self) -> &BundledAssets {
        &self.bundled
    }

    /// Kills model processes orphaned by a previous run. Call once at startup.
    pub fn reap_orphans(&self) {
        self.asr_sidecar.reap_orphan();
        self.prompt_sidecar.reap_orphan();
    }

    pub fn temp_dir(&self) -> &PathBuf {
        &self.temp_dir
    }

    pub fn build(&self, models: &ModelSettings) -> Providers {
        let asr: Arc<dyn AsrProvider> = match models.asr_provider {
            AsrProviderKind::Mock => Arc::new(MockAsr::default()),
            AsrProviderKind::FunAsr => Arc::new(FunAsrProvider {
                sidecar: self.asr_sidecar.clone(),
                runtime: models.asr_runtime_path.clone(),
                model: models.asr_model_path.clone(),
                script: self.funasr_script.clone(),
                temp_dir: self.temp_dir.clone(),
                timeout: Duration::from_millis(models.asr_timeout_ms as u64),
            }),
            AsrProviderKind::WhisperCpp => Arc::new(WhisperCppProvider {
                runtime: resolve(&models.asr_runtime_path, &self.bundled.whisper_cli),
                model: resolve(&models.asr_model_path, &self.bundled.whisper_model),
                temp_dir: self.temp_dir.clone(),
            }),
        };
        let compiler: Arc<dyn PromptCompiler> = match models.prompt_provider {
            PromptProviderKind::Mock => Arc::new(MockPromptCompiler::default()),
            PromptProviderKind::LlamaCpp => Arc::new(LlamaCppCompiler::new(
                self.prompt_sidecar.clone(),
                resolve(&models.prompt_runtime_path, &self.bundled.llama_server),
                resolve(&models.prompt_model_path, &self.bundled.prompt_model),
                models.context_size,
                models.max_output_tokens,
            )),
        };
        Providers { asr, compiler }
    }

    pub async fn stop_all(&self) {
        self.asr_sidecar.stop().await;
        self.prompt_sidecar.stop().await;
    }
}

/// Health of both providers; a failing check is reported as unavailable.
pub async fn health(providers: &Providers) -> ProvidersHealth {
    let unavailable =
        |name: &str| ProviderHealth::new(name, ProviderStatus::Unavailable, "Health check failed.");
    ProvidersHealth {
        asr: providers
            .asr
            .health_check()
            .await
            .unwrap_or_else(|_| unavailable("asr")),
        prompt: providers
            .compiler
            .health_check()
            .await
            .unwrap_or_else(|_| unavailable("prompt")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;

    fn touch(path: &std::path::Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[tokio::test]
    async fn bundled_assets_are_used_when_paths_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("bundled");
        assert_eq!(BundledAssets::discover(&root), BundledAssets::default());
        touch(&root.join("llama/llama-server"));
        touch(&root.join("whisper/whisper-cli"));
        touch(&root.join("models/Qwen3.gguf"));
        touch(&root.join("models/ggml-large-v3-turbo.bin"));
        touch(&root.join("models/readme.txt"));
        let bundled = BundledAssets::discover(&root);
        assert!(bundled.is_complete());
        assert_eq!(bundled.prompt_model, Some(root.join("models/Qwen3.gguf")));
        assert_eq!(
            bundled.whisper_model,
            Some(root.join("models/ggml-large-v3-turbo.bin"))
        );

        let hub = ProviderHub::new(
            dir.path().into(),
            dir.path().into(),
            dir.path().join("x.py"),
        )
        .with_bundled(bundled);
        let mut models = Settings::default().models;
        models.asr_provider = AsrProviderKind::WhisperCpp;
        models.prompt_provider = PromptProviderKind::LlamaCpp;
        // Empty paths resolve to the bundle, so nothing is "not configured".
        let h = health(&hub.build(&models)).await;
        assert_eq!(h.asr.status, ProviderStatus::Ready);
        assert_eq!(
            h.prompt.status,
            ProviderStatus::Unavailable,
            "configured but not started yet"
        );

        // An explicit path always wins over the bundle.
        models.prompt_model_path = "/nonexistent/custom.gguf".into();
        models.asr_runtime_path = "/nonexistent/whisper-cli".into();
        let h = health(&hub.build(&models)).await;
        assert_eq!(h.prompt.status, ProviderStatus::NotConfigured);
        assert_eq!(h.asr.status, ProviderStatus::NotConfigured);
    }

    #[test]
    fn resolve_prefers_configured_path() {
        let bundled = Some(PathBuf::from("/bundle/x"));
        assert_eq!(resolve("/mine", &bundled), "/mine");
        assert_eq!(resolve("  ", &bundled), "/bundle/x");
        assert_eq!(resolve("", &None), "");
    }

    #[tokio::test]
    async fn defaults_build_mock_providers_that_are_ready_without_models() {
        let dir = tempfile::tempdir().unwrap();
        let hub = ProviderHub::new(
            dir.path().into(),
            dir.path().into(),
            dir.path().join("x.py"),
        );
        let providers = hub.build(&Settings::default().models);
        let h = health(&providers).await;
        assert_eq!(h.asr.status, ProviderStatus::Ready);
        assert_eq!(h.prompt.status, ProviderStatus::Ready);
    }

    #[tokio::test]
    async fn real_providers_without_paths_are_not_configured() {
        let dir = tempfile::tempdir().unwrap();
        let hub = ProviderHub::new(
            dir.path().into(),
            dir.path().into(),
            dir.path().join("x.py"),
        );
        let mut models = Settings::default().models;
        models.asr_provider = AsrProviderKind::FunAsr;
        models.prompt_provider = PromptProviderKind::LlamaCpp;
        let h = health(&hub.build(&models)).await;
        assert_eq!(h.asr.status, ProviderStatus::NotConfigured);
        assert_eq!(h.prompt.status, ProviderStatus::NotConfigured);
        models.asr_provider = AsrProviderKind::WhisperCpp;
        assert_eq!(
            health(&hub.build(&models)).await.asr.status,
            ProviderStatus::NotConfigured
        );
    }
}
