//! Maps sidecar process state to provider health.

use std::path::Path;

use super::manager::SidecarState;
use crate::types::{ProviderHealth, ProviderStatus};

/// Returns a description of what is missing, or `None` when both paths exist.
pub fn missing_paths(runtime: &str, model: &str) -> Option<String> {
    if runtime.trim().is_empty() {
        return Some("Runtime executable is not set.".into());
    }
    if !Path::new(runtime).is_file() {
        return Some("Runtime executable does not exist.".into());
    }
    if model.trim().is_empty() {
        return Some("Model path is not set.".into());
    }
    if !Path::new(model).exists() {
        return Some("Model path does not exist.".into());
    }
    None
}

/// Health of a provider backed by a managed process that is not (yet)
/// answering requests.
pub fn health_from_state(provider: &str, state: SidecarState) -> ProviderHealth {
    match state {
        SidecarState::Running => {
            ProviderHealth::new(provider, ProviderStatus::Starting, "Sidecar is running.")
        }
        SidecarState::NotStarted => ProviderHealth::new(
            provider,
            ProviderStatus::Unavailable,
            "Sidecar is not running. It starts on first use or when you press Test.",
        ),
        SidecarState::Crashed => ProviderHealth::new(
            provider,
            ProviderStatus::Unavailable,
            "Sidecar exited unexpectedly. It will be restarted on the next request.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crashed_sidecar_reports_unavailable() {
        let h = health_from_state("llama.cpp", SidecarState::Crashed);
        assert_eq!(h.status, ProviderStatus::Unavailable);
        assert_eq!(
            health_from_state("x", SidecarState::NotStarted).status,
            ProviderStatus::Unavailable
        );
        assert_eq!(
            health_from_state("x", SidecarState::Running).status,
            ProviderStatus::Starting
        );
    }

    #[test]
    fn validates_paths() {
        assert!(missing_paths("", "").is_some());
        assert!(missing_paths("/nonexistent/bin", "/tmp").is_some());
        assert!(missing_paths("/bin/sh", "").is_some());
        assert!(missing_paths("/bin/sh", "/nonexistent/model.gguf").is_some());
        #[cfg(unix)]
        assert!(missing_paths("/bin/sh", "/bin").is_none());
    }
}
