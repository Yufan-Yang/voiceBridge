//! Application wiring: builds every service and connects it to Tauri.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio::sync::mpsc;

use crate::audio::capture::CpalAudioSource;
use crate::audio::AudioSource;
use crate::config::{self, ConfigStore, JsonFileStore, Settings};
use crate::error::AppError;
use crate::events::{AppEvent, EventSink};
use crate::history::HistoryStore;
use crate::injection::GenericClipboardAdapter;
use crate::pipeline::{Pipeline, PipelineDeps};
use crate::platform::{self, PermissionManager, ShortcutAction, WindowManager};
use crate::providers::ProviderHub;
use crate::shortcuts::{self, TauriShortcuts};
use crate::target::TargetRegistry;
use crate::types::{OutputKind, Phase};
use crate::vocab::VocabStore;
use crate::{logging, overlay};

const MONITOR_INTERVAL: Duration = Duration::from_millis(1500);
const ESCAPE: &str = "Escape";

pub struct AppState {
    pub pipeline: Arc<Pipeline>,
    pub registry: Arc<TargetRegistry>,
    pub vocab: Arc<VocabStore>,
    pub settings: Arc<RwLock<Settings>>,
    pub store: Arc<dyn ConfigStore>,
    pub history: Arc<HistoryStore>,
    pub hub: Arc<ProviderHub>,
    pub windows: Arc<dyn WindowManager>,
    pub permissions: Arc<dyn PermissionManager>,
    pub audio: Arc<dyn AudioSource>,
    pub injector: Arc<GenericClipboardAdapter>,
    pub shortcuts: Arc<TauriShortcuts>,
    pub data_dir: PathBuf,
    /// (x, y, dirty) — last overlay position in physical pixels.
    pub overlay_position: Mutex<Option<(i32, i32, bool)>>,
}

impl AppState {
    /// Called when the application exits while tasks may still be active.
    pub fn shutdown(&self) {
        self.pipeline.cancel();
        self.audio.abort();
        let hub = self.hub.clone();
        tauri::async_runtime::block_on(async move { hub.stop_all().await });
        crate::audio::temp::cleanup_stale(self.hub.temp_dir());
    }

    /// Starts the configured model runtimes in the background so the first
    /// utterance does not pay the model-load time. Mock providers are no-ops;
    /// unconfigured providers fail quietly and report through health checks.
    pub fn warm_up_providers(&self) {
        let providers = self.pipeline.providers();
        tauri::async_runtime::spawn(async move {
            let (asr, prompt) = tokio::join!(providers.asr.start(), providers.compiler.start());
            for (name, outcome) in [("asr", asr), ("prompt", prompt)] {
                if let Err(e) = outcome {
                    logging::event("-", "warm_up", &format!("{name} {:?}", e.code));
                }
            }
        });
    }

    pub fn apply_shortcuts(&self) -> Vec<(String, AppError)> {
        let settings = self.settings.read().unwrap().shortcuts.clone();
        let failures = shortcuts::register_all(self.shortcuts.as_ref(), &settings);
        for (accel, e) in &failures {
            logging::event("-", "shortcut", &format!("failed {accel}: {}", e.details));
        }
        failures
    }
}

/// Forwards backend events to the webviews, and keeps a plain Escape
/// shortcut registered only while an utterance is in progress so it never
/// interferes with editors otherwise.
struct TauriEventSink {
    app: AppHandle,
    escape_registered: AtomicBool,
}

impl TauriEventSink {
    fn sync_escape(&self, phase: Phase) {
        let wanted = phase.is_processing();
        if self.escape_registered.swap(wanted, Ordering::SeqCst) == wanted {
            return;
        }
        let shortcuts = self.app.global_shortcut();
        if wanted {
            let _ = shortcuts.on_shortcut(ESCAPE, |app, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(state) = app.try_state::<AppState>() {
                            state.pipeline.cancel();
                        }
                    });
                }
            });
        } else {
            let _ = shortcuts.unregister(ESCAPE);
        }
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: AppEvent) {
        let name = event.name();
        let _ = match &event {
            AppEvent::StateChanged(s) => self.app.emit(name, s),
            AppEvent::RecordingLevel(l) => self.app.emit(name, l),
            AppEvent::InterimTranscript(t) => self.app.emit(name, t),
            AppEvent::TargetsChanged(t) => self.app.emit(name, t),
            AppEvent::ProviderHealthChanged(h) => self.app.emit(name, h),
            AppEvent::UtteranceUpdated(u) => self.app.emit(name, u),
            AppEvent::InjectionResult(r) => self.app.emit(name, r),
            AppEvent::PermissionRequired(p) => self.app.emit(name, p),
            AppEvent::FatalError(e) => self.app.emit(name, e),
        };
        if let AppEvent::StateChanged(s) = &event {
            self.sync_escape(s.phase);
        }
    }
}

fn funasr_script(app: &AppHandle) -> PathBuf {
    let bundled = app
        .path()
        .resolve(
            "_up_/sidecars/funasr_sidecar.py",
            tauri::path::BaseDirectory::Resource,
        )
        .ok()
        .filter(|p| p.is_file());
    bundled.unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sidecars/funasr_sidecar.py")
    })
}

/// Runtimes and models inside the app bundle, or `src-tauri/bundled` when
/// running from the source tree.
fn bundled_assets(app: &AppHandle) -> crate::providers::BundledAssets {
    use crate::providers::BundledAssets;
    let in_bundle = app
        .path()
        .resolve("bundled", tauri::path::BaseDirectory::Resource)
        .ok()
        .map(|dir| BundledAssets::discover(&dir))
        .filter(|found| *found != BundledAssets::default());
    in_bundle.unwrap_or_else(|| {
        BundledAssets::discover(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bundled"))
    })
}

enum Ptt {
    Press,
    Release,
    /// Tap mode: start when idle, stop when recording.
    Toggle,
}

fn handle_shortcut(
    app: &AppHandle,
    ptt: &mpsc::UnboundedSender<Ptt>,
    action: ShortcutAction,
    pressed: bool,
) {
    if action == ShortcutAction::PushToTalk {
        // Press and release go through one queue so they are handled in order.
        let tap = app.try_state::<AppState>().is_some_and(|s| {
            s.settings.read().unwrap().behavior.talk_mode == config::TalkMode::Tap
        });
        let event = match (tap, pressed) {
            (true, true) => Ptt::Toggle,
            (true, false) => return, // key-up means nothing in tap mode
            (false, true) => Ptt::Press,
            (false, false) => Ptt::Release,
        };
        let _ = ptt.send(event);
        return;
    }
    if !pressed {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let (default_output, auto_submit) = {
            let s = state.settings.read().unwrap();
            (s.behavior.default_output, s.behavior.auto_submit)
        };
        match action {
            ShortcutAction::Cancel => {
                state.pipeline.cancel();
            }
            ShortcutAction::SelectSlot(slot) => {
                let _ = state.registry.select_slot(slot);
                state.pipeline.emit_targets();
            }
            ShortcutAction::BindSlot(slot) => {
                match state
                    .registry
                    .bind_active(slot, default_output, auto_submit)
                {
                    Ok(_) => state.pipeline.emit_targets(),
                    Err(e) => {
                        let _ = app.emit("fatal_error", &e);
                    }
                }
            }
            ShortcutAction::PrevTarget => {
                state.registry.select_relative(-1);
                state.pipeline.emit_targets();
            }
            ShortcutAction::NextTarget => {
                state.registry.select_relative(1);
                state.pipeline.emit_targets();
            }
            ShortcutAction::InjectLastRaw => {
                let _ = state.pipeline.inject(None, Some(OutputKind::Raw)).await;
            }
            ShortcutAction::InjectLastPrompt => {
                let _ = state.pipeline.inject(None, Some(OutputKind::Prompt)).await;
            }
            ShortcutAction::PushToTalk => {}
        }
    });
}

pub fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let paths = app.path();
    let config_dir = paths.app_config_dir()?;
    let data_dir = paths.app_data_dir()?;
    let log_dir = paths.app_log_dir()?;
    let temp_dir = paths.app_cache_dir()?.join("tmp");
    for dir in [&config_dir, &data_dir, &log_dir, &temp_dir] {
        let _ = std::fs::create_dir_all(dir);
    }
    logging::init(&log_dir);
    // Remove audio left behind by a previous crash.
    let removed = crate::audio::temp::cleanup_stale(&temp_dir);
    logging::event(
        "-",
        "startup",
        &format!("os={} stale_temp_files={removed}", platform::CURRENT_OS),
    );

    let store: Arc<dyn ConfigStore> = Arc::new(JsonFileStore::new(config_dir));
    let first_run = !matches!(store.load(config::SETTINGS_KEY), Ok(Some(_)));
    let loaded = config::load_settings(store.as_ref());
    let overlay_saved = loaded.overlay.x.zip(loaded.overlay.y);
    let save_history = loaded.behavior.save_history;
    let settings = Arc::new(RwLock::new(loaded));

    let native = platform::native();
    let registry = Arc::new(TargetRegistry::load(native.windows.clone(), store.clone()));
    let vocab = Arc::new(VocabStore::default());
    for target in registry.list() {
        vocab.refresh(&target);
    }
    let history = Arc::new(HistoryStore::new(store.clone()));
    if save_history {
        history.load_persisted();
    }
    let bundled = bundled_assets(app);
    if first_run && bundled.is_complete() {
        // All-in-one build, first launch: use the bundled models instead of
        // the mock providers. Paths stay empty, which means "bundled".
        let snapshot = {
            let mut s = settings.write().unwrap();
            s.models.asr_provider = config::AsrProviderKind::WhisperCpp;
            s.models.asr_model_id = config::defaults::DEFAULT_WHISPER_MODEL_ID.to_string();
            s.models.prompt_provider = config::PromptProviderKind::LlamaCpp;
            s.clone()
        };
        let _ = config::save_settings(store.as_ref(), &snapshot);
    }
    logging::event(
        "-",
        "startup",
        &format!("bundled_models={}", bundled.is_complete()),
    );
    let hub =
        Arc::new(ProviderHub::new(log_dir, temp_dir, funasr_script(app)).with_bundled(bundled));
    hub.reap_orphans();
    let providers = hub.build(&settings.read().unwrap().models);
    let injector = Arc::new(GenericClipboardAdapter::new(
        native.windows.clone(),
        native.clipboard.clone(),
        native.keys.clone(),
        native.permissions.clone(),
    ));
    let dev_wav = std::env::var_os("VOICEBRIDGE_DEV_WAV").filter(|_| cfg!(debug_assertions));
    let audio: Arc<dyn AudioSource> = match dev_wav {
        // Debug builds only: replay a WAV file instead of the microphone.
        Some(path) => Arc::new(crate::audio::WavFileSource::new(PathBuf::from(path))),
        None => Arc::new(CpalAudioSource::new()),
    };
    let events: Arc<dyn EventSink> = Arc::new(TauriEventSink {
        app: app.clone(),
        escape_registered: AtomicBool::new(false),
    });

    let pipeline = Pipeline::new(PipelineDeps {
        providers,
        injector: injector.clone(),
        audio: audio.clone(),
        registry: registry.clone(),
        vocab: vocab.clone(),
        events,
        settings: settings.clone(),
        history: history.clone(),
        permissions: native.permissions.clone(),
        data_dir: Some(data_dir.clone()),
    });

    // Push-to-Talk worker: strictly ordered press / release handling.
    let (ptt_tx, mut ptt_rx) = mpsc::unbounded_channel::<Ptt>();
    {
        let pipeline = pipeline.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = ptt_rx.recv().await {
                let recording = pipeline.phase() == Phase::Listening;
                let start = match event {
                    Ptt::Press => true,
                    Ptt::Release => false,
                    Ptt::Toggle => !recording,
                };
                if start {
                    let _ = pipeline.ptt_press();
                } else if let Some(job) = pipeline.begin_release() {
                    let pipeline = pipeline.clone();
                    tokio::spawn(async move { pipeline.process(job).await });
                }
            }
        });
    }

    let shortcut_app = app.clone();
    let shortcut_manager = Arc::new(TauriShortcuts {
        app: app.clone(),
        handler: Arc::new(move |action, pressed| {
            handle_shortcut(&shortcut_app, &ptt_tx, action, pressed)
        }),
    });

    let state = AppState {
        pipeline: pipeline.clone(),
        registry: registry.clone(),
        vocab,
        settings: settings.clone(),
        store: store.clone(),
        history,
        hub,
        windows: native.windows,
        permissions: native.permissions,
        audio,
        injector,
        shortcuts: shortcut_manager,
        data_dir,
        overlay_position: Mutex::new(None),
    };
    state.apply_shortcuts();
    state.warm_up_providers();
    app.manage(state);

    overlay::init(app, overlay_saved);

    // Quit cleanly on SIGTERM / SIGINT so model processes are stopped too.
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let signal_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let (Ok(mut term), Ok(mut int)) = (
                signal(SignalKind::terminate()),
                signal(SignalKind::interrupt()),
            ) else {
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
            signal_app.exit(0);
        });
    }

    // Background monitor: target liveness, overlay position, display changes.
    let monitor_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(MONITOR_INTERVAL);
        loop {
            interval.tick().await;
            let Some(state) = monitor_app.try_state::<AppState>() else {
                continue;
            };
            let registry = state.registry.clone();
            let prune = state.settings.read().unwrap().behavior.follow_focus;
            let changed = tokio::task::spawn_blocking(move || registry.refresh_status(prune))
                .await
                .unwrap_or(false);
            if changed {
                state.pipeline.emit_targets();
            }
            overlay::ensure_visible(&monitor_app);
            let dirty = {
                let mut pos = state.overlay_position.lock().unwrap();
                match pos.as_mut() {
                    Some((x, y, dirty)) if *dirty => {
                        *dirty = false;
                        Some((*x, *y))
                    }
                    _ => None,
                }
            };
            if let Some((x, y)) = dirty {
                let snapshot = {
                    let mut s = state.settings.write().unwrap();
                    s.overlay.x = Some(x);
                    s.overlay.y = Some(y);
                    s.clone()
                };
                let _ = config::save_settings(state.store.as_ref(), &snapshot);
            }
        }
    });

    Ok(())
}
