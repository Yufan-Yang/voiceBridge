//! In-memory desktop used by tests and available for headless demos.

use std::sync::{Arc, Mutex};

use super::*;
use crate::error::{AppError, ErrorCode, Result};

#[derive(Default)]
pub struct MockDesktopState {
    pub windows: Vec<WindowInfo>,
    pub foreground: Option<String>,
    pub clipboard: Option<String>,
    pub change_count: i64,
    /// (foreground window id at paste time, clipboard text pasted)
    pub pastes: Vec<(Option<String>, String)>,
    pub enters: u32,
    pub activations: Vec<String>,
    /// When false, `activate` reports success but the foreground is unchanged.
    pub activation_takes_effect: bool,
    pub activation_fails: bool,
    /// Simulates the user copying something right after the paste.
    pub user_copies_after_paste: Option<String>,
    pub paste_fails: bool,
    pub clipboard_write_fails: bool,
    pub accessibility: PermissionStatus,
    pub microphone: PermissionStatus,
}

/// One object implementing every platform trait over shared state.
#[derive(Clone)]
pub struct MockDesktop {
    pub state: Arc<Mutex<MockDesktopState>>,
}

impl Default for MockDesktop {
    fn default() -> Self {
        Self::new()
    }
}

impl MockDesktop {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockDesktopState {
                activation_takes_effect: true,
                accessibility: PermissionStatus::Granted,
                microphone: PermissionStatus::Granted,
                ..Default::default()
            })),
        }
    }

    pub fn window(id: &str, pid: u32, exe: &str, title: &str) -> WindowInfo {
        WindowInfo {
            platform_window_id: id.to_string(),
            process_id: pid,
            executable_or_bundle_id: exe.to_string(),
            app_name: exe.rsplit('/').next().unwrap_or(exe).to_string(),
            title: title.to_string(),
        }
    }

    pub fn add_window(&self, w: WindowInfo) {
        self.state.lock().unwrap().windows.push(w);
    }

    pub fn remove_window(&self, id: &str) {
        let mut s = self.state.lock().unwrap();
        s.windows.retain(|w| w.platform_window_id != id);
        if s.foreground.as_deref() == Some(id) {
            s.foreground = None;
        }
    }

    pub fn set_foreground(&self, id: &str) {
        self.state.lock().unwrap().foreground = Some(id.to_string());
    }

    pub fn platform(&self) -> Platform {
        Platform {
            windows: Arc::new(self.clone()),
            clipboard: Arc::new(self.clone()),
            keys: Arc::new(self.clone()),
            permissions: Arc::new(self.clone()),
        }
    }
}

impl WindowManager for MockDesktop {
    fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(self.state.lock().unwrap().windows.clone())
    }

    fn foreground_window(&self) -> Result<Option<WindowInfo>> {
        let s = self.state.lock().unwrap();
        Ok(s.foreground.as_ref().and_then(|id| {
            s.windows
                .iter()
                .find(|w| &w.platform_window_id == id)
                .cloned()
        }))
    }

    fn window_info(&self, platform_window_id: &str) -> Result<Option<WindowInfo>> {
        let s = self.state.lock().unwrap();
        Ok(s.windows
            .iter()
            .find(|w| w.platform_window_id == platform_window_id)
            .cloned())
    }

    fn activate(&self, window: &WindowInfo) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.activation_fails {
            return Err(AppError::new(ErrorCode::TargetActivationFailed));
        }
        s.activations.push(window.platform_window_id.clone());
        if s.activation_takes_effect {
            s.foreground = Some(window.platform_window_id.clone());
        }
        Ok(())
    }
}

impl ClipboardManager for MockDesktop {
    fn read_text(&self) -> Result<Option<String>> {
        Ok(self.state.lock().unwrap().clipboard.clone())
    }

    fn write_text(&self, text: &str) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.clipboard_write_fails {
            return Err(AppError::new(ErrorCode::ClipboardFailed));
        }
        s.clipboard = Some(text.to_string());
        s.change_count += 1;
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.clipboard = None;
        s.change_count += 1;
        Ok(())
    }

    fn change_count(&self) -> Result<i64> {
        Ok(self.state.lock().unwrap().change_count)
    }
}

impl KeySimulator for MockDesktop {
    fn paste(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if s.paste_fails {
            return Err(AppError::new(ErrorCode::InputInjectionFailed));
        }
        let text = s.clipboard.clone().unwrap_or_default();
        let fg = s.foreground.clone();
        s.pastes.push((fg, text));
        if let Some(user) = s.user_copies_after_paste.take() {
            s.clipboard = Some(user);
            s.change_count += 1;
        }
        Ok(())
    }

    fn enter(&self) -> Result<()> {
        self.state.lock().unwrap().enters += 1;
        Ok(())
    }
}

impl PermissionManager for MockDesktop {
    fn status(&self, kind: PermissionKind) -> PermissionStatus {
        let s = self.state.lock().unwrap();
        match kind {
            PermissionKind::Microphone => s.microphone,
            PermissionKind::Accessibility => s.accessibility,
        }
    }

    fn request(&self, kind: PermissionKind) -> PermissionStatus {
        self.status(kind)
    }

    fn open_settings(&self, _kind: PermissionKind) -> Result<()> {
        Ok(())
    }
}

/// Records shortcut registrations; rejects accelerators listed in `invalid`.
#[derive(Default)]
pub struct MockShortcuts {
    pub registered: Mutex<Vec<(String, ShortcutAction)>>,
    pub invalid: Vec<String>,
}

impl GlobalShortcutManager for MockShortcuts {
    fn register(&self, accelerator: &str, action: ShortcutAction) -> Result<()> {
        if accelerator.is_empty() || self.invalid.iter().any(|a| a == accelerator) {
            return Err(AppError::new(ErrorCode::InvalidInput).with_details("invalid accelerator"));
        }
        let mut r = self.registered.lock().unwrap();
        if r.iter().any(|(a, _)| a == accelerator) {
            return Err(
                AppError::new(ErrorCode::InvalidInput).with_details("duplicate accelerator")
            );
        }
        r.push((accelerator.to_string(), action));
        Ok(())
    }

    fn unregister_all(&self) -> Result<()> {
        self.registered.lock().unwrap().clear();
        Ok(())
    }
}
