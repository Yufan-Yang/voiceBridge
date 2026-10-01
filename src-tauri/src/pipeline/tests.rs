//! Pipeline tests. Everything runs against mocks: no desktop session, no
//! microphone and no model files are needed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use tokio::sync::Semaphore;

use super::*;
use crate::asr::TranscriptResult;
use crate::audio::MockAudioSource;
use crate::config::MemoryStore;
use crate::events::CollectingSink;
use crate::platform::mock::MockDesktop;
use crate::prompt::schema::parse_model_output;
use crate::prompt::{MockPromptCompiler, PromptCompileResult};

type Gate = Option<Arc<Semaphore>>;

async fn pass(gate: &Gate) {
    if let Some(g) = gate {
        g.acquire().await.unwrap().forget();
    }
}

/// ASR double. Each call pops (text, gate) and waits at the gate. It
/// deliberately ignores the cancellation token, like a runtime that returns
/// late, so stale-result handling is exercised.
#[derive(Default)]
struct ScriptedAsr {
    script: Mutex<VecDeque<(Result<String>, Gate)>>,
    calls: AtomicU32,
    hotwords: Mutex<Vec<Vec<String>>>,
}

impl ScriptedAsr {
    fn push(&self, text: &str, gate: Gate) {
        self.script
            .lock()
            .unwrap()
            .push_back((Ok(text.to_string()), gate));
    }
}

#[async_trait]
impl AsrProvider for ScriptedAsr {
    async fn health_check(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth::new("test", ProviderStatus::Ready, ""))
    }

    async fn transcribe(
        &self,
        audio: AudioBuffer,
        context: AsrContext,
        _cancel: CancellationToken,
    ) -> Result<TranscriptResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.hotwords.lock().unwrap().push(context.hotwords);
        let (text, gate) = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or((Ok("default transcript".to_string()), None));
        pass(&gate).await;
        text.map(|text| TranscriptResult {
            text,
            duration_ms: audio.duration_ms(),
        })
    }
}

enum CompilerMode {
    /// Behave like the rule-based mock.
    Mock,
    /// Return this text as if the model produced it.
    ModelText(String),
    Fail(ErrorCode),
    Hang,
}

struct TestCompiler {
    mode: Mutex<CompilerMode>,
    gate: Mutex<Gate>,
    calls: AtomicU32,
    inputs: Mutex<Vec<PromptCompileInput>>,
}

impl Default for TestCompiler {
    fn default() -> Self {
        Self {
            mode: Mutex::new(CompilerMode::Mock),
            gate: Mutex::new(None),
            calls: AtomicU32::new(0),
            inputs: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PromptCompiler for TestCompiler {
    async fn health_check(&self) -> Result<ProviderHealth> {
        Ok(ProviderHealth::new("test", ProviderStatus::Ready, ""))
    }

    async fn compile(
        &self,
        input: PromptCompileInput,
        cancel: CancellationToken,
    ) -> Result<PromptCompileResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(input.clone());
        let gate = self.gate.lock().unwrap().clone();
        pass(&gate).await;
        let action = match &*self.mode.lock().unwrap() {
            CompilerMode::Mock => None,
            CompilerMode::ModelText(t) => Some(parse_model_output(t)),
            CompilerMode::Fail(code) => Some(Err(AppError::new(*code))),
            CompilerMode::Hang => Some(Err(AppError::new(ErrorCode::Internal))),
        };
        if matches!(&*self.mode.lock().unwrap(), CompilerMode::Hang) {
            std::future::pending::<()>().await;
        }
        match action {
            Some(r) => r,
            None => MockPromptCompiler::instant().compile(input, cancel).await,
        }
    }
}

#[derive(Default)]
struct RecordingInjector {
    /// (target id, text, auto_submit)
    calls: Mutex<Vec<(String, String, bool)>>,
    fail: Mutex<Option<ErrorCode>>,
}

#[async_trait]
impl InputInjector for RecordingInjector {
    async fn inject(
        &self,
        target: &TargetSlot,
        text: &str,
        options: InjectionOptions,
    ) -> Result<InjectionResult> {
        if let Some(code) = *self.fail.lock().unwrap() {
            return Err(AppError::new(code));
        }
        self.calls
            .lock()
            .unwrap()
            .push((target.id.clone(), text.to_string(), options.auto_submit));
        Ok(InjectionResult {
            target_id: target.id.clone(),
            chars: text.chars().count() as u32,
            submitted: options.auto_submit,
            clipboard_restored: true,
        })
    }
}

struct Harness {
    p: Arc<Pipeline>,
    desk: MockDesktop,
    registry: Arc<TargetRegistry>,
    audio: Arc<MockAudioSource>,
    asr: Arc<ScriptedAsr>,
    compiler: Arc<TestCompiler>,
    injector: Arc<RecordingInjector>,
    sink: Arc<CollectingSink>,
    settings: Arc<RwLock<Settings>>,
    history: Arc<HistoryStore>,
    store: Arc<MemoryStore>,
    target_a: TargetSlot,
    target_b: TargetSlot,
}

fn harness() -> Harness {
    let desk = MockDesktop::new();
    desk.add_window(MockDesktop::window("1", 10, "/Apps/Claude", "backend"));
    desk.add_window(MockDesktop::window("2", 20, "/Apps/Codex", "web"));
    desk.add_window(MockDesktop::window("3", 30, "/Apps/Cursor", "infra"));
    let store = Arc::new(MemoryStore::default());
    let registry = Arc::new(TargetRegistry::new(Arc::new(desk.clone()), store.clone()));
    let target_a = registry
        .bind_window_id("1", 1, OutputKind::Prompt, false)
        .unwrap();
    let target_b = registry
        .bind_window_id("2", 2, OutputKind::Prompt, false)
        .unwrap();
    registry
        .bind_window_id("3", 3, OutputKind::Prompt, false)
        .unwrap();
    registry.select(&target_a.id).unwrap();

    let audio = Arc::new(MockAudioSource::default());
    let asr = Arc::new(ScriptedAsr::default());
    let compiler = Arc::new(TestCompiler::default());
    let injector = Arc::new(RecordingInjector::default());
    let sink = Arc::new(CollectingSink::default());
    let settings = Arc::new(RwLock::new(Settings::default()));
    let history = Arc::new(HistoryStore::new(store.clone()));
    let p = Pipeline::new(PipelineDeps {
        providers: Providers {
            asr: asr.clone(),
            compiler: compiler.clone(),
        },
        injector: injector.clone(),
        audio: audio.clone(),
        registry: registry.clone(),
        vocab: Arc::new(VocabStore::default()),
        events: sink.clone(),
        settings: settings.clone(),
        history: history.clone(),
        permissions: Arc::new(desk.clone()),
        data_dir: None,
    });
    Harness {
        p,
        desk,
        registry,
        audio,
        asr,
        compiler,
        injector,
        sink,
        settings,
        history,
        store,
        target_a,
        target_b,
    }
}

fn gate() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(0))
}

async fn wait_phase(p: &Arc<Pipeline>, phase: Phase) {
    for _ in 0..400 {
        if p.phase() == phase {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {phase:?}; currently {:?}", p.phase());
}

/// Press + release, processing to completion.
async fn speak(h: &Harness) {
    h.p.ptt_press().unwrap().expect("recording should start");
    h.p.ptt_release().await;
}

fn spawn_release(h: &Harness) -> tokio::task::JoinHandle<()> {
    let p = h.p.clone();
    tokio::spawn(async move { p.ptt_release().await })
}

// --------------------------------------------------------------- full flow

#[tokio::test]
async fn full_flow_produces_three_texts_and_injects_the_prompt_without_enter() {
    let h = harness();
    h.asr.push("change the login handler, add two tests", None);
    speak(&h).await;

    let r = h.p.last_result().unwrap();
    assert_eq!(r.raw_transcript, "change the login handler, add two tests");
    assert_eq!(
        r.normalized_transcript,
        "Change the login handler, add two tests."
    );
    assert!(r.compiled_prompt.contains("- Add two tests"));
    assert_eq!(r.status, UtteranceStatus::Injected);
    assert_eq!(h.p.phase(), Phase::Done);

    // The compiled prompt is what gets injected by default, and Enter is off.
    let calls = h.injector.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        vec![(h.target_a.id.clone(), r.compiled_prompt.clone(), false)]
    );

    // The UI saw every phase in order.
    let phases: Vec<Phase> = h
        .sink
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AppEvent::StateChanged(s) => Some(s.phase),
            _ => None,
        })
        .collect();
    let mut dedup = phases.clone();
    dedup.dedup();
    assert_eq!(
        dedup,
        vec![
            Phase::Listening,
            Phase::FinalizingAudio,
            Phase::Transcribing,
            Phase::Normalizing,
            Phase::CompilingPrompt,
            Phase::Ready,
            Phase::Injecting,
            Phase::Done,
        ]
    );
}

#[tokio::test]
async fn each_text_variant_can_be_injected_independently() {
    let h = harness();
    h.settings.write().unwrap().behavior.auto_inject = false;
    h.asr.push("fix the parser, keep the api stable", None);
    speak(&h).await;
    assert_eq!(h.p.phase(), Phase::Ready);
    assert!(
        h.injector.calls.lock().unwrap().is_empty(),
        "auto-inject is off"
    );

    let r = h.p.last_result().unwrap();
    h.p.inject(None, Some(OutputKind::Raw)).await.unwrap();
    h.p.inject(None, Some(OutputKind::Normalized))
        .await
        .unwrap();
    h.p.inject(Some(r.id.clone()), Some(OutputKind::Prompt))
        .await
        .unwrap();
    let texts: Vec<String> = h
        .injector
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.1.clone())
        .collect();
    assert_eq!(
        texts,
        vec![r.raw_transcript, r.normalized_transcript, r.compiled_prompt]
    );
}

#[tokio::test]
async fn target_output_preference_and_auto_submit_are_per_target() {
    let h = harness();
    h.registry
        .set_output(&h.target_a.id, OutputKind::Raw)
        .unwrap();
    h.registry.set_auto_submit(&h.target_a.id, true).unwrap();
    h.asr.push("ship it", None);
    speak(&h).await;
    assert_eq!(
        h.injector.calls.lock().unwrap().clone(),
        vec![(h.target_a.id.clone(), "ship it".to_string(), true)]
    );
}

// ----------------------------------------------------------- frozen target

#[tokio::test]
async fn target_is_frozen_when_recording_begins() {
    let h = harness();
    h.p.ptt_press().unwrap();
    let snap = h.p.snapshot();
    assert_eq!(snap.phase, Phase::Listening);
    assert_eq!(snap.frozen_target_id, Some(h.target_a.id.clone()));

    // Switching while still recording only affects the next utterance.
    h.registry.select(&h.target_b.id).unwrap();
    h.p.ptt_release().await;
    assert_eq!(h.p.last_result().unwrap().frozen_target_id, h.target_a.id);
    assert_eq!(h.injector.calls.lock().unwrap()[0].0, h.target_a.id);

    speak(&h).await;
    assert_eq!(h.p.last_result().unwrap().frozen_target_id, h.target_b.id);
    assert_eq!(h.injector.calls.lock().unwrap()[1].0, h.target_b.id);
}

#[tokio::test]
async fn switching_targets_during_transcription_does_not_change_frozen_target() {
    let h = harness();
    let asr_gate = gate();
    h.asr.push("first utterance", Some(asr_gate.clone()));
    h.p.ptt_press().unwrap();
    let job = spawn_release(&h);
    wait_phase(&h.p, Phase::Transcribing).await;

    h.registry.select(&h.target_b.id).unwrap();
    h.p.emit_targets();
    let snap = h.p.snapshot();
    assert_eq!(snap.selected_target_id, Some(h.target_b.id.clone()));
    assert_eq!(snap.frozen_target_id, Some(h.target_a.id.clone()));

    asr_gate.add_permits(1);
    job.await.unwrap();
    let r = h.p.last_result().unwrap();
    assert_eq!(r.frozen_target_id, h.target_a.id);
    let calls = h.injector.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0, h.target_a.id,
        "must not be rerouted to the newly selected target"
    );
}

#[tokio::test]
async fn switching_targets_during_prompt_compilation_does_not_reroute() {
    let h = harness();
    let compile_gate = gate();
    *h.compiler.gate.lock().unwrap() = Some(compile_gate.clone());
    h.p.ptt_press().unwrap();
    let job = spawn_release(&h);
    wait_phase(&h.p, Phase::CompilingPrompt).await;
    h.registry.select(&h.target_b.id).unwrap();
    compile_gate.add_permits(1);
    job.await.unwrap();
    assert_eq!(h.injector.calls.lock().unwrap()[0].0, h.target_a.id);
    // The compiler was given the frozen target's context, not the new one.
    assert_eq!(
        h.compiler.inputs.lock().unwrap()[0].target.target_alias,
        h.target_a.alias
    );
}

// ------------------------------------------------------------ stale results

#[tokio::test]
async fn older_utterance_result_cannot_overwrite_a_newer_utterance() {
    let h = harness();
    let slow = gate();
    h.asr.push("OLD utterance text", Some(slow.clone()));
    h.asr.push("NEW utterance text", None);

    h.p.ptt_press().unwrap();
    let old_job = spawn_release(&h);
    wait_phase(&h.p, Phase::Transcribing).await;

    // A new recording starts while the old one is still transcribing.
    speak(&h).await;
    assert_eq!(
        h.p.last_result().unwrap().raw_transcript,
        "NEW utterance text"
    );

    // The old ASR call now returns late.
    slow.add_permits(1);
    old_job.await.unwrap();
    let r = h.p.last_result().unwrap();
    assert_eq!(r.raw_transcript, "NEW utterance text");
    let calls = h.injector.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "the superseded utterance must not be injected"
    );
    assert!(calls[0].1.contains("NEW"));
    assert_eq!(h.history.list().len(), 1);
}

#[tokio::test]
async fn stale_async_result_is_ignored_after_cancellation() {
    let h = harness();
    let slow = gate();
    h.asr.push("should never appear", Some(slow.clone()));
    h.p.ptt_press().unwrap();
    let job = spawn_release(&h);
    wait_phase(&h.p, Phase::Transcribing).await;

    assert!(h.p.cancel());
    assert_eq!(h.p.phase(), Phase::Canceled);
    slow.add_permits(1);
    job.await.unwrap();

    assert!(h.p.last_result().is_none());
    assert_eq!(h.p.phase(), Phase::Canceled);
    assert!(h.injector.calls.lock().unwrap().is_empty());
    assert_eq!(
        h.compiler.calls.load(Ordering::SeqCst),
        0,
        "no later stage may run"
    );
}

#[tokio::test]
async fn canceling_prevents_later_injection() {
    let h = harness();
    let compile_gate = gate();
    *h.compiler.gate.lock().unwrap() = Some(compile_gate.clone());
    h.p.ptt_press().unwrap();
    let job = spawn_release(&h);
    wait_phase(&h.p, Phase::CompilingPrompt).await;

    assert!(h.p.cancel());
    compile_gate.add_permits(1);
    job.await.unwrap();

    assert!(
        h.injector.calls.lock().unwrap().is_empty(),
        "canceled utterance must not be injected"
    );
    assert_eq!(h.p.phase(), Phase::Canceled);
    // The transcription that already existed is still available.
    assert_eq!(
        h.p.last_result().unwrap().status,
        UtteranceStatus::Transcribed
    );
}

#[tokio::test]
async fn canceling_while_recording_discards_audio() {
    let h = harness();
    h.p.ptt_press().unwrap();
    assert!(h.audio.is_active());
    assert!(h.p.cancel());
    assert!(!h.audio.is_active());
    assert_eq!(h.audio.aborts.load(Ordering::SeqCst), 1);
    // The release that follows the cancel does nothing.
    h.p.ptt_release().await;
    assert_eq!(h.asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(h.p.phase(), Phase::Canceled);
}

// ------------------------------------------------------------- concurrency

#[tokio::test]
async fn rapid_push_to_talk_creates_a_single_recording_task() {
    let h = harness();
    let first = h.p.ptt_press().unwrap();
    assert!(first.is_some());
    for _ in 0..10 {
        assert_eq!(h.p.ptt_press().unwrap(), None);
    }
    assert_eq!(h.audio.starts.load(Ordering::SeqCst), 1);
    assert_eq!(h.p.snapshot().utterance_id, first);

    // Repeated releases only process once.
    let (a, b) = tokio::join!(h.p.ptt_release(), h.p.ptt_release());
    let _ = (a, b);
    assert_eq!(h.asr.calls.load(Ordering::SeqCst), 1);
    assert_eq!(h.injector.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn recording_is_refused_while_injecting() {
    let h = harness();
    // Force the machine into INJECTING through the public API.
    h.settings.write().unwrap().behavior.auto_inject = false;
    speak(&h).await;
    let id = h.p.last_result().unwrap().id;
    assert_eq!(
        h.p.apply(&Event::InjectStarted {
            utterance_id: id,
            frozen_target_id: None
        }),
        Outcome::Applied
    );
    let err = h.p.ptt_press().unwrap_err();
    assert_eq!(err.code, ErrorCode::Busy);
    assert!(!h.p.cancel(), "an injection cannot be canceled midway");
    assert_eq!(h.audio.starts.load(Ordering::SeqCst), 1);
}

// --------------------------------------------------------- prompt compiler

#[tokio::test]
async fn prompt_compiler_cannot_overwrite_raw_transcript() {
    let h = harness();
    h.asr.push("um make the thing faster", None);
    *h.compiler.mode.lock().unwrap() = CompilerMode::ModelText(
        r#"{"raw_transcript":"REWRITTEN BY MODEL","normalized_transcript":"Make the thing faster.",
            "intent":"optimize","prompt":"Improve the performance of the thing.",
            "uncertain_identifiers":["thing"],"needs_confirmation":false}"#
            .to_string(),
    );
    speak(&h).await;
    let r = h.p.last_result().unwrap();
    assert_eq!(
        r.raw_transcript, "um make the thing faster",
        "raw transcript is immutable"
    );
    assert_eq!(r.normalized_transcript, "Make the thing faster.");
    assert_eq!(r.compiled_prompt, "Improve the performance of the thing.");
    assert_eq!(r.uncertain_identifiers, vec!["thing"]);
    // The model never even receives a way to change it: input is read-only.
    assert_eq!(
        h.compiler.inputs.lock().unwrap()[0].raw_transcript,
        "um make the thing faster"
    );
}

#[tokio::test]
async fn invalid_prompt_json_triggers_fallback_and_keeps_transcript() {
    let h = harness();
    h.asr.push("rename the config loader", None);
    *h.compiler.mode.lock().unwrap() =
        CompilerMode::ModelText("Sure! Here is some code: fn main() {".into());
    speak(&h).await;

    let snap = h.p.snapshot();
    let r = snap.last_result.unwrap();
    assert_eq!(r.raw_transcript, "rename the config loader");
    assert_eq!(r.normalized_transcript, "Rename the config loader.");
    assert_eq!(
        r.compiled_prompt, r.normalized_transcript,
        "fallback uses the normalized transcript"
    );
    assert_eq!(r.status, UtteranceStatus::PromptFailed);
    assert_eq!(snap.phase, Phase::Ready);
    assert_eq!(snap.notice.as_deref(), Some(FALLBACK_NOTICE));
    assert!(snap.error.is_none());
    assert_eq!(
        h.compiler.calls.load(Ordering::SeqCst),
        1,
        "no endless retries"
    );
    assert!(
        h.injector.calls.lock().unwrap().is_empty(),
        "a failed prompt is not auto-injected"
    );

    // The user can still inject the raw or normalized transcript.
    h.p.inject(None, Some(OutputKind::Raw)).await.unwrap();
    h.p.inject(None, Some(OutputKind::Normalized))
        .await
        .unwrap();
    let texts: Vec<String> = h
        .injector
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.1.clone())
        .collect();
    assert_eq!(
        texts,
        vec!["rename the config loader", "Rename the config loader."]
    );
}

#[tokio::test]
async fn prompt_timeout_after_asr_success_preserves_transcript() {
    let h = harness();
    h.settings.write().unwrap().models.prompt_timeout_ms = 40;
    *h.compiler.mode.lock().unwrap() = CompilerMode::Hang;
    h.asr.push("add retry logic", None);
    speak(&h).await;
    let r = h.p.last_result().unwrap();
    assert_eq!(r.raw_transcript, "add retry logic");
    assert_eq!(r.status, UtteranceStatus::PromptFailed);
    assert_eq!(h.p.snapshot().notice.as_deref(), Some(FALLBACK_NOTICE));
}

#[tokio::test]
async fn needs_confirmation_blocks_automatic_injection() {
    let h = harness();
    *h.compiler.mode.lock().unwrap() = CompilerMode::ModelText(
        r#"{"normalized_transcript":"Fix that function.","intent":"fix","prompt":"Fix the function.",
            "uncertain_identifiers":["that function"],"needs_confirmation":true}"#
            .into(),
    );
    speak(&h).await;
    assert_eq!(h.p.phase(), Phase::Ready);
    assert!(h.p.last_result().unwrap().needs_confirmation);
    assert!(h.injector.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recompile_keeps_raw_transcript() {
    let h = harness();
    h.settings.write().unwrap().behavior.auto_inject = false;
    h.asr.push("tidy the imports", None);
    *h.compiler.mode.lock().unwrap() = CompilerMode::Fail(ErrorCode::PromptProcessFailed);
    speak(&h).await;
    assert_eq!(
        h.p.last_result().unwrap().status,
        UtteranceStatus::PromptFailed
    );

    *h.compiler.mode.lock().unwrap() = CompilerMode::Mock;
    let r = h.p.recompile_last().await.unwrap();
    assert_eq!(r.raw_transcript, "tidy the imports");
    assert_eq!(r.status, UtteranceStatus::Compiled);
    assert_eq!(h.p.phase(), Phase::Ready);
    assert!(h.p.snapshot().notice.is_none());
}

// ----------------------------------------------------------------- failures

#[tokio::test]
async fn asr_failure_leaves_the_application_operational() {
    let h = harness();
    h.asr
        .script
        .lock()
        .unwrap()
        .push_back((Err(AppError::new(ErrorCode::AsrProcessFailed)), None));
    speak(&h).await;
    let snap = h.p.snapshot();
    assert_eq!(snap.phase, Phase::Error);
    assert_eq!(snap.error.unwrap().code, ErrorCode::AsrProcessFailed);

    // A new utterance works straight away.
    h.asr.push("second try", None);
    speak(&h).await;
    assert_eq!(h.p.last_result().unwrap().raw_transcript, "second try");
    assert_eq!(h.p.phase(), Phase::Done);
}

#[tokio::test]
async fn asr_timeout_is_reported() {
    let h = harness();
    h.settings.write().unwrap().models.asr_timeout_ms = 30;
    h.asr.push("never", Some(gate()));
    speak(&h).await;
    assert_eq!(h.p.snapshot().error.unwrap().code, ErrorCode::AsrTimeout);
}

#[tokio::test]
async fn injection_failure_preserves_texts_for_retry() {
    let h = harness();
    *h.injector.fail.lock().unwrap() = Some(ErrorCode::ForegroundVerificationFailed);
    h.asr.push("update the readme", None);
    speak(&h).await;

    let snap = h.p.snapshot();
    assert_eq!(snap.phase, Phase::Error);
    assert_eq!(
        snap.error.unwrap().code,
        ErrorCode::ForegroundVerificationFailed
    );
    let r = snap.last_result.unwrap();
    assert_eq!(r.status, UtteranceStatus::InjectionFailed);
    assert!(
        !r.raw_transcript.is_empty()
            && !r.normalized_transcript.is_empty()
            && !r.compiled_prompt.is_empty()
    );

    // Retry succeeds once the problem is gone.
    *h.injector.fail.lock().unwrap() = None;
    h.p.inject(None, None).await.unwrap();
    assert_eq!(h.p.phase(), Phase::Done);
    assert_eq!(h.p.last_result().unwrap().status, UtteranceStatus::Injected);
}

#[tokio::test]
async fn target_closed_before_injection_reports_error_and_keeps_text() {
    let h = harness();
    h.asr.push("deploy the fix", None);
    h.p.ptt_press().unwrap();
    // The frozen target is unbound while the utterance is being recorded.
    h.registry.unbind(&h.target_a.id).unwrap();
    h.desk.remove_window("1");
    h.p.ptt_release().await;
    // No target context any more, so nothing is auto-injected anywhere else.
    assert!(h.injector.calls.lock().unwrap().is_empty());
    assert_eq!(h.p.phase(), Phase::Ready);
    let err = h.p.inject(None, None).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::TargetNotFound);
    assert_eq!(h.p.last_result().unwrap().raw_transcript, "deploy the fix");
}

#[tokio::test]
async fn microphone_permission_denied_is_reported() {
    let h = harness();
    h.desk.state.lock().unwrap().microphone = PermissionStatus::Denied;
    let err = h.p.ptt_press().unwrap_err();
    assert_eq!(err.code, ErrorCode::MicPermissionDenied);
    assert_eq!(h.p.phase(), Phase::Error);
    assert_eq!(h.audio.starts.load(Ordering::SeqCst), 0);
    let asked = h.sink.events.lock().unwrap().iter().any(
        |e| matches!(e, AppEvent::PermissionRequired(p) if p.kind == PermissionKind::Microphone),
    );
    assert!(asked);
}

#[tokio::test]
async fn missing_microphone_is_reported() {
    let h = harness();
    *h.audio.fail_start.lock().unwrap() = Some(AppError::new(ErrorCode::MicDeviceNotFound));
    let err = h.p.ptt_press().unwrap_err();
    assert_eq!(err.code, ErrorCode::MicDeviceNotFound);
    assert_eq!(h.p.phase(), Phase::Error);
    // Recovers as soon as a device is available again.
    *h.audio.fail_start.lock().unwrap() = None;
    speak(&h).await;
    assert_eq!(h.p.phase(), Phase::Done);
}

// ------------------------------------------------------- context & privacy

#[tokio::test]
async fn vocabulary_follows_the_frozen_target() {
    let h = harness();
    h.registry
        .set_manual_terms(&h.target_a.id, vec!["useUserQuery".into()])
        .unwrap();
    h.registry
        .set_manual_terms(&h.target_b.id, vec!["HandleLogin".into()])
        .unwrap();

    h.asr.push("fix use user query", None);
    speak(&h).await;
    assert_eq!(
        h.p.last_result().unwrap().normalized_transcript,
        "Fix useUserQuery."
    );

    h.registry.select(&h.target_b.id).unwrap();
    h.asr.push("fix use user query and handle login", None);
    speak(&h).await;
    assert_eq!(
        h.p.last_result().unwrap().normalized_transcript,
        "Fix use user query and HandleLogin."
    );
    let hotwords = h.asr.hotwords.lock().unwrap().clone();
    assert_eq!(
        hotwords,
        vec![
            vec!["useUserQuery".to_string()],
            vec!["HandleLogin".to_string()]
        ]
    );
    let inputs = h.compiler.inputs.lock().unwrap();
    assert_eq!(inputs[1].project_terms, vec!["HandleLogin"]);
    assert!(inputs[1].selected_code.is_none());
}

#[tokio::test]
async fn history_is_memory_only_unless_enabled() {
    let h = harness();
    speak(&h).await;
    assert_eq!(h.history.list().len(), 1);
    assert!(
        crate::config::ConfigStore::load(&*h.store, crate::config::HISTORY_KEY)
            .unwrap()
            .is_none(),
        "history must not be written to disk by default"
    );

    h.settings.write().unwrap().behavior.save_history = true;
    speak(&h).await;
    assert!(
        crate::config::ConfigStore::load(&*h.store, crate::config::HISTORY_KEY)
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn utterance_without_target_is_kept_but_not_injected() {
    let h = harness();
    for t in h.registry.list() {
        h.registry.unbind(&t.id).unwrap();
    }
    h.asr.push("no target yet", None);
    speak(&h).await;
    assert_eq!(h.p.phase(), Phase::Ready);
    assert_eq!(h.p.last_result().unwrap().frozen_target_id, "");
    assert!(h.injector.calls.lock().unwrap().is_empty());
}

// ------------------------------------------------------------ follow focus

#[tokio::test]
async fn words_go_to_the_window_in_focus_when_speaking_starts() {
    let h = harness();
    // Target A is selected, but the user is typing in window 2 (target B).
    h.desk.set_foreground("2");
    h.asr.push("first", None);
    speak(&h).await;
    assert_eq!(h.p.last_result().unwrap().frozen_target_id, h.target_b.id);
    assert_eq!(h.injector.calls.lock().unwrap()[0].0, h.target_b.id);

    // A window that was never pinned is picked up automatically.
    h.desk
        .add_window(MockDesktop::window("77", 70, "/Apps/Terminal", "zsh"));
    h.desk.set_foreground("77");
    h.asr.push("second", None);
    speak(&h).await;
    let terminal = h
        .registry
        .list()
        .into_iter()
        .find(|t| t.platform_window_id == "77")
        .unwrap();
    assert_eq!(h.injector.calls.lock().unwrap()[1].0, terminal.id);
    assert!(
        !h.injector.calls.lock().unwrap()[1].2,
        "Enter stays off for auto-tracked windows"
    );
}

#[tokio::test]
async fn moving_focus_while_processing_does_not_move_the_utterance() {
    let h = harness();
    h.desk.set_foreground("2");
    let asr_gate = gate();
    h.asr.push("stay with window two", Some(asr_gate.clone()));
    h.p.ptt_press().unwrap();
    let job = spawn_release(&h);
    wait_phase(&h.p, Phase::Transcribing).await;

    // The user clicks into another window while the models are working.
    h.desk.set_foreground("3");
    asr_gate.add_permits(1);
    job.await.unwrap();
    assert_eq!(h.injector.calls.lock().unwrap()[0].0, h.target_b.id);

    // The target only changes when a new utterance starts in the other window.
    h.asr.push("now window three", None);
    speak(&h).await;
    let third = h
        .registry
        .list()
        .into_iter()
        .find(|t| t.platform_window_id == "3")
        .unwrap();
    assert_eq!(h.injector.calls.lock().unwrap()[1].0, third.id);
}

#[tokio::test]
async fn follow_focus_can_be_turned_off() {
    let h = harness();
    h.settings.write().unwrap().behavior.follow_focus = false;
    h.desk.set_foreground("2");
    speak(&h).await;
    assert_eq!(
        h.injector.calls.lock().unwrap()[0].0,
        h.target_a.id,
        "manual selection is used"
    );
}

// ------------------------------------------------------------- fast output

#[tokio::test]
async fn raw_or_normalized_output_skips_the_prompt_model() {
    let h = harness();
    h.registry
        .set_output(&h.target_a.id, OutputKind::Normalized)
        .unwrap();
    h.asr.push("tidy the imports", None);
    speak(&h).await;
    assert_eq!(
        h.compiler.calls.load(Ordering::SeqCst),
        0,
        "prompt model must not run"
    );
    assert_eq!(h.p.phase(), Phase::Done);
    let r = h.p.last_result().unwrap();
    assert_eq!(r.raw_transcript, "tidy the imports");
    assert_eq!(r.compiled_prompt, "");
    assert_eq!(h.injector.calls.lock().unwrap()[0].1, "Tidy the imports.");

    // The prompt is still available on demand.
    let r = h.p.recompile_last().await.unwrap();
    assert_eq!(h.compiler.calls.load(Ordering::SeqCst), 1);
    assert!(!r.compiled_prompt.is_empty());
    assert_eq!(r.raw_transcript, "tidy the imports");
}

#[tokio::test]
async fn result_records_audio_length_and_processing_times() {
    let h = harness();
    speak(&h).await;
    let r = h.p.last_result().unwrap();
    assert_eq!(
        r.audio_ms, 800,
        "length of the recording (the mock records 800 ms)"
    );
    // Stage times are measured, not invented: instant mocks finish in well under a second.
    assert!(r.transcribe_ms < 1000 && r.compile_ms < 1000);

    h.registry
        .set_output(&h.target_a.id, OutputKind::Raw)
        .unwrap();
    speak(&h).await;
    assert_eq!(
        h.p.last_result().unwrap().compile_ms,
        0,
        "skipped prompt step takes no time"
    );
}
