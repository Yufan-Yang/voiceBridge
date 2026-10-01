use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::fallback::normalize_transcript;
use super::schema::{PromptCompileInput, PromptCompileResult};
use crate::error::{AppError, Result};
use crate::types::{ProviderHealth, ProviderStatus};

#[async_trait]
pub trait PromptCompiler: Send + Sync {
    async fn health_check(&self) -> Result<ProviderHealth>;

    /// Starts the underlying runtime if it has one. Used by "Test model".
    async fn start(&self) -> Result<()> {
        Ok(())
    }

    async fn compile(
        &self,
        input: PromptCompileInput,
        cancel: CancellationToken,
    ) -> Result<PromptCompileResult>;
}

/// Rule-based compiler so the full pipeline runs with no model installed.
/// It only restructures what was said; it adds no requirements.
pub struct MockPromptCompiler {
    pub delay: Duration,
}

impl Default for MockPromptCompiler {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(500),
        }
    }
}

impl MockPromptCompiler {
    pub fn instant() -> Self {
        Self {
            delay: Duration::ZERO,
        }
    }
}

const FILLERS: &[&str] = &["um", "uh", "like", "you know", "so"];

fn clean_clause(clause: &str) -> Option<String> {
    let mut c = clause
        .trim()
        .trim_end_matches(['.', ',', ';'])
        .trim()
        .to_string();
    for prefix in ["and then ", "and ", "then "] {
        if c.to_ascii_lowercase().starts_with(prefix) {
            c = c[prefix.len()..].to_string();
        }
    }
    for filler in FILLERS {
        let lower = c.to_ascii_lowercase();
        if lower == *filler {
            return None;
        }
        if lower.starts_with(&format!("{filler} ")) {
            c = c[filler.len() + 1..].to_string();
        }
    }
    let mut chars = c.chars();
    let first = chars.next()?;
    let rest: String = chars.collect();
    let starts_lower_word = first.is_ascii_lowercase()
        && rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphabetic())
            .all(|ch| ch.is_ascii_lowercase());
    Some(if starts_lower_word {
        format!("{}{rest}", first.to_ascii_uppercase())
    } else {
        c
    })
}

#[async_trait]
impl PromptCompiler for MockPromptCompiler {
    async fn health_check(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth::new(
            "mock",
            ProviderStatus::Ready,
            "Mock compiler: rule-based formatting, no model required.",
        ))
    }

    async fn compile(
        &self,
        input: PromptCompileInput,
        cancel: CancellationToken,
    ) -> Result<PromptCompileResult> {
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::canceled()),
            _ = tokio::time::sleep(self.delay) => {}
        }
        let normalized = normalize_transcript(&input.raw_transcript, &input.project_terms);
        let clauses: Vec<String> = normalized
            .split(", ")
            .flat_map(|part| part.split(". "))
            .filter_map(clean_clause)
            .collect();
        let scope = match &input.target.active_file {
            Some(file) => format!("Task (in `{file}`):"),
            None => "Task:".to_string(),
        };
        let prompt = if clauses.len() <= 1 {
            normalized.clone()
        } else {
            let bullets: Vec<String> = clauses.iter().map(|c| format!("- {c}")).collect();
            format!("{scope}\n\n{}", bullets.join("\n"))
        };
        Ok(PromptCompileResult {
            normalized_transcript: normalized,
            intent: "task".to_string(),
            prompt,
            uncertain_identifiers: Vec::new(),
            needs_confirmation: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_compiler_structures_without_inventing_requirements() {
        let input = PromptCompileInput {
            raw_transcript: "um change use user query so it takes user id, add two tests, and don't change the return structure".into(),
            project_terms: vec!["useUserQuery".into(), "userId".into()],
            ..Default::default()
        };
        let out = MockPromptCompiler::instant()
            .compile(input, CancellationToken::new())
            .await
            .unwrap();
        assert!(out
            .prompt
            .starts_with("Task:\n\n- Change useUserQuery so it takes userId"));
        assert!(out.prompt.contains("- Add two tests"));
        assert!(out.prompt.contains("- Don't change the return structure"));
        assert_eq!(
            out.prompt.lines().filter(|l| l.starts_with("- ")).count(),
            3
        );
    }

    #[tokio::test]
    async fn mock_compiler_honours_cancellation() {
        let c = MockPromptCompiler {
            delay: Duration::from_secs(30),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = c
            .compile(PromptCompileInput::default(), cancel)
            .await
            .unwrap_err();
        assert!(err.is_canceled());
    }
}
