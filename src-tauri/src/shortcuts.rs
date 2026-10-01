//! Global shortcut registration.

use std::sync::Arc;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use crate::config::ShortcutSettings;
use crate::error::{AppError, ErrorCode, Result};
use crate::platform::{GlobalShortcutManager, ShortcutAction};

/// All (accelerator, action) pairs described by the settings.
pub fn bindings(settings: &ShortcutSettings) -> Vec<(String, ShortcutAction)> {
    let mut out = vec![
        (settings.push_to_talk.clone(), ShortcutAction::PushToTalk),
        (settings.cancel.clone(), ShortcutAction::Cancel),
        (settings.prev_target.clone(), ShortcutAction::PrevTarget),
        (settings.next_target.clone(), ShortcutAction::NextTarget),
        (
            settings.inject_last_raw.clone(),
            ShortcutAction::InjectLastRaw,
        ),
        (
            settings.inject_last_prompt.clone(),
            ShortcutAction::InjectLastPrompt,
        ),
    ];
    for (i, accel) in settings.select_slot.iter().enumerate() {
        out.push((accel.clone(), ShortcutAction::SelectSlot(i as u8 + 1)));
    }
    for (i, accel) in settings.bind_slot.iter().enumerate() {
        out.push((accel.clone(), ShortcutAction::BindSlot(i as u8 + 1)));
    }
    out
}

/// Replaces all registrations. Blank accelerators are skipped (disabled).
/// A bad or conflicting accelerator is reported but does not stop the rest.
pub fn register_all(
    manager: &dyn GlobalShortcutManager,
    settings: &ShortcutSettings,
) -> Vec<(String, AppError)> {
    let _ = manager.unregister_all();
    let mut failures = Vec::new();
    for (accel, action) in bindings(settings) {
        if accel.trim().is_empty() {
            continue;
        }
        if let Err(e) = manager.register(&accel, action) {
            failures.push((accel, e));
        }
    }
    failures
}

pub type ShortcutHandler = Arc<dyn Fn(ShortcutAction, bool) + Send + Sync>;

/// `GlobalShortcutManager` backed by the Tauri global-shortcut plugin.
/// The handler receives the action and whether the key was pressed (true) or
/// released (false).
pub struct TauriShortcuts {
    pub app: AppHandle,
    pub handler: ShortcutHandler,
}

impl GlobalShortcutManager for TauriShortcuts {
    fn register(&self, accelerator: &str, action: ShortcutAction) -> Result<()> {
        let handler = self.handler.clone();
        self.app
            .global_shortcut()
            .on_shortcut(accelerator, move |_app, _shortcut, event| {
                handler(action, event.state == ShortcutState::Pressed);
            })
            .map_err(|e| {
                AppError::new(ErrorCode::InvalidInput)
                    .with_message(format!(
                        "The shortcut {accelerator} could not be registered."
                    ))
                    .with_details(e.to_string())
            })
    }

    fn unregister_all(&self) -> Result<()> {
        self.app
            .global_shortcut()
            .unregister_all()
            .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::mock::MockShortcuts;

    #[test]
    fn registers_every_configured_action() {
        let mock = MockShortcuts::default();
        let failures = register_all(&mock, &ShortcutSettings::default());
        assert!(failures.is_empty());
        let registered = mock.registered.lock().unwrap();
        // PTT, cancel, prev, next, 2 inject + 9 select + 9 bind.
        assert_eq!(registered.len(), 24);
        assert!(registered.contains(&("Ctrl+Alt+Space".to_string(), ShortcutAction::PushToTalk)));
        assert!(registered.contains(&("Ctrl+Alt+3".to_string(), ShortcutAction::SelectSlot(3))));
        assert!(registered.contains(&("Ctrl+Alt+Shift+9".to_string(), ShortcutAction::BindSlot(9))));
    }

    #[test]
    fn bad_shortcut_does_not_block_the_others_and_blank_disables() {
        let mock = MockShortcuts {
            invalid: vec!["Ctrl+Alt+P".to_string()],
            ..Default::default()
        };
        let settings = ShortcutSettings {
            inject_last_raw: String::new(),
            ..Default::default()
        };
        let failures = register_all(&mock, &settings);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "Ctrl+Alt+P");
        assert_eq!(mock.registered.lock().unwrap().len(), 22);

        // Re-registering replaces rather than accumulates.
        register_all(&mock, &settings);
        assert_eq!(mock.registered.lock().unwrap().len(), 22);
    }
}
