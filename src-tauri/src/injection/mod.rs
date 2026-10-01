//! Text injection into pinned target windows.

pub mod clipboard;
pub mod generic;

use async_trait::async_trait;

use crate::error::Result;
use crate::target::TargetSlot;
use crate::types::InjectionResult;

pub use generic::GenericClipboardAdapter;

#[derive(Debug, Clone)]
pub struct InjectionOptions {
    /// Press Enter after pasting. Only ever true when the target enables it.
    pub auto_submit: bool,
    /// How long to wait for the target to become the foreground window.
    pub activation_timeout_ms: u32,
    /// Pause between the paste keystroke and restoring the clipboard.
    pub paste_settle_ms: u32,
}

impl Default for InjectionOptions {
    fn default() -> Self {
        Self {
            auto_submit: false,
            activation_timeout_ms: 1200,
            paste_settle_ms: 250,
        }
    }
}

#[async_trait]
pub trait InputInjector: Send + Sync {
    async fn inject(
        &self,
        target: &TargetSlot,
        text: &str,
        options: InjectionOptions,
    ) -> Result<InjectionResult>;
}
