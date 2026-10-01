//! Live test of the real macOS platform layer: pins a TextEdit window and
//! injects text into it. `#[ignore]`d because it needs a desktop session and
//! Accessibility permission for the process running the tests (your
//! terminal). Run with `scripts/live-tests.sh desktop`.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use voicebridge_lib::error::ErrorCode;
use voicebridge_lib::injection::{GenericClipboardAdapter, InjectionOptions, InputInjector};
use voicebridge_lib::platform::{self, WindowInfo};
use voicebridge_lib::target::TargetSlot;
use voicebridge_lib::types::{OutputKind, PermissionKind, PermissionStatus};

fn focused_text(pid: u32) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/ax_focused_text.swift");
    let out = Command::new("swift")
        .arg(script)
        .arg(pid.to_string())
        .output()
        .expect("run swift helper");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn app_running(name: &str) -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-x", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn wait_for_window(windows: &dyn platform::WindowManager, marker: &str) -> WindowInfo {
    for _ in 0..100 {
        if let Some(w) = windows
            .list_windows()
            .unwrap()
            .into_iter()
            .find(|w| w.title.contains(marker))
        {
            return w;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("TextEdit window with title containing {marker} did not appear");
}

fn open_app(args: &[&str]) {
    assert!(Command::new("/usr/bin/open")
        .args(args)
        .status()
        .unwrap()
        .success());
}

#[tokio::test]
#[ignore = "needs a desktop session and Accessibility permission"]
async fn injects_into_a_real_window_with_verification() {
    if std::env::var("VB_LIVE_DESKTOP").is_err() {
        eprintln!("SKIPPED: VB_LIVE_DESKTOP is not set");
        return;
    }
    let native = platform::native();
    assert_eq!(
        native.permissions.status(PermissionKind::Accessibility),
        PermissionStatus::Granted,
        "grant Accessibility to the terminal running this test"
    );

    let textedit_was_running = app_running("TextEdit");
    let dir = tempfile::tempdir().unwrap();
    let marker = format!(
        "vb-live-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let file = dir.path().join(format!("{marker}.txt"));
    std::fs::write(&file, "").unwrap();
    open_app(&["-a", "TextEdit", file.to_str().unwrap()]);

    let window = wait_for_window(native.windows.as_ref(), &marker).await;
    eprintln!("pinned window: {window:?}");
    assert!(Path::new(&window.executable_or_bundle_id).ends_with("TextEdit"));
    let target = TargetSlot::from_window(&window, 1, OutputKind::Prompt);
    assert!(!target.auto_submit);

    // The lookup by id returns the same identity.
    let again = native
        .windows
        .window_info(&window.platform_window_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (again.process_id, &again.executable_or_bundle_id),
        (window.process_id, &window.executable_or_bundle_id)
    );

    // Put another application in front so activation is really exercised.
    open_app(&["-a", "Finder"]);
    tokio::time::sleep(Duration::from_millis(800)).await;
    let before = native.windows.foreground_window().unwrap();
    eprintln!(
        "foreground before injection: {:?}",
        before.as_ref().map(|w| &w.app_name)
    );
    assert_ne!(
        before.map(|w| w.platform_window_id),
        Some(window.platform_window_id.clone())
    );

    let adapter = GenericClipboardAdapter::new(
        native.windows.clone(),
        native.clipboard.clone(),
        native.keys.clone(),
        native.permissions.clone(),
    );
    let sentinel = format!("user clipboard {marker}");
    native.clipboard.write_text(&sentinel).unwrap();

    // 1. Paste a multi-line prompt; Enter must not be pressed.
    let prompt = format!("Update `useUserQuery` ({marker}):\n- accept `userId`\n- 保留返回结构 ✓");
    let result = adapter
        .inject(&target, &prompt, InjectionOptions::default())
        .await
        .expect("inject");
    eprintln!("injection result: {result:?}");
    assert!(!result.submitted);
    assert!(result.clipboard_restored);
    let fg = native
        .windows
        .foreground_window()
        .unwrap()
        .expect("foreground window");
    assert_eq!(
        fg.platform_window_id, window.platform_window_id,
        "target must be in front after injection"
    );
    assert_eq!(
        native.clipboard.read_text().unwrap().as_deref(),
        Some(sentinel.as_str()),
        "clipboard restored"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        focused_text(window.process_id),
        prompt,
        "pasted text must match exactly, with no trailing newline"
    );

    // 2. A target whose identity no longer matches is refused before anything happens.
    let mut impostor = target.clone();
    impostor.process_id += 1;
    let err = adapter
        .inject(&impostor, "MUST NOT APPEAR", InjectionOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TargetIdentityMismatch);
    let mut wrong_app = target.clone();
    wrong_app.executable_or_bundle_id = "/Applications/Other.app/Contents/MacOS/Other".into();
    let err = adapter
        .inject(&wrong_app, "MUST NOT APPEAR", InjectionOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TargetIdentityMismatch);
    assert_eq!(focused_text(window.process_id), prompt);
    assert_eq!(
        native.clipboard.read_text().unwrap().as_deref(),
        Some(sentinel.as_str())
    );

    // 3. Enter is pressed only with auto_submit.
    let options = InjectionOptions {
        auto_submit: true,
        ..Default::default()
    };
    let result = adapter.inject(&target, " SUBMIT", options).await.unwrap();
    assert!(result.submitted);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        focused_text(window.process_id),
        format!("{prompt} SUBMIT\n")
    );

    // 4. A closed window is reported, and nothing is pasted elsewhere.
    if textedit_was_running {
        eprintln!("TextEdit was already running: leaving the test document open (close it without saving).");
    } else {
        let _ = Command::new("/usr/bin/pkill")
            .args(["-x", "TextEdit"])
            .status();
        for _ in 0..50 {
            if native
                .windows
                .window_info(&window.platform_window_id)
                .unwrap()
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let err = adapter
            .inject(&target, "MUST NOT APPEAR", InjectionOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::TargetNotFound);
        assert_eq!(
            native.clipboard.read_text().unwrap().as_deref(),
            Some(sentinel.as_str())
        );
    }
}

#[test]
#[ignore = "needs a microphone and Microphone permission"]
fn records_from_the_real_microphone() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use voicebridge_lib::audio::capture::CpalAudioSource;
    use voicebridge_lib::audio::{AudioSource, CaptureOptions};

    if std::env::var("VB_LIVE_DESKTOP").is_err() {
        eprintln!("SKIPPED: VB_LIVE_DESKTOP is not set");
        return;
    }
    let native = platform::native();
    assert_eq!(
        native.permissions.status(PermissionKind::Microphone),
        PermissionStatus::Granted,
        "grant Microphone access to the terminal running this test"
    );
    let source = CpalAudioSource::new();
    let devices = source.list_devices();
    eprintln!("input devices: {devices:?}");
    assert!(!devices.is_empty());

    let levels = Arc::new(AtomicU32::new(0));
    let callback = |counter: Arc<AtomicU32>| -> voicebridge_lib::audio::LevelCallback {
        Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    };

    // Prefer the system default input. A machine without a physical
    // microphone (only virtual devices) reports MIC_DEVICE_NOT_FOUND for the
    // default; the capture path is then exercised through a named device.
    let mut chosen: Option<String> = None;
    let default_options = CaptureOptions {
        device: None,
        max_secs: 5,
    };
    if let Err(e) = source.start(default_options, callback(levels.clone())) {
        assert_eq!(
            e.code,
            ErrorCode::MicDeviceNotFound,
            "unexpected error: {e:?}"
        );
        eprintln!("NOTE: no usable default input device; trying named devices");
        chosen = devices.iter().find_map(|name| {
            let options = CaptureOptions {
                device: Some(name.clone()),
                max_secs: 5,
            };
            source
                .start(options, callback(levels.clone()))
                .ok()
                .map(|_| name.clone())
        });
        assert!(chosen.is_some(), "no input device could be opened");
    }
    eprintln!(
        "recording from: {}",
        chosen.as_deref().unwrap_or("system default")
    );
    let options = || CaptureOptions {
        device: chosen.clone(),
        max_secs: 5,
    };

    // Only one recording at a time.
    assert_eq!(
        source.start(options(), Arc::new(|_| {})).unwrap_err().code,
        ErrorCode::Busy
    );
    std::thread::sleep(Duration::from_millis(1200));
    let audio = source.stop().expect("stop capture");
    eprintln!(
        "captured {audio:?}, level callbacks: {}",
        levels.load(Ordering::SeqCst)
    );
    assert!(audio.sample_rate >= 8000);
    assert!(
        (900..=2500).contains(&audio.duration_ms()),
        "captured {} ms",
        audio.duration_ms()
    );
    assert!(
        levels.load(Ordering::SeqCst) >= 5,
        "input level should be reported while recording"
    );

    // Aborting discards the recording.
    source
        .start(options(), Arc::new(|_| {}))
        .expect("restart capture");
    std::thread::sleep(Duration::from_millis(300));
    source.abort();
    assert!(source.stop().is_err(), "abort discards the recording");
}
