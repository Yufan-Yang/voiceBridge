//! Cross-platform interfaces. Each operating system provides its own
//! implementation; unsupported platforms return explicit `Unsupported` errors.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::Result;
use crate::types::{PermissionKind, PermissionStatus};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod mock;
#[cfg(target_os = "windows")]
pub mod windows;

/// A visible top-level desktop window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WindowInfo {
    pub platform_window_id: String,
    pub process_id: u32,
    pub executable_or_bundle_id: String,
    pub app_name: String,
    pub title: String,
}

pub trait WindowManager: Send + Sync {
    /// Visible top-level windows, front to back.
    fn list_windows(&self) -> Result<Vec<WindowInfo>>;
    fn foreground_window(&self) -> Result<Option<WindowInfo>>;
    /// Look up a window by platform id. `None` when it no longer exists.
    fn window_info(&self, platform_window_id: &str) -> Result<Option<WindowInfo>>;
    fn activate(&self, window: &WindowInfo) -> Result<()>;
}

pub trait ClipboardManager: Send + Sync {
    fn read_text(&self) -> Result<Option<String>>;
    fn write_text(&self, text: &str) -> Result<()>;
    fn clear(&self) -> Result<()>;
    /// Monotonic clipboard version; changes whenever anyone writes.
    fn change_count(&self) -> Result<i64>;
}

/// Low-level key simulation used by input injectors.
pub trait KeySimulator: Send + Sync {
    fn paste(&self) -> Result<()>;
    fn enter(&self) -> Result<()>;
}

pub trait PermissionManager: Send + Sync {
    fn status(&self, kind: PermissionKind) -> PermissionStatus;
    /// Ask the OS to show its permission prompt, where one exists.
    fn request(&self, kind: PermissionKind) -> PermissionStatus;
    fn open_settings(&self, kind: PermissionKind) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutAction {
    PushToTalk,
    Cancel,
    SelectSlot(u8),
    BindSlot(u8),
    PrevTarget,
    NextTarget,
    InjectLastRaw,
    InjectLastPrompt,
}

pub trait GlobalShortcutManager: Send + Sync {
    fn register(&self, accelerator: &str, action: ShortcutAction) -> Result<()>;
    fn unregister_all(&self) -> Result<()>;
}

pub struct Platform {
    pub windows: Arc<dyn WindowManager>,
    pub clipboard: Arc<dyn ClipboardManager>,
    pub keys: Arc<dyn KeySimulator>,
    pub permissions: Arc<dyn PermissionManager>,
}

/// Native implementation for the current operating system.
pub fn native() -> Platform {
    #[cfg(target_os = "macos")]
    {
        macos::platform()
    }
    #[cfg(target_os = "windows")]
    {
        windows::platform()
    }
    #[cfg(target_os = "linux")]
    {
        linux::platform()
    }
}

pub const CURRENT_OS: &str = std::env::consts::OS;
