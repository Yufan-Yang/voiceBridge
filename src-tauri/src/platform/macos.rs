//! macOS platform layer.
//!
//! Uses the CoreGraphics window list for enumeration, the Accessibility API
//! for titles / focus / activation, NSPasteboard for the clipboard and
//! CGEvent for key simulation. Everything is called through plain C FFI and
//! `objc_msgSend`, so there is no dependency on Objective-C binding crates.

#![allow(
    non_upper_case_globals,
    non_snake_case,
    clippy::missing_safety_doc,
    clippy::manual_dangling_ptr
)]

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::Arc;

use core_foundation::base::TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{
    CFDictionaryGetTypeID, CFDictionaryGetValue, CFDictionaryRef,
};
use core_foundation_sys::number::{
    kCFBooleanFalse, kCFBooleanTrue, kCFNumberFloat64Type, kCFNumberSInt64Type, CFNumberGetTypeID,
    CFNumberGetValue, CFNumberRef,
};
use core_foundation_sys::string::{CFStringGetTypeID, CFStringRef};

use super::*;
use crate::error::{AppError, ErrorCode, Result};

type AXRef = CFTypeRef;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXRef;
    fn AXUIElementCreateSystemWide() -> AXRef;
    fn AXUIElementCopyAttributeValue(el: AXRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: AXRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementPerformAction(el: AXRef, action: CFStringRef) -> i32;
    fn AXUIElementGetPid(el: AXRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(el: AXRef, seconds: f32) -> i32;
    // Private but long-stable: maps an accessibility window to its CGWindowID.
    fn _AXUIElementGetWindow(el: AXRef, out: *mut u32) -> i32;

    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    fn CGEventCreateKeyboardEvent(source: *const c_void, keycode: u16, down: bool) -> *mut c_void;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventPost(tap: u32, event: *mut c_void);
}

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *mut c_void;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

extern "C" {
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
}

// ---------------------------------------------------------------------------
// Minimal Objective-C runtime helpers
// ---------------------------------------------------------------------------

pub mod objc {
    use super::*;

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        pub fn objc_getClass(name: *const c_char) -> *mut c_void;
        pub fn sel_registerName(name: *const c_char) -> *mut c_void;
        pub fn objc_msgSend();
        pub fn objc_autoreleasePoolPush() -> *mut c_void;
        pub fn objc_autoreleasePoolPop(pool: *mut c_void);
        pub fn object_setClass(obj: *mut c_void, cls: *mut c_void) -> *mut c_void;
        pub fn objc_allocateClassPair(
            superclass: *mut c_void,
            name: *const c_char,
            extra: usize,
        ) -> *mut c_void;
        pub fn objc_registerClassPair(cls: *mut c_void);
        pub fn class_addMethod(
            cls: *mut c_void,
            sel: *mut c_void,
            imp: *const c_void,
            types: *const c_char,
        ) -> bool;
    }

    pub unsafe fn class(name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap();
        objc_getClass(c.as_ptr())
    }

    pub unsafe fn sel(name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap();
        sel_registerName(c.as_ptr())
    }

    type Send0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type Send1 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type Send2 =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> *mut c_void;

    /// `[obj sel]` returning a pointer-sized value.
    pub unsafe fn send0(obj: *mut c_void, s: &str) -> *mut c_void {
        let f: Send0 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(s))
    }

    /// `[obj sel:a]` with a pointer-sized argument.
    pub unsafe fn send1(obj: *mut c_void, s: &str, a: *mut c_void) -> *mut c_void {
        let f: Send1 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(s), a)
    }

    pub unsafe fn send2(obj: *mut c_void, s: &str, a: *mut c_void, b: *mut c_void) -> *mut c_void {
        let f: Send2 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel(s), a, b)
    }

    pub unsafe fn nsstring(s: &str) -> *mut c_void {
        let c = CString::new(s.replace('\0', "")).unwrap();
        send1(
            class("NSString"),
            "stringWithUTF8String:",
            c.as_ptr() as *mut c_void,
        )
    }

    /// Runs `f` inside an autorelease pool.
    pub unsafe fn with_pool<T>(f: impl FnOnce() -> T) -> T {
        let pool = objc_autoreleasePoolPush();
        let out = f();
        objc_autoreleasePoolPop(pool);
        out
    }

    /// Objective-C BOOL results arrive in the low byte of the return register.
    pub fn as_bool(v: *mut c_void) -> bool {
        (v as usize & 0xff) != 0
    }
}

// ---------------------------------------------------------------------------
// CoreFoundation helpers
// ---------------------------------------------------------------------------

unsafe fn dict_value(d: CFDictionaryRef, key: &str) -> *const c_void {
    let k = CFString::new(key);
    CFDictionaryGetValue(d, k.as_concrete_TypeRef() as *const c_void)
}

unsafe fn dict_i64(d: CFDictionaryRef, key: &str) -> Option<i64> {
    let v = dict_value(d, key);
    if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
        return None;
    }
    let mut out: i64 = 0;
    CFNumberGetValue(
        v as CFNumberRef,
        kCFNumberSInt64Type,
        &mut out as *mut i64 as *mut c_void,
    );
    Some(out)
}

unsafe fn dict_f64(d: CFDictionaryRef, key: &str) -> Option<f64> {
    let v = dict_value(d, key);
    if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
        return None;
    }
    let mut out: f64 = 0.0;
    CFNumberGetValue(
        v as CFNumberRef,
        kCFNumberFloat64Type,
        &mut out as *mut f64 as *mut c_void,
    );
    Some(out)
}

unsafe fn cf_to_string(v: CFTypeRef) -> Option<String> {
    if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
        return None;
    }
    Some(CFString::wrap_under_get_rule(v as CFStringRef).to_string())
}

unsafe fn dict_string(d: CFDictionaryRef, key: &str) -> Option<String> {
    cf_to_string(dict_value(d, key))
}

// ---------------------------------------------------------------------------
// Window enumeration
// ---------------------------------------------------------------------------

const CG_ON_SCREEN_ONLY: u32 = 1;
const CG_EXCLUDE_DESKTOP: u32 = 16;

struct CgWindow {
    id: u32,
    pid: i32,
    owner: String,
    name: String,
}

fn cg_windows(on_screen_only: bool) -> Vec<CgWindow> {
    let mut out = Vec::new();
    let option = CG_EXCLUDE_DESKTOP | if on_screen_only { CG_ON_SCREEN_ONLY } else { 0 };
    unsafe {
        let arr = CGWindowListCopyWindowInfo(option, 0);
        if arr.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(arr) {
            let d = CFArrayGetValueAtIndex(arr, i) as CFDictionaryRef;
            if d.is_null() || dict_i64(d, "kCGWindowLayer") != Some(0) {
                continue; // only normal-level application windows
            }
            let (Some(id), Some(pid)) = (
                dict_i64(d, "kCGWindowNumber"),
                dict_i64(d, "kCGWindowOwnerPID"),
            ) else {
                continue;
            };
            let bounds = dict_value(d, "kCGWindowBounds");
            if !bounds.is_null() && CFGetTypeID(bounds) == CFDictionaryGetTypeID() {
                let b = bounds as CFDictionaryRef;
                let w = dict_f64(b, "Width").unwrap_or(0.0);
                let h = dict_f64(b, "Height").unwrap_or(0.0);
                if w < 80.0 || h < 60.0 {
                    continue; // helper / status windows
                }
            }
            out.push(CgWindow {
                id: id as u32,
                pid: pid as i32,
                owner: dict_string(d, "kCGWindowOwnerName").unwrap_or_default(),
                name: dict_string(d, "kCGWindowName").unwrap_or_default(),
            });
        }
        CFRelease(arr as CFTypeRef);
    }
    out
}

fn executable_path(pid: i32) -> String {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buf[..n as usize]).to_string()
}

unsafe fn ax_copy(el: AXRef, attr: &str) -> Option<CFTypeRef> {
    let a = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(el, a.as_concrete_TypeRef(), &mut value);
    if err == 0 && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

unsafe fn ax_app(pid: i32) -> AXRef {
    let app = AXUIElementCreateApplication(pid);
    if !app.is_null() {
        AXUIElementSetMessagingTimeout(app, 0.6);
    }
    app
}

/// Calls `f(window_element, cg_window_id)` for each accessibility window of `pid`.
unsafe fn ax_each_window(pid: i32, mut f: impl FnMut(AXRef, u32)) {
    let app = ax_app(pid);
    if app.is_null() {
        return;
    }
    if let Some(arr) = ax_copy(app, "AXWindows") {
        let arr_ref = arr as CFArrayRef;
        for i in 0..CFArrayGetCount(arr_ref) {
            let w = CFArrayGetValueAtIndex(arr_ref, i) as AXRef;
            let mut id: u32 = 0;
            if !w.is_null() && _AXUIElementGetWindow(w, &mut id) == 0 {
                f(w, id);
            }
        }
        CFRelease(arr);
    }
    CFRelease(app);
}

fn ax_titles(pid: i32) -> HashMap<u32, String> {
    let mut titles = HashMap::new();
    if !unsafe { AXIsProcessTrusted() } {
        return titles;
    }
    unsafe {
        ax_each_window(pid, |w, id| {
            if let Some(t) = ax_copy(w, "AXTitle") {
                if let Some(s) = cf_to_string(t) {
                    titles.insert(id, s);
                }
                CFRelease(t);
            }
        });
    }
    titles
}

fn ax_focused_pid() -> Option<i32> {
    unsafe {
        if !AXIsProcessTrusted() {
            return None;
        }
        let sys = AXUIElementCreateSystemWide();
        if sys.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(sys, 0.6);
        let mut out = None;
        if let Some(app) = ax_copy(sys, "AXFocusedApplication") {
            let mut pid: i32 = 0;
            if AXUIElementGetPid(app, &mut pid) == 0 {
                out = Some(pid);
            }
            CFRelease(app);
        }
        CFRelease(sys);
        out
    }
}

fn ax_focused_window(pid: i32) -> Option<u32> {
    unsafe {
        if !AXIsProcessTrusted() {
            return None;
        }
        let app = ax_app(pid);
        if app.is_null() {
            return None;
        }
        let mut out = None;
        if let Some(w) = ax_copy(app, "AXFocusedWindow") {
            let mut id: u32 = 0;
            if _AXUIElementGetWindow(w, &mut id) == 0 {
                out = Some(id);
            }
            CFRelease(w);
        }
        CFRelease(app);
        out
    }
}

fn to_info(w: &CgWindow, titles: &HashMap<u32, String>) -> WindowInfo {
    let title = titles
        .get(&w.id)
        .cloned()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| w.name.clone());
    WindowInfo {
        platform_window_id: w.id.to_string(),
        process_id: w.pid as u32,
        executable_or_bundle_id: executable_path(w.pid),
        app_name: w.owner.clone(),
        title,
    }
}

pub struct MacWindowManager;

impl WindowManager for MacWindowManager {
    fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        let own = std::process::id() as i32;
        let wins: Vec<CgWindow> = cg_windows(true)
            .into_iter()
            .filter(|w| w.pid != own)
            .collect();
        let mut titles: HashMap<i32, HashMap<u32, String>> = HashMap::new();
        for w in &wins {
            titles.entry(w.pid).or_insert_with(|| ax_titles(w.pid));
        }
        Ok(wins.iter().map(|w| to_info(w, &titles[&w.pid])).collect())
    }

    fn foreground_window(&self) -> Result<Option<WindowInfo>> {
        // Front-to-back order: the first normal-level window is frontmost.
        let wins = cg_windows(true);
        let pid = match ax_focused_pid().or_else(|| wins.first().map(|w| w.pid)) {
            Some(pid) => pid,
            None => return Ok(None),
        };
        let focused = ax_focused_window(pid);
        let win = match focused {
            Some(id) => wins.iter().find(|w| w.id == id),
            None => wins.iter().find(|w| w.pid == pid),
        };
        Ok(win.map(|w| to_info(w, &ax_titles(pid))))
    }

    fn window_info(&self, platform_window_id: &str) -> Result<Option<WindowInfo>> {
        let id: u32 = platform_window_id.parse().map_err(|_| {
            AppError::new(ErrorCode::TargetNotFound).with_details("invalid window handle")
        })?;
        // Include off-screen windows so minimized targets stay "online".
        let wins = cg_windows(false);
        Ok(wins
            .iter()
            .find(|w| w.id == id)
            .map(|w| to_info(w, &ax_titles(w.pid))))
    }

    fn activate(&self, window: &WindowInfo) -> Result<()> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err(AppError::new(ErrorCode::AccessibilityPermissionDenied));
        }
        let id: u32 = window.platform_window_id.parse().map_err(|_| {
            AppError::new(ErrorCode::TargetNotFound).with_details("invalid window handle")
        })?;
        let pid = window.process_id as i32;
        let mut raised = false;
        unsafe {
            ax_each_window(pid, |w, wid| {
                if wid != id || raised {
                    return;
                }
                let minimized = CFString::new("AXMinimized");
                AXUIElementSetAttributeValue(
                    w,
                    minimized.as_concrete_TypeRef(),
                    kCFBooleanFalse as CFTypeRef,
                );
                let raise = CFString::new("AXRaise");
                let ok = AXUIElementPerformAction(w, raise.as_concrete_TypeRef()) == 0;
                let main = CFString::new("AXMain");
                AXUIElementSetAttributeValue(
                    w,
                    main.as_concrete_TypeRef(),
                    kCFBooleanTrue as CFTypeRef,
                );
                raised = ok;
            });
            if !raised {
                return Err(AppError::new(ErrorCode::TargetActivationFailed).with_details(
                    "window not reachable through the accessibility API (other Space or closed)",
                ));
            }
            let app = ax_app(pid);
            if !app.is_null() {
                let front = CFString::new("AXFrontmost");
                AXUIElementSetAttributeValue(
                    app,
                    front.as_concrete_TypeRef(),
                    kCFBooleanTrue as CFTypeRef,
                );
                CFRelease(app);
            }
            // Also ask AppKit; harmless when the app is already frontmost.
            objc::with_pool(|| {
                let cls = objc::class("NSRunningApplication");
                if cls.is_null() {
                    return;
                }
                let running = objc::send1(
                    cls,
                    "runningApplicationWithProcessIdentifier:",
                    pid as usize as *mut c_void,
                );
                if !running.is_null() {
                    // NSApplicationActivateIgnoringOtherApps
                    objc::send1(running, "activateWithOptions:", 2usize as *mut c_void);
                }
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

const PASTEBOARD_STRING_TYPE: &str = "public.utf8-plain-text";

pub struct MacClipboard;

unsafe fn pasteboard() -> Result<*mut c_void> {
    let pb = objc::send0(objc::class("NSPasteboard"), "generalPasteboard");
    if pb.is_null() {
        Err(AppError::new(ErrorCode::ClipboardFailed).with_details("no general pasteboard"))
    } else {
        Ok(pb)
    }
}

impl ClipboardManager for MacClipboard {
    fn read_text(&self) -> Result<Option<String>> {
        unsafe {
            objc::with_pool(|| {
                let pb = pasteboard()?;
                let s = objc::send1(pb, "stringForType:", objc::nsstring(PASTEBOARD_STRING_TYPE));
                if s.is_null() {
                    return Ok(None);
                }
                let utf8 = objc::send0(s, "UTF8String") as *const c_char;
                if utf8.is_null() {
                    return Ok(None);
                }
                Ok(Some(CStr::from_ptr(utf8).to_string_lossy().to_string()))
            })
        }
    }

    fn write_text(&self, text: &str) -> Result<()> {
        unsafe {
            objc::with_pool(|| {
                let pb = pasteboard()?;
                objc::send0(pb, "clearContents");
                let ok = objc::send2(
                    pb,
                    "setString:forType:",
                    objc::nsstring(text),
                    objc::nsstring(PASTEBOARD_STRING_TYPE),
                );
                if objc::as_bool(ok) {
                    Ok(())
                } else {
                    Err(AppError::new(ErrorCode::ClipboardFailed).with_details("setString failed"))
                }
            })
        }
    }

    fn clear(&self) -> Result<()> {
        unsafe {
            objc::with_pool(|| {
                let pb = pasteboard()?;
                objc::send0(pb, "clearContents");
                Ok(())
            })
        }
    }

    fn change_count(&self) -> Result<i64> {
        unsafe {
            objc::with_pool(|| {
                let pb = pasteboard()?;
                Ok(objc::send0(pb, "changeCount") as isize as i64)
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Key simulation
// ---------------------------------------------------------------------------

const KEY_V: u16 = 9;
const KEY_RETURN: u16 = 36;
const FLAG_COMMAND: u64 = 0x0010_0000;
const HID_EVENT_TAP: u32 = 0;

pub struct MacKeys;

fn key_tap(keycode: u16, flags: u64) -> Result<()> {
    if !unsafe { AXIsProcessTrusted() } {
        return Err(AppError::new(ErrorCode::AccessibilityPermissionDenied));
    }
    unsafe {
        for down in [true, false] {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, down);
            if ev.is_null() {
                return Err(AppError::new(ErrorCode::InputInjectionFailed)
                    .with_details("could not create keyboard event"));
            }
            // Explicit flags so modifiers still held from the shortcut are ignored.
            CGEventSetFlags(ev, flags);
            CGEventPost(HID_EVENT_TAP, ev);
            CFRelease(ev as CFTypeRef);
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
    }
    Ok(())
}

impl KeySimulator for MacKeys {
    fn paste(&self) -> Result<()> {
        key_tap(KEY_V, FLAG_COMMAND)
    }

    fn enter(&self) -> Result<()> {
        key_tap(KEY_RETURN, 0)
    }
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

pub struct MacPermissions;

fn microphone_status() -> PermissionStatus {
    unsafe {
        let cls = objc::class("AVCaptureDevice");
        if cls.is_null() {
            return PermissionStatus::NotDetermined;
        }
        let status =
            objc::send1(cls, "authorizationStatusForMediaType:", AVMediaTypeAudio) as isize;
        match status {
            3 => PermissionStatus::Granted,
            1 | 2 => PermissionStatus::Denied,
            _ => PermissionStatus::NotDetermined,
        }
    }
}

impl PermissionManager for MacPermissions {
    fn status(&self, kind: PermissionKind) -> PermissionStatus {
        match kind {
            PermissionKind::Microphone => microphone_status(),
            PermissionKind::Accessibility => {
                if unsafe { AXIsProcessTrusted() } {
                    PermissionStatus::Granted
                } else {
                    PermissionStatus::Denied
                }
            }
        }
    }

    fn request(&self, kind: PermissionKind) -> PermissionStatus {
        match kind {
            // The system shows the microphone prompt when capture first starts.
            PermissionKind::Microphone => microphone_status(),
            PermissionKind::Accessibility => unsafe {
                let key = CFString::new("AXTrustedCheckOptionPrompt");
                let dict = core_foundation::dictionary::CFDictionary::from_CFType_pairs(&[(
                    key.as_CFType(),
                    core_foundation::boolean::CFBoolean::true_value().as_CFType(),
                )]);
                if AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef()) {
                    PermissionStatus::Granted
                } else {
                    PermissionStatus::Denied
                }
            },
        }
    }

    fn open_settings(&self, kind: PermissionKind) -> Result<()> {
        let pane = match kind {
            PermissionKind::Microphone => "Privacy_Microphone",
            PermissionKind::Accessibility => "Privacy_Accessibility",
        };
        std::process::Command::new("/usr/bin/open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{pane}"
            ))
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.kind().to_string()))
    }
}

pub fn platform() -> Platform {
    Platform {
        windows: Arc::new(MacWindowManager),
        clipboard: Arc::new(MacClipboard),
        keys: Arc::new(MacKeys),
        permissions: Arc::new(MacPermissions),
    }
}

// ---------------------------------------------------------------------------
// Overlay window: non-activating panel
// ---------------------------------------------------------------------------

extern "C" fn return_no(_this: *mut c_void, _sel: *mut c_void) -> bool {
    false
}

extern "C" fn return_yes(_this: *mut c_void, _sel: *mut c_void) -> bool {
    true
}

/// Turns the overlay NSWindow into a non-activating NSPanel so that clicking
/// it never takes keyboard focus away from the coding window.
///
/// Must be called on the main thread with a valid `NSWindow*`.
pub unsafe fn make_non_activating_panel(ns_window: *mut c_void) {
    if ns_window.is_null() {
        return;
    }
    let name = CString::new("VoiceBridgeOverlayPanel").unwrap();
    let mut cls = objc::objc_getClass(name.as_ptr());
    if cls.is_null() {
        let panel = objc::class("NSPanel");
        if panel.is_null() {
            return;
        }
        cls = objc::objc_allocateClassPair(panel, name.as_ptr(), 0);
        if cls.is_null() {
            return;
        }
        let types = CString::new("B@:").unwrap();
        for s in ["canBecomeKeyWindow", "canBecomeMainWindow"] {
            objc::class_addMethod(
                cls,
                objc::sel(s),
                return_no as *const c_void,
                types.as_ptr(),
            );
        }
        // AppKit asks this when deciding whether a click activates the app.
        objc::class_addMethod(
            cls,
            objc::sel("_isNonactivatingPanel"),
            return_yes as *const c_void,
            types.as_ptr(),
        );
        objc::objc_registerClassPair(cls);
    }
    objc::object_setClass(ns_window, cls);

    const NONACTIVATING_PANEL: usize = 1 << 7;
    let mask = objc::send0(ns_window, "styleMask") as usize;
    objc::send1(
        ns_window,
        "setStyleMask:",
        (mask | NONACTIVATING_PANEL) as *mut c_void,
    );
    // NSFloatingWindowLevel
    objc::send1(ns_window, "setLevel:", 3usize as *mut c_void);
    // canJoinAllSpaces | stationary | fullScreenAuxiliary
    let behavior: usize = (1 << 0) | (1 << 4) | (1 << 8);
    objc::send1(ns_window, "setCollectionBehavior:", behavior as *mut c_void);
    objc::send1(ns_window, "setHidesOnDeactivate:", std::ptr::null_mut());
    objc::send1(ns_window, "setFloatingPanel:", 1usize as *mut c_void);

    // The non-activating style bit is only pushed to the window server when a
    // panel is created with it. This window was created as a normal window,
    // so the "prevents activation" tag has to be set explicitly.
    let prevents = objc::sel("_setPreventsActivation:");
    let responds = objc::send1(ns_window, "respondsToSelector:", prevents);
    if objc::as_bool(responds) {
        objc::send1(ns_window, "_setPreventsActivation:", 1usize as *mut c_void);
    }
}
