//! Overlay window behaviour: non-activating, always on top, remembered
//! position, and kept on a visible display.

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow, Window, WindowEvent};

use crate::app_state::AppState;

pub const OVERLAY_LABEL: &str = "overlay";
pub const SETTINGS_LABEL: &str = "settings";

/// True when the point lies on any connected display.
fn on_any_monitor(window: &WebviewWindow, x: i32, y: i32) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    monitors.iter().any(|m| {
        let (p, s) = (m.position(), m.size());
        x >= p.x && y >= p.y && x < p.x + s.width as i32 - 40 && y < p.y + s.height as i32 - 20
    })
}

fn move_to_default(window: &WebviewWindow) {
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let width = window.outer_size().map(|s| s.width as i32).unwrap_or(420);
    let (p, s) = (monitor.position(), monitor.size());
    let x = p.x + (s.width as i32 - width) / 2;
    let y = p.y + (64.0 * monitor.scale_factor()) as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// Moves the overlay back onto a display if its position is no longer
/// visible (for example after a monitor was unplugged).
pub fn ensure_visible(app: &AppHandle) {
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };
    if let Ok(pos) = window.outer_position() {
        if !on_any_monitor(&window, pos.x, pos.y) {
            move_to_default(&window);
        }
    }
}

pub fn init(app: &AppHandle, saved: Option<(i32, i32)>) {
    let Some(window) = app.get_webview_window(OVERLAY_LABEL) else {
        return;
    };
    match saved {
        Some((x, y)) if on_any_monitor(&window, x, y) => {
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        _ => move_to_default(&window),
    }

    #[cfg(target_os = "macos")]
    {
        // `setup` runs on the main thread, which AppKit requires here.
        if let Ok(ns_window) = window.ns_window() {
            unsafe { crate::platform::macos::make_non_activating_panel(ns_window) };
        }
    }
    let _ = window.set_always_on_top(true);
    let _ = window.show();

    // The settings window stays hidden until the user opens it.
    if let Some(settings) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = settings.hide();
    }
}

pub fn on_window_event(window: &Window, event: &WindowEvent) {
    match (window.label(), event) {
        (OVERLAY_LABEL, WindowEvent::Moved(pos)) => {
            if let Some(state) = window.app_handle().try_state::<AppState>() {
                *state.overlay_position.lock().unwrap() = Some((pos.x, pos.y, true));
            }
        }
        (OVERLAY_LABEL, WindowEvent::ScaleFactorChanged { .. }) => {
            ensure_visible(window.app_handle())
        }
        // Closing the settings window only hides it; the overlay keeps running.
        (SETTINGS_LABEL, WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = window.hide();
        }
        _ => {}
    }
}
