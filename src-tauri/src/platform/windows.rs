//! windows platform layer. Not implemented in the MVP: every capability returns
//! an explicit `Unsupported` error so nothing silently pretends to work.

use std::sync::Arc;

use super::*;
use crate::error::{AppError, Result};

pub struct Unsupported;

fn unsupported<T>(what: &str) -> Result<T> {
    Err(AppError::unsupported(&format!(
        "{what} is not implemented on windows"
    )))
}

impl WindowManager for Unsupported {
    fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        unsupported("window enumeration")
    }
    fn foreground_window(&self) -> Result<Option<WindowInfo>> {
        unsupported("foreground window lookup")
    }
    fn window_info(&self, _platform_window_id: &str) -> Result<Option<WindowInfo>> {
        unsupported("window lookup")
    }
    fn activate(&self, _window: &WindowInfo) -> Result<()> {
        unsupported("window activation")
    }
}

impl ClipboardManager for Unsupported {
    fn read_text(&self) -> Result<Option<String>> {
        unsupported("clipboard read")
    }
    fn write_text(&self, _text: &str) -> Result<()> {
        unsupported("clipboard write")
    }
    fn clear(&self) -> Result<()> {
        unsupported("clipboard clear")
    }
    fn change_count(&self) -> Result<i64> {
        unsupported("clipboard version")
    }
}

impl KeySimulator for Unsupported {
    fn paste(&self) -> Result<()> {
        unsupported("paste simulation")
    }
    fn enter(&self) -> Result<()> {
        unsupported("key simulation")
    }
}

impl PermissionManager for Unsupported {
    fn status(&self, _kind: PermissionKind) -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    fn request(&self, _kind: PermissionKind) -> PermissionStatus {
        PermissionStatus::Unsupported
    }
    fn open_settings(&self, _kind: PermissionKind) -> Result<()> {
        unsupported("opening permission settings")
    }
}

pub fn platform() -> Platform {
    Platform {
        windows: Arc::new(Unsupported),
        clipboard: Arc::new(Unsupported),
        keys: Arc::new(Unsupported),
        permissions: Arc::new(Unsupported),
    }
}
