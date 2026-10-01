//! Live tests against real local runtimes and models. They are `#[ignore]`d
//! so the normal suite needs no models; run them with
//! `scripts/live-tests.sh` (see the README) or:
//!
//! ```sh
//! VB_LLAMA_SERVER=… VB_PROMPT_MODEL=… cargo test --test live_models -- --ignored --nocapture
//! ```
//!
//! A test whose environment variables are not set is skipped with a message.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use voicebridge_lib::asr::funasr::FunAsrProvider;
use voicebridge_lib::asr::whisper_cpp::WhisperCppProvider;
use voicebridge_lib::asr::{AsrContext, AsrProvider};
use voicebridge_lib::audio::{load_wav, prepare_for_asr, AudioBuffer};
use voicebridge_lib::error::ErrorCode;
use voicebridge_lib::prompt::llama_cpp::LlamaCppCompiler;
use voicebridge_lib::prompt::{PromptCompileInput, PromptCompiler, TargetContext};
use voicebridge_lib::sidecar::{SidecarManager, SidecarState};
use voicebridge_lib::types::ProviderStatus;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

macro_rules! require_env {
    ($($name:literal),+) => {
        ($(match env($name) {
            Some(v) => v,
            None => {
                eprintln!("SKIPPED: {} is not set", $name);
                return;
            }
        }),+)
    };
}

fn sample_audio(path: &str) -> AudioBuffer {
    prepare_for_asr(
        load_wav(std::path::Path::new(path)).expect("sample WAV"),
        true,
    )
    .expect("speech in sample")
}

fn hotwords() -> Vec<String> {
    vec!["useUserQuery".to_string(), "userId".to_string()]
}

fn example_input() -> PromptCompileInput {
    PromptCompileInput {
        raw_transcript:
            "change that use user query thing so it takes user id, don't make the request when \
                         there is no id, add two tests, and don't change the return structure"
                .to_string(),
        target: TargetContext {
            app_name: "Claude Code".into(),
            target_alias: "Backend".into(),
            project_root: Some("/path/to/project".into()),
            active_file: Some("src/hooks/useUserQuery.ts".into()),
            language: Some("typescript".into()),
        },
        project_terms: vec![
            "useUserQuery".into(),
            "userId".into(),
            "TanStack Query".into(),
            "enabled".into(),
        ],
        selected_code: None,
        diagnostics: vec![],
    }
}

#[tokio::test]
#[ignore = "needs llama-server and a GGUF model"]
async fn llama_cpp_compiles_a_prompt_and_supports_cancel_and_crash_recovery() {
    let (runtime, model) = require_env!("VB_LLAMA_SERVER", "VB_PROMPT_MODEL");
    let logs = tempfile::tempdir().unwrap();
    let sidecar = Arc::new(SidecarManager::new(
        "prompt-live",
        logs.path().to_path_buf(),
        ErrorCode::PromptProcessFailed,
    ));
    let compiler = LlamaCppCompiler::new(sidecar.clone(), runtime, model, 8192, 512);

    assert_eq!(
        compiler.health_check().await.unwrap().status,
        ProviderStatus::Unavailable
    );

    // First request loads the model.
    let started = Instant::now();
    let out = compiler
        .compile(example_input(), CancellationToken::new())
        .await
        .expect("compile with a real model");
    eprintln!("first compile (incl. model load): {:?}", started.elapsed());
    eprintln!(
        "intent: {}\nnormalized: {}\nprompt:\n{}",
        out.intent, out.normalized_transcript, out.prompt
    );
    assert!(!out.prompt.trim().is_empty());
    assert!(
        out.prompt.contains("useUserQuery"),
        "identifier should be corrected from project_terms"
    );
    assert!(out.prompt.contains("userId"));
    assert!(
        !out.prompt.contains("```"),
        "the compiler must not generate code blocks"
    );
    assert_eq!(
        compiler.health_check().await.unwrap().status,
        ProviderStatus::Ready
    );

    // Warm request.
    let started = Instant::now();
    compiler
        .compile(example_input(), CancellationToken::new())
        .await
        .unwrap();
    eprintln!("warm compile: {:?}", started.elapsed());

    // The server must be bound to loopback only and reject a wrong key.
    let pid = sidecar.pid().await.unwrap();
    let listen = std::process::Command::new("/usr/sbin/lsof")
        .args([
            "-a",
            "-p",
            &pid.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-n",
            "-P",
        ])
        .output()
        .unwrap();
    let listen = String::from_utf8_lossy(&listen.stdout).to_string();
    eprintln!("listening sockets:\n{listen}");
    assert!(
        listen.contains("127.0.0.1:"),
        "expected a loopback listener"
    );
    assert!(!listen.contains("*:"), "must not listen on all interfaces");
    let port: u16 = listen
        .split("127.0.0.1:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .expect("port");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let unauthorized = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .bearer_auth("wrong-key")
        .json(&LlamaCppCompiler::request_body(&example_input(), 16))
        .send()
        .await
        .unwrap();
    assert_eq!(
        unauthorized.status().as_u16(),
        401,
        "requests without the session key are refused"
    );

    // Cancellation returns promptly.
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let err = compiler.compile(example_input(), cancel).await.unwrap_err();
    assert!(err.is_canceled());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancel took {:?}",
        started.elapsed()
    );

    // Crash: kill the server, health goes unavailable, next request recovers.
    std::process::Command::new("/bin/kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(sidecar.state().await, SidecarState::Crashed);
    assert_eq!(
        compiler.health_check().await.unwrap().status,
        ProviderStatus::Unavailable
    );
    compiler
        .compile(example_input(), CancellationToken::new())
        .await
        .expect("recovers after a crash");

    sidecar.stop().await;
    let log =
        std::fs::read_to_string(logs.path().join("sidecar-prompt-live.log")).unwrap_or_default();
    assert!(
        !log.contains("use user query"),
        "sidecar log must not contain the transcript"
    );
}

#[tokio::test]
#[ignore = "needs whisper-cli and a ggml model"]
async fn whisper_cpp_transcribes_speech() {
    let (runtime, model, wav) = require_env!("VB_WHISPER_CLI", "VB_WHISPER_MODEL", "VB_SAMPLE_WAV");
    let tmp = tempfile::tempdir().unwrap();
    let provider = WhisperCppProvider {
        runtime,
        model,
        temp_dir: tmp.path().to_path_buf(),
    };
    assert_eq!(
        provider.health_check().await.unwrap().status,
        ProviderStatus::Ready
    );
    let context = AsrContext {
        utterance_id: "live-whisper".into(),
        hotwords: hotwords(),
        language: Some("en".into()),
    };
    let started = Instant::now();
    let out = provider
        .transcribe(
            sample_audio(&wav),
            context.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("transcribe");
    eprintln!("whisper ({:?}): {}", started.elapsed(), out.text);
    let lower = out.text.to_lowercase();
    assert!(
        lower.contains("user") && lower.contains("tests"),
        "unexpected transcript: {}",
        out.text
    );
    assert_eq!(
        std::fs::read_dir(tmp.path()).unwrap().count(),
        0,
        "temp audio must be deleted"
    );

    // Cancellation kills the process and still removes the temp file.
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let err = provider
        .transcribe(sample_audio(&wav), context, cancel)
        .await
        .unwrap_err();
    assert!(err.is_canceled());
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[tokio::test]
#[ignore = "needs Python with funasr and a Fun-ASR model directory"]
async fn fun_asr_sidecar_transcribes_speech() {
    let (runtime, model, wav) =
        require_env!("VB_FUNASR_PYTHON", "VB_FUNASR_MODEL", "VB_SAMPLE_WAV");
    let tmp = tempfile::tempdir().unwrap();
    let sidecar = Arc::new(SidecarManager::new(
        "asr-live",
        tmp.path().to_path_buf(),
        ErrorCode::AsrProcessFailed,
    ));
    let provider = FunAsrProvider {
        sidecar: sidecar.clone(),
        runtime,
        model,
        script: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sidecars/funasr_sidecar.py"),
        temp_dir: tmp.path().join("audio"),
        timeout: Duration::from_secs(120),
    };
    let started = Instant::now();
    if let Err(e) = provider.start().await {
        let log =
            std::fs::read_to_string(tmp.path().join("sidecar-asr-live.log")).unwrap_or_default();
        panic!(
            "sidecar failed to start: {:?} {}\n--- sidecar log ---\n{log}",
            e.code, e.details
        );
    }
    eprintln!("fun-asr model load: {:?}", started.elapsed());
    assert_eq!(
        provider.health_check().await.unwrap().status,
        ProviderStatus::Ready
    );

    let context = AsrContext {
        utterance_id: "live-funasr".into(),
        hotwords: hotwords(),
        language: None,
    };
    let started = Instant::now();
    let out = provider
        .transcribe(sample_audio(&wav), context, CancellationToken::new())
        .await
        .expect("transcribe");
    eprintln!("fun-asr ({:?}): {}", started.elapsed(), out.text);
    let lower = out.text.to_lowercase();
    assert!(
        lower.contains("user") && lower.contains("test"),
        "unexpected transcript: {}",
        out.text
    );
    assert_eq!(
        std::fs::read_dir(tmp.path().join("audio")).unwrap().count(),
        0,
        "temp audio must be deleted"
    );

    sidecar.stop().await;
    let log = std::fs::read_to_string(tmp.path().join("sidecar-asr-live.log")).unwrap_or_default();
    assert!(
        !log.to_lowercase().contains("user query"),
        "sidecar log must not contain the transcript"
    );
}
