//! Generic clipboard-paste injector for any top-level window.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::clipboard::ClipboardSession;
use super::{InjectionOptions, InputInjector};
use crate::error::{AppError, ErrorCode, Result};
use crate::platform::{ClipboardManager, KeySimulator, PermissionManager, WindowManager};
use crate::target::matcher::identity_matches;
use crate::target::TargetSlot;
use crate::types::{InjectionResult, PermissionKind, PermissionStatus};

const FOREGROUND_POLL: Duration = Duration::from_millis(40);

pub struct GenericClipboardAdapter {
    windows: Arc<dyn WindowManager>,
    clipboard: Arc<dyn ClipboardManager>,
    keys: Arc<dyn KeySimulator>,
    permissions: Arc<dyn PermissionManager>,
    /// Serializes every clipboard operation: once an injection has started,
    /// nothing else may touch the clipboard until it finishes.
    lock: Mutex<()>,
}

impl GenericClipboardAdapter {
    pub fn new(
        windows: Arc<dyn WindowManager>,
        clipboard: Arc<dyn ClipboardManager>,
        keys: Arc<dyn KeySimulator>,
        permissions: Arc<dyn PermissionManager>,
    ) -> Self {
        Self {
            windows,
            clipboard,
            keys,
            permissions,
            lock: Mutex::new(()),
        }
    }

    /// Copies text for the user (the Copy buttons), serialized with injections.
    pub async fn copy_text(&self, text: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        self.clipboard.write_text(text)
    }

    /// True only when the foreground window is exactly the pinned target.
    fn target_is_foreground(&self, target: &TargetSlot) -> Result<bool> {
        Ok(matches!(
            self.windows.foreground_window()?,
            Some(ref fg) if identity_matches(target, fg)
        ))
    }

    fn require_foreground(&self, target: &TargetSlot) -> Result<()> {
        if self.target_is_foreground(target)? {
            Ok(())
        } else {
            Err(AppError::new(ErrorCode::ForegroundVerificationFailed))
        }
    }
}

#[async_trait]
impl InputInjector for GenericClipboardAdapter {
    async fn inject(
        &self,
        target: &TargetSlot,
        text: &str,
        options: InjectionOptions,
    ) -> Result<InjectionResult> {
        let _guard = self.lock.lock().await;

        if self.permissions.status(PermissionKind::Accessibility) == PermissionStatus::Denied {
            return Err(AppError::new(ErrorCode::AccessibilityPermissionDenied));
        }

        // 1. The target window must still exist.
        let window = self
            .windows
            .window_info(&target.platform_window_id)?
            .ok_or_else(|| {
                AppError::new(ErrorCode::TargetNotFound).with_details("target window closed")
            })?;

        // 2. Window id, process id and application identity must still match.
        if !identity_matches(target, &window) {
            return Err(AppError::new(ErrorCode::TargetIdentityMismatch));
        }

        // 3. Activate the target window.
        self.windows.activate(&window).map_err(|e| match e.code {
            ErrorCode::AccessibilityPermissionDenied | ErrorCode::Unsupported => e,
            _ => AppError::new(ErrorCode::TargetActivationFailed).with_details(e.details),
        })?;

        // 4–5. Re-read the foreground window until it is confirmed to be the target.
        let attempts =
            (options.activation_timeout_ms as u64 / FOREGROUND_POLL.as_millis() as u64).max(1);
        let mut confirmed = false;
        for _ in 0..attempts {
            if self.target_is_foreground(target)? {
                confirmed = true;
                break;
            }
            tokio::time::sleep(FOREGROUND_POLL).await;
        }
        if !confirmed {
            return Err(AppError::new(ErrorCode::ForegroundVerificationFailed));
        }

        // 6–7. Save the clipboard (contents + version), then write our text.
        let session = ClipboardSession::begin(self.clipboard.as_ref(), text)?;

        // Last check immediately before the keystroke: never paste into an
        // unverified foreground window.
        if let Err(e) = self.require_foreground(target) {
            let _ = session.restore();
            return Err(e);
        }

        // 8. Simulate the paste shortcut.
        if let Err(e) = self.keys.paste() {
            let _ = session.restore();
            return Err(match e.code {
                ErrorCode::AccessibilityPermissionDenied | ErrorCode::Unsupported => e,
                _ => AppError::new(ErrorCode::InputInjectionFailed).with_details(e.details),
            });
        }

        // 9. Give the application time to read the clipboard.
        tokio::time::sleep(Duration::from_millis(options.paste_settle_ms as u64)).await;

        // 10–11. Restore the previous clipboard unless the user changed it.
        let clipboard_restored = session.restore().unwrap_or(false);

        // 12–13. Enter is pressed only when auto_submit is explicitly enabled,
        // and only if the target is still in front.
        let mut submitted = false;
        if options.auto_submit {
            self.require_foreground(target)?;
            self.keys.enter().map_err(|e| {
                AppError::new(ErrorCode::InputInjectionFailed).with_details(e.details)
            })?;
            submitted = true;
        }

        Ok(InjectionResult {
            target_id: target.id.clone(),
            chars: text.chars().count() as u32,
            submitted,
            clipboard_restored,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockDesktop;
    use crate::types::OutputKind;

    fn setup() -> (MockDesktop, GenericClipboardAdapter, TargetSlot) {
        let desk = MockDesktop::new();
        let editor = MockDesktop::window("10", 100, "/Apps/Code", "backend");
        desk.add_window(editor.clone());
        desk.add_window(MockDesktop::window("20", 200, "/Apps/Mail", "Inbox"));
        desk.set_foreground("20");
        desk.write_text("user clipboard").unwrap();
        let p = desk.platform();
        let adapter = GenericClipboardAdapter::new(p.windows, p.clipboard, p.keys, p.permissions);
        let target = TargetSlot::from_window(&editor, 1, OutputKind::Prompt);
        (desk, adapter, target)
    }

    fn fast() -> InjectionOptions {
        InjectionOptions {
            auto_submit: false,
            activation_timeout_ms: 120,
            paste_settle_ms: 5,
        }
    }

    #[tokio::test]
    async fn pastes_into_target_restores_clipboard_and_does_not_press_enter() {
        let (desk, adapter, target) = setup();
        let r = adapter
            .inject(&target, "hello prompt", fast())
            .await
            .unwrap();
        let s = desk.state.lock().unwrap();
        assert_eq!(
            s.pastes,
            vec![(Some("10".to_string()), "hello prompt".to_string())]
        );
        assert_eq!(s.enters, 0, "Enter must not be pressed by default");
        assert_eq!(s.clipboard.as_deref(), Some("user clipboard"));
        assert!(r.clipboard_restored && !r.submitted);
        assert_eq!(r.chars, 12);
    }

    #[tokio::test]
    async fn enter_is_pressed_only_with_auto_submit() {
        let (desk, adapter, target) = setup();
        let opts = InjectionOptions {
            auto_submit: true,
            ..fast()
        };
        let r = adapter.inject(&target, "go", opts).await.unwrap();
        assert!(r.submitted);
        assert_eq!(desk.state.lock().unwrap().enters, 1);
        assert!(!InjectionOptions::default().auto_submit);
    }

    #[tokio::test]
    async fn blocked_when_target_window_closed() {
        let (desk, adapter, target) = setup();
        desk.remove_window("10");
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::TargetNotFound);
        assert!(desk.state.lock().unwrap().pastes.is_empty());
    }

    #[tokio::test]
    async fn blocked_when_target_identity_does_not_match() {
        let (desk, adapter, target) = setup();
        // The window id was recycled by a different process / application.
        desk.remove_window("10");
        desk.add_window(MockDesktop::window("10", 555, "/Apps/Terminal", "backend"));
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::TargetIdentityMismatch);
        let s = desk.state.lock().unwrap();
        assert!(s.pastes.is_empty() && s.activations.is_empty());
        assert_eq!(
            s.clipboard.as_deref(),
            Some("user clipboard"),
            "clipboard untouched"
        );
    }

    #[tokio::test]
    async fn blocked_when_foreground_verification_fails() {
        let (desk, adapter, target) = setup();
        // Activation "succeeds" but another window stays in front.
        desk.state.lock().unwrap().activation_takes_effect = false;
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::ForegroundVerificationFailed);
        let s = desk.state.lock().unwrap();
        assert!(
            s.pastes.is_empty(),
            "nothing may be pasted into an unverified window"
        );
        assert_eq!(s.clipboard.as_deref(), Some("user clipboard"));
    }

    #[tokio::test]
    async fn activation_failure_and_missing_permission() {
        let (desk, adapter, target) = setup();
        desk.state.lock().unwrap().activation_fails = true;
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::TargetActivationFailed);

        desk.state.lock().unwrap().accessibility = PermissionStatus::Denied;
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AccessibilityPermissionDenied);
        assert!(desk.state.lock().unwrap().pastes.is_empty());
    }

    #[tokio::test]
    async fn clipboard_and_paste_failures_are_reported() {
        let (desk, adapter, target) = setup();
        desk.state.lock().unwrap().clipboard_write_fails = true;
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::ClipboardFailed);

        {
            let mut s = desk.state.lock().unwrap();
            s.clipboard_write_fails = false;
            s.paste_fails = true;
        }
        let err = adapter.inject(&target, "x", fast()).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InputInjectionFailed);
        assert_eq!(
            desk.state.lock().unwrap().clipboard.as_deref(),
            Some("user clipboard"),
            "clipboard is restored after a failed paste"
        );
    }

    #[tokio::test]
    async fn user_clipboard_change_is_not_overwritten() {
        let (desk, adapter, target) = setup();
        desk.state.lock().unwrap().user_copies_after_paste = Some("fresh user copy".into());
        let r = adapter.inject(&target, "injected", fast()).await.unwrap();
        assert!(!r.clipboard_restored);
        assert_eq!(
            desk.state.lock().unwrap().clipboard.as_deref(),
            Some("fresh user copy")
        );
    }

    #[tokio::test]
    async fn concurrent_injections_are_serialized() {
        let (desk, adapter, target) = setup();
        let adapter = Arc::new(adapter);
        let mut handles = Vec::new();
        for i in 0..4 {
            let (a, t) = (adapter.clone(), target.clone());
            handles.push(tokio::spawn(async move {
                a.inject(&t, &format!("text {i}"), fast()).await
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let s = desk.state.lock().unwrap();
        assert_eq!(s.pastes.len(), 4);
        // Each paste saw exactly its own text, and the user's clipboard survived.
        let mut texts: Vec<_> = s.pastes.iter().map(|(_, t)| t.clone()).collect();
        texts.sort();
        assert_eq!(texts, vec!["text 0", "text 1", "text 2", "text 3"]);
        assert_eq!(s.clipboard.as_deref(), Some("user clipboard"));
    }
}
