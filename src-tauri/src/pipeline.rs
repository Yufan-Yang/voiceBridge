//! Utterance orchestration: record → transcribe → normalize → compile →
//! inject. Owns the state machine and enforces the routing and concurrency
//! rules (frozen target, per-utterance cancellation, stale-result rejection).

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::asr::{AsrContext, AsrProvider};
use crate::audio::{self, AudioBuffer, AudioSource, CaptureOptions};
use crate::config::Settings;
use crate::error::{AppError, ErrorCode, Result};
use crate::events::{AppEvent, EventSink};
use crate::history::HistoryStore;
use crate::injection::{InjectionOptions, InputInjector};
use crate::logging;
use crate::platform::PermissionManager;
use crate::prompt::fallback::{fallback_prompt, normalize_transcript, FALLBACK_NOTICE};
use crate::prompt::{PromptCompileInput, PromptCompiler, TargetContext};
use crate::state_machine::{reduce, Event, Machine, Outcome};
use crate::target::{TargetRegistry, TargetSlot};
use crate::types::*;
use crate::vocab::VocabStore;

/// Upper bound on vocabulary terms handed to the models per utterance.
const MAX_CONTEXT_TERMS: usize = 80;
const ERROR_NOTICE_MIN_MS: u64 = 8000;

#[derive(Clone)]
pub struct Providers {
    pub asr: Arc<dyn AsrProvider>,
    pub compiler: Arc<dyn PromptCompiler>,
}

pub struct PipelineDeps {
    pub providers: Providers,
    pub injector: Arc<dyn InputInjector>,
    pub audio: Arc<dyn AudioSource>,
    pub registry: Arc<TargetRegistry>,
    pub vocab: Arc<VocabStore>,
    pub events: Arc<dyn EventSink>,
    pub settings: Arc<RwLock<Settings>>,
    pub history: Arc<HistoryStore>,
    pub permissions: Arc<dyn PermissionManager>,
    /// Where opt-in audio / transcript files go. `None` disables both.
    pub data_dir: Option<PathBuf>,
}

struct Inner {
    machine: Machine,
    /// Cancellation token of the current utterance.
    token: CancellationToken,
    /// Bumped on every transition; lets delayed resets detect newer activity.
    epoch: u64,
    error: Option<AppError>,
    notice: Option<String>,
    recording_started_ms: Option<f64>,
    last_result: Option<UtteranceResult>,
}

/// A released recording waiting to be processed.
pub struct Job {
    id: String,
    token: CancellationToken,
    audio: Option<AudioBuffer>,
}

pub struct Pipeline {
    inner: Mutex<Inner>,
    providers: RwLock<Providers>,
    injector: Arc<dyn InputInjector>,
    audio: Arc<dyn AudioSource>,
    registry: Arc<TargetRegistry>,
    vocab: Arc<VocabStore>,
    events: Arc<dyn EventSink>,
    settings: Arc<RwLock<Settings>>,
    history: Arc<HistoryStore>,
    permissions: Arc<dyn PermissionManager>,
    data_dir: Option<PathBuf>,
}

/// Runs `fut` until it finishes, the utterance is canceled, or `timeout`
/// elapses (reported as `timeout_code`).
async fn run_cancellable<T>(
    token: &CancellationToken,
    timeout: Duration,
    timeout_code: ErrorCode,
    fut: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        _ = token.cancelled() => Err(AppError::canceled()),
        r = tokio::time::timeout(timeout, fut) => match r {
            Ok(inner) => inner,
            Err(_) => Err(AppError::new(timeout_code)),
        },
    }
}

impl Pipeline {
    pub fn new(deps: PipelineDeps) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                machine: Machine::default(),
                token: CancellationToken::new(),
                epoch: 0,
                error: None,
                notice: None,
                recording_started_ms: None,
                last_result: None,
            }),
            providers: RwLock::new(deps.providers),
            injector: deps.injector,
            audio: deps.audio,
            registry: deps.registry,
            vocab: deps.vocab,
            events: deps.events,
            settings: deps.settings,
            history: deps.history,
            permissions: deps.permissions,
            data_dir: deps.data_dir,
        })
    }

    pub fn set_providers(&self, providers: Providers) {
        *self.providers.write().unwrap() = providers;
    }

    pub fn providers(&self) -> Providers {
        self.providers.read().unwrap().clone()
    }

    fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn snapshot(&self) -> AppSnapshot {
        let (phase, utterance_id, frozen, last_result, error, notice, started) = {
            let g = self.inner.lock().unwrap();
            (
                g.machine.phase,
                g.machine.utterance_id.clone(),
                g.machine.frozen_target_id.clone(),
                g.last_result.clone(),
                g.error.clone(),
                g.notice.clone(),
                g.recording_started_ms,
            )
        };
        let targets = self.registry.snapshot();
        let settings = self.settings.read().unwrap();
        AppSnapshot {
            phase,
            utterance_id,
            frozen_target_id: frozen,
            selected_target_id: targets.selected_id,
            targets: targets.targets,
            suggestions: targets.suggestions,
            last_result,
            error,
            notice,
            recording_started_ms: started,
            max_recording_secs: settings.audio.max_recording_secs,
            offline: !settings.privacy.network_inference,
            dev_mode: cfg!(debug_assertions),
        }
    }

    pub fn emit_state(&self) {
        self.events.emit(AppEvent::StateChanged(self.snapshot()));
    }

    /// Call after any change to the target registry.
    pub fn emit_targets(&self) {
        self.events
            .emit(AppEvent::TargetsChanged(self.registry.snapshot()));
        self.emit_state();
    }

    pub fn last_result(&self) -> Option<UtteranceResult> {
        self.inner.lock().unwrap().last_result.clone()
    }

    pub fn clear_last_result(&self) {
        self.inner.lock().unwrap().last_result = None;
        self.emit_state();
    }

    pub fn phase(&self) -> Phase {
        self.inner.lock().unwrap().machine.phase
    }

    fn apply(&self, event: &Event) -> Outcome {
        let mut g = self.inner.lock().unwrap();
        let (machine, outcome) = reduce(&g.machine, event);
        if outcome == Outcome::Applied {
            g.machine = machine;
            g.epoch += 1;
        }
        outcome
    }

    /// Advances the current utterance. Fails with `OPERATION_CANCELED` when
    /// the utterance was canceled or superseded, so no later stage runs.
    fn step(&self, token: &CancellationToken, event: Event) -> Result<()> {
        if token.is_cancelled() || self.apply(&event) != Outcome::Applied {
            return Err(AppError::canceled());
        }
        self.emit_state();
        Ok(())
    }

    /// Stores a result for utterance `id` and advances the machine — only if
    /// that utterance is still the current one.
    fn store_result(
        &self,
        token: &CancellationToken,
        result: &UtteranceResult,
        event: Event,
        notice: Option<String>,
    ) -> Result<()> {
        {
            let mut g = self.inner.lock().unwrap();
            if token.is_cancelled() || g.machine.utterance_id.as_deref() != Some(result.id.as_str())
            {
                return Err(AppError::canceled());
            }
            let (machine, outcome) = reduce(&g.machine, &event);
            if outcome != Outcome::Applied {
                return Err(AppError::canceled());
            }
            g.machine = machine;
            g.epoch += 1;
            g.last_result = Some(result.clone());
            g.notice = notice;
        }
        self.events.emit(AppEvent::UtteranceUpdated(result.clone()));
        self.emit_state();
        Ok(())
    }

    fn fail(self: &Arc<Self>, id: &str, error: AppError) {
        logging::event(id, "error", &format!("{:?}", error.code));
        let applied = {
            let mut g = self.inner.lock().unwrap();
            let (machine, outcome) = reduce(
                &g.machine,
                &Event::Failed {
                    utterance_id: id.to_string(),
                },
            );
            if outcome == Outcome::Applied {
                g.machine = machine;
                g.epoch += 1;
                g.error = Some(error.clone());
                g.recording_started_ms = None;
            }
            outcome == Outcome::Applied
        };
        if !applied {
            return; // stale utterance: its failure is irrelevant
        }
        self.emit_permission_if_needed(&error);
        self.emit_state();
        self.schedule_reset(true);
    }

    fn emit_permission_if_needed(&self, error: &AppError) {
        let kind = match error.code {
            ErrorCode::MicPermissionDenied => PermissionKind::Microphone,
            ErrorCode::AccessibilityPermissionDenied => PermissionKind::Accessibility,
            _ => return,
        };
        self.events
            .emit(AppEvent::PermissionRequired(PermissionRequired {
                kind,
                message: error.recovery.clone(),
            }));
    }

    /// Returns to idle after the notice period unless something newer happened.
    fn schedule_reset(self: &Arc<Self>, is_error: bool) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let epoch = self.inner.lock().unwrap().epoch;
        let notice_ms = self.settings.read().unwrap().behavior.completion_notice_ms as u64;
        let delay = if is_error {
            notice_ms.max(ERROR_NOTICE_MIN_MS)
        } else {
            notice_ms
        };
        let this = self.clone();
        handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let reset = {
                let mut g = this.inner.lock().unwrap();
                if g.epoch != epoch {
                    false
                } else {
                    let (machine, outcome) = reduce(&g.machine, &Event::Reset);
                    if outcome == Outcome::Applied {
                        g.machine = machine;
                        g.epoch += 1;
                        g.error = None;
                        g.notice = None;
                    }
                    outcome == Outcome::Applied
                }
            };
            if reset {
                this.emit_state();
            }
        });
    }

    // ----------------------------------------------------------------- record

    /// Push-to-Talk pressed. Freezes the selected target and starts capture.
    /// Returns `Ok(None)` when a recording is already active (key repeat or
    /// rapid presses), so there is never more than one recording task.
    pub fn ptt_press(self: &Arc<Self>) -> Result<Option<String>> {
        let id = uuid::Uuid::new_v4().to_string();
        if matches!(self.phase(), Phase::Listening | Phase::Injecting) {
            // Key repeat or a busy injector: do not retarget anything.
        } else {
            let behavior = self.settings.read().unwrap().behavior.clone();
            if behavior.follow_focus {
                // The utterance goes to the window the user is in right now.
                // If that is VoiceBridge itself (or nothing), the previous
                // target keeps being used.
                self.registry
                    .track_foreground(behavior.default_output, behavior.auto_submit);
            }
        }
        {
            let mut g = self.inner.lock().unwrap();
            if g.machine.phase == Phase::Listening {
                return Ok(None);
            }
            // The target is frozen here; later selection changes only affect
            // the next utterance.
            let frozen = self.registry.selected_id();
            let (machine, outcome) = reduce(
                &g.machine,
                &Event::PttPressed {
                    utterance_id: id.clone(),
                    frozen_target_id: frozen,
                },
            );
            if outcome != Outcome::Applied {
                return Err(AppError::new(ErrorCode::Busy).with_details("injection in progress"));
            }
            // Cancel any unfinished processing of the previous utterance.
            g.token.cancel();
            g.token = CancellationToken::new();
            g.machine = machine;
            g.epoch += 1;
            g.error = None;
            g.notice = None;
            g.recording_started_ms = Some(now_ms());
        }
        logging::event(&id, "listening", "");
        self.emit_state();

        let settings = self.settings();
        if self.permissions.status(PermissionKind::Microphone) == PermissionStatus::Denied {
            let e = AppError::new(ErrorCode::MicPermissionDenied);
            self.fail(&id, e.clone());
            return Err(e);
        }
        let events = self.events.clone();
        let started = self.audio.start(
            CaptureOptions {
                device: settings.audio.input_device.clone(),
                max_secs: settings.audio.max_recording_secs,
            },
            Arc::new(move |level| events.emit(AppEvent::RecordingLevel(level))),
        );
        if let Err(e) = started {
            self.fail(&id, e.clone());
            return Err(e);
        }

        // Enforce the maximum recording duration.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let this = self.clone();
            let utterance = id.clone();
            let max = Duration::from_secs(settings.audio.max_recording_secs as u64);
            handle.spawn(async move {
                tokio::time::sleep(max).await;
                let still_recording = {
                    let g = this.inner.lock().unwrap();
                    g.machine.phase == Phase::Listening
                        && g.machine.utterance_id.as_deref() == Some(utterance.as_str())
                };
                if still_recording {
                    this.ptt_release().await;
                }
            });
        }
        Ok(Some(id))
    }

    /// Push-to-Talk released: stops capture. Returns the job to process, or
    /// `None` when nothing was recording.
    pub fn begin_release(self: &Arc<Self>) -> Option<Job> {
        let (id, token) = {
            let mut g = self.inner.lock().unwrap();
            if g.machine.phase != Phase::Listening {
                return None;
            }
            let id = g.machine.utterance_id.clone()?;
            let (machine, outcome) = reduce(
                &g.machine,
                &Event::PttReleased {
                    utterance_id: id.clone(),
                },
            );
            if outcome != Outcome::Applied {
                return None;
            }
            g.machine = machine;
            g.epoch += 1;
            g.recording_started_ms = None;
            (id, g.token.clone())
        };
        self.emit_state();
        match self.audio.stop() {
            Ok(audio) => Some(Job {
                id,
                token,
                audio: Some(audio),
            }),
            Err(e) => {
                self.fail(&id, e);
                None
            }
        }
    }

    /// Runs every stage for a released recording.
    pub async fn process(self: &Arc<Self>, job: Job) {
        let Job { id, token, audio } = job;
        let started = Instant::now();
        match self.run_stages(&id, &token, audio).await {
            Ok(()) => logging::event(
                &id,
                "finished",
                &format!("total_ms={}", started.elapsed().as_millis()),
            ),
            // Canceled or superseded: the state already reflects that.
            Err(e) if e.is_canceled() => logging::event(&id, "canceled", ""),
            Err(e) => self.fail(&id, e),
        }
    }

    pub async fn ptt_release(self: &Arc<Self>) {
        if let Some(job) = self.begin_release() {
            self.process(job).await;
        }
    }

    /// Development helper: runs an utterance from supplied audio instead of
    /// the microphone.
    pub async fn simulate_utterance(self: &Arc<Self>, audio: AudioBuffer) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let token = {
            let mut g = self.inner.lock().unwrap();
            let frozen = self.registry.selected_id();
            let (pressed, o1) = reduce(
                &g.machine,
                &Event::PttPressed {
                    utterance_id: id.clone(),
                    frozen_target_id: frozen,
                },
            );
            let (released, o2) = reduce(
                &pressed,
                &Event::PttReleased {
                    utterance_id: id.clone(),
                },
            );
            if o1 != Outcome::Applied || o2 != Outcome::Applied {
                return Err(AppError::new(ErrorCode::Busy));
            }
            g.token.cancel();
            g.token = CancellationToken::new();
            g.machine = released;
            g.epoch += 1;
            g.error = None;
            g.notice = None;
            g.token.clone()
        };
        self.emit_state();
        self.process(Job {
            id,
            token,
            audio: Some(audio),
        })
        .await;
        Ok(())
    }

    // ---------------------------------------------------------------- stages

    fn frozen_target(&self, id: &str) -> (String, Option<TargetSlot>) {
        let frozen = {
            let g = self.inner.lock().unwrap();
            if g.machine.utterance_id.as_deref() == Some(id) {
                g.machine.frozen_target_id.clone()
            } else {
                None
            }
        };
        let target = frozen.as_ref().and_then(|t| self.registry.get(t));
        (frozen.unwrap_or_default(), target)
    }

    fn compile_input(
        raw: &str,
        target: Option<&TargetSlot>,
        terms: &[String],
    ) -> PromptCompileInput {
        let target_ctx = target
            .map(|t| TargetContext {
                app_name: std::path::Path::new(&t.executable_or_bundle_id)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                target_alias: t.alias.clone(),
                project_root: t.project_root.clone(),
                active_file: None,
                language: None,
            })
            .unwrap_or_default();
        PromptCompileInput {
            raw_transcript: raw.to_string(),
            target: target_ctx,
            project_terms: terms.iter().take(MAX_CONTEXT_TERMS).cloned().collect(),
            selected_code: None,
            diagnostics: Vec::new(),
        }
    }

    /// Compiles the prompt for `result`. On any failure other than
    /// cancellation the normalized transcript becomes the prompt, so a
    /// compiler problem can never discard the ASR result.
    async fn compile_into(
        &self,
        result: &mut UtteranceResult,
        token: &CancellationToken,
        target: Option<&TargetSlot>,
        terms: &[String],
        settings: &Settings,
    ) -> Result<Option<String>> {
        let compiler = self.providers().compiler;
        let input = Self::compile_input(&result.raw_transcript, target, terms);
        let started = Instant::now();
        let compiled = run_cancellable(
            token,
            Duration::from_millis(settings.models.prompt_timeout_ms as u64),
            ErrorCode::PromptTimeout,
            compiler.compile(input, token.clone()),
        )
        .await;
        match compiled {
            Ok(c) => {
                logging::event(
                    &result.id,
                    "compiled",
                    &format!("ms={}", started.elapsed().as_millis()),
                );
                // `raw_transcript` is never assigned here: the compiler
                // result type does not even carry one.
                if !c.normalized_transcript.trim().is_empty() {
                    result.normalized_transcript = c.normalized_transcript;
                }
                result.compiled_prompt = c.prompt;
                result.uncertain_identifiers = c.uncertain_identifiers;
                result.needs_confirmation = c.needs_confirmation;
                result.status = UtteranceStatus::Compiled;
                Ok(None)
            }
            Err(e) if e.is_canceled() => Err(e),
            Err(e) => {
                logging::event(&result.id, "compile_failed", &format!("{:?}", e.code));
                result.compiled_prompt = fallback_prompt(&result.normalized_transcript);
                result.uncertain_identifiers.clear();
                result.needs_confirmation = false;
                result.status = UtteranceStatus::PromptFailed;
                Ok(Some(FALLBACK_NOTICE.to_string()))
            }
        }
    }

    async fn run_stages(
        self: &Arc<Self>,
        id: &str,
        token: &CancellationToken,
        audio: Option<AudioBuffer>,
    ) -> Result<()> {
        let settings = self.settings();
        let uid = || id.to_string();

        // FINALIZING_AUDIO
        let audio = audio.ok_or_else(|| AppError::new(ErrorCode::AudioCaptureFailed))?;
        if settings.privacy.save_audio {
            self.save_audio_copy(id, &audio);
        }
        let audio = audio::prepare_for_asr(audio, settings.audio.vad_enabled)?;
        let (frozen_target_id, target) = self.frozen_target(id);
        let terms = target
            .as_ref()
            .map(|t| self.vocab.terms_for(t))
            .unwrap_or_default();

        // TRANSCRIBING
        self.step(
            token,
            Event::AudioFinalized {
                utterance_id: uid(),
            },
        )?;
        let asr = self.providers().asr;
        let context = AsrContext {
            utterance_id: uid(),
            hotwords: terms.iter().take(MAX_CONTEXT_TERMS).cloned().collect(),
            language: Some(settings.models.asr_language.clone()).filter(|l| !l.trim().is_empty()),
        };
        let asr_started = Instant::now();
        let transcript = run_cancellable(
            token,
            Duration::from_millis(settings.models.asr_timeout_ms as u64),
            ErrorCode::AsrTimeout,
            asr.transcribe(audio, context, token.clone()),
        )
        .await?;
        logging::event(
            id,
            "transcribed",
            &format!("ms={}", asr_started.elapsed().as_millis()),
        );
        if transcript.text.trim().is_empty() {
            return Err(AppError::new(ErrorCode::NoSpeechDetected));
        }

        // NORMALIZING — from here on the raw transcript is immutable.
        let mut result = UtteranceResult {
            id: uid(),
            created_at: chrono::Utc::now().to_rfc3339(),
            frozen_target_id,
            normalized_transcript: normalize_transcript(&transcript.text, &terms),
            raw_transcript: transcript.text,
            compiled_prompt: String::new(),
            uncertain_identifiers: Vec::new(),
            needs_confirmation: false,
            status: UtteranceStatus::Transcribed,
        };
        self.store_result(
            token,
            &result,
            Event::Transcribed {
                utterance_id: uid(),
            },
            None,
        )?;
        if settings.privacy.save_transcripts {
            self.append_transcript(&result);
        }

        // COMPILING_PROMPT
        self.step(
            token,
            Event::Normalized {
                utterance_id: uid(),
            },
        )?;
        let notice = self
            .compile_into(&mut result, token, target.as_ref(), &terms, &settings)
            .await?;

        // READY
        self.store_result(
            token,
            &result,
            Event::Compiled {
                utterance_id: uid(),
            },
            notice,
        )?;
        self.history.upsert(&result, settings.behavior.save_history);

        // INJECTING — only automatic when the prompt compiled cleanly.
        let auto = settings.behavior.auto_inject
            && result.status == UtteranceStatus::Compiled
            && !result.needs_confirmation
            && target.is_some();
        if auto {
            // Injection errors are reported by `inject_result` itself.
            let _ = self.inject_result(result, None, Some(token)).await;
        }
        Ok(())
    }

    fn save_audio_copy(&self, id: &str, audio: &AudioBuffer) {
        let Some(dir) = &self.data_dir else { return };
        let dir = dir.join("recordings");
        if let Ok(file) = audio::temp::TempAudioFile::write(&dir, audio) {
            let _ = std::fs::copy(file.path(), dir.join(format!("{id}.wav")));
        }
    }

    fn append_transcript(&self, result: &UtteranceResult) {
        use std::io::Write;
        let Some(dir) = &self.data_dir else { return };
        let _ = std::fs::create_dir_all(dir);
        let line = serde_json::json!({
            "id": result.id,
            "created_at": result.created_at,
            "raw_transcript": result.raw_transcript,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("transcripts.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }
    }

    // ---------------------------------------------------------------- cancel

    /// Cancels the current utterance (or dismisses a finished one). Returns
    /// false when there was nothing to cancel or an injection is running.
    pub fn cancel(self: &Arc<Self>) -> bool {
        let (applied, was_listening, canceled) = {
            let mut g = self.inner.lock().unwrap();
            let was_listening = g.machine.phase == Phase::Listening;
            let (machine, outcome) = reduce(&g.machine, &Event::Canceled);
            if outcome != Outcome::Applied {
                (false, false, false)
            } else {
                g.token.cancel();
                g.token = CancellationToken::new();
                g.machine = machine;
                g.epoch += 1;
                g.error = None;
                g.notice = None;
                g.recording_started_ms = None;
                (true, was_listening, g.machine.phase == Phase::Canceled)
            }
        };
        if !applied {
            return false;
        }
        if was_listening {
            self.audio.abort();
        }
        logging::event("-", "cancel", "");
        self.emit_state();
        if canceled {
            self.schedule_reset(false);
        }
        true
    }

    // ---------------------------------------------------------------- inject

    /// Injects one text variant of the last utterance (or of `utterance_id`).
    /// The utterance always goes to the target frozen when it was recorded;
    /// only utterances recorded without a target use the current selection.
    pub async fn inject(
        self: &Arc<Self>,
        utterance_id: Option<String>,
        kind: Option<OutputKind>,
    ) -> Result<InjectionResult> {
        let result = match utterance_id {
            Some(id) => self
                .last_result()
                .filter(|r| r.id == id)
                .or_else(|| self.history.get(&id)),
            None => self.last_result(),
        }
        .ok_or_else(|| {
            AppError::new(ErrorCode::InvalidInput)
                .with_message("There is no transcription to inject yet.")
        })?;
        self.inject_result(result, kind, None).await
    }

    async fn inject_result(
        self: &Arc<Self>,
        result: UtteranceResult,
        kind: Option<OutputKind>,
        guard: Option<&CancellationToken>,
    ) -> Result<InjectionResult> {
        let target_id = if result.frozen_target_id.is_empty() {
            self.registry.selected_id()
        } else {
            Some(result.frozen_target_id.clone())
        };
        let target = target_id.and_then(|id| self.registry.get(&id));
        let prepared = target
            .ok_or_else(|| {
                AppError::new(ErrorCode::TargetNotFound)
                    .with_details("no pinned target for this utterance")
            })
            .and_then(|target| {
                let kind = kind.unwrap_or(target.preferred_output);
                let text = result.text(kind).to_string();
                if text.trim().is_empty() {
                    Err(AppError::new(ErrorCode::InvalidInput)
                        .with_message("That text is empty, so there is nothing to inject."))
                } else {
                    Ok((target, text))
                }
            });

        // Enter INJECTING. A canceled or superseded utterance never gets here.
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(token) = guard {
                if token.is_cancelled()
                    || g.machine.utterance_id.as_deref() != Some(result.id.as_str())
                {
                    return Err(AppError::canceled());
                }
            }
            let (machine, outcome) = reduce(
                &g.machine,
                &Event::InjectStarted {
                    utterance_id: result.id.clone(),
                    frozen_target_id: prepared.as_ref().ok().map(|(t, _)| t.id.clone()),
                },
            );
            if outcome != Outcome::Applied {
                return Err(AppError::new(ErrorCode::Busy));
            }
            g.machine = machine;
            g.epoch += 1;
            g.error = None;
        }
        self.emit_state();

        let started = Instant::now();
        let outcome = match prepared {
            Ok((target, text)) => {
                let options = InjectionOptions {
                    auto_submit: target.auto_submit,
                    ..Default::default()
                };
                self.injector.inject(&target, &text, options).await
            }
            Err(e) => Err(e),
        };
        logging::event(
            &result.id,
            "injected",
            &format!(
                "ok={} ms={}",
                outcome.is_ok(),
                started.elapsed().as_millis()
            ),
        );

        let status = if outcome.is_ok() {
            UtteranceStatus::Injected
        } else {
            UtteranceStatus::InjectionFailed
        };
        let updated = {
            let mut g = self.inner.lock().unwrap();
            let event = match &outcome {
                Ok(_) => Event::InjectFinished {
                    utterance_id: result.id.clone(),
                },
                Err(_) => Event::Failed {
                    utterance_id: result.id.clone(),
                },
            };
            let (machine, o) = reduce(&g.machine, &event);
            if o == Outcome::Applied {
                g.machine = machine;
                g.epoch += 1;
                g.error = outcome.as_ref().err().cloned();
            }
            // Generated text is always preserved so the user can retry.
            let mut updated = result.clone();
            updated.status = status;
            if let Some(last) = g.last_result.as_mut().filter(|r| r.id == result.id) {
                last.status = status;
            }
            updated
        };
        let persist = self.settings.read().unwrap().behavior.save_history;
        self.history.upsert(&updated, persist);
        self.events
            .emit(AppEvent::InjectionResult(InjectionOutcome {
                utterance_id: result.id.clone(),
                ok: outcome.is_ok(),
                result: outcome.as_ref().ok().cloned(),
                error: outcome.as_ref().err().cloned(),
            }));
        self.events.emit(AppEvent::UtteranceUpdated(updated));
        if let Err(e) = &outcome {
            self.emit_permission_if_needed(e);
        }
        self.emit_state();
        self.schedule_reset(outcome.is_err());
        outcome
    }

    // ------------------------------------------------------------- recompile

    /// Re-runs prompt compilation for the last utterance. The raw transcript
    /// is left untouched.
    pub async fn recompile_last(self: &Arc<Self>) -> Result<UtteranceResult> {
        let mut result = self.last_result().ok_or_else(|| {
            AppError::new(ErrorCode::InvalidInput)
                .with_message("There is no transcription to recompile yet.")
        })?;
        let target = Some(result.frozen_target_id.clone())
            .filter(|t| !t.is_empty())
            .and_then(|t| self.registry.get(&t));
        let token = {
            let mut g = self.inner.lock().unwrap();
            let (machine, outcome) = reduce(
                &g.machine,
                &Event::RecompileStarted {
                    utterance_id: result.id.clone(),
                    frozen_target_id: target.as_ref().map(|t| t.id.clone()),
                },
            );
            if outcome != Outcome::Applied {
                return Err(AppError::new(ErrorCode::Busy));
            }
            g.token.cancel();
            g.token = CancellationToken::new();
            g.machine = machine;
            g.epoch += 1;
            g.error = None;
            g.notice = None;
            g.token.clone()
        };
        self.emit_state();

        let settings = self.settings();
        let terms = target
            .as_ref()
            .map(|t| self.vocab.terms_for(t))
            .unwrap_or_default();
        result.normalized_transcript = normalize_transcript(&result.raw_transcript, &terms);
        let notice = self
            .compile_into(&mut result, &token, target.as_ref(), &terms, &settings)
            .await?;
        let id = result.id.clone();
        self.store_result(
            &token,
            &result,
            Event::Compiled { utterance_id: id },
            notice,
        )?;
        self.history.upsert(&result, settings.behavior.save_history);
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
