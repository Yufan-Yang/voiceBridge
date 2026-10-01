//! Lifecycle management for local model sidecar processes: start, stop,
//! restart, crash detection and sanitized stderr logging.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, ErrorCode, Result};
use crate::logging::sanitize_line;

pub const SESSION_TOKEN_ENV: &str = "VOICEBRIDGE_SESSION_TOKEN";

#[derive(Debug, Clone)]
pub struct SidecarSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Speak the JSON-lines protocol over stdin/stdout.
    pub stdio_protocol: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarState {
    NotStarted,
    Running,
    /// The process exited without being asked to.
    Crashed,
}

struct Io {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

#[derive(Default)]
struct Inner {
    child: Option<Child>,
    spec: Option<SidecarSpec>,
    crashed: bool,
}

pub struct SidecarManager {
    name: String,
    log_dir: PathBuf,
    /// Random per-session secret shared only with this sidecar.
    token: String,
    /// Error category reported for process failures.
    error_code: ErrorCode,
    inner: Mutex<Inner>,
    io: Mutex<Option<Io>>,
    /// Held by providers while they bring the sidecar up, so a warm-up and a
    /// first request cannot start two processes.
    pub start_lock: Mutex<()>,
}

pub fn new_session_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Picks a free loopback port. Sidecars only ever bind 127.0.0.1.
pub fn free_loopback_port() -> Result<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| AppError::new(ErrorCode::Internal).with_details(e.kind().to_string()))
}

impl SidecarManager {
    pub fn new(name: &str, log_dir: PathBuf, error_code: ErrorCode) -> Self {
        Self {
            name: name.to_string(),
            log_dir,
            token: new_session_token(),
            error_code,
            inner: Mutex::new(Inner::default()),
            io: Mutex::new(None),
            start_lock: Mutex::new(()),
        }
    }

    fn pid_file(&self) -> PathBuf {
        self.log_dir.join(format!("sidecar-{}.pid", self.name))
    }

    /// Kills a sidecar left running by a previous run that was killed or
    /// crashed before it could stop its children. The recorded executable
    /// must still match, so an unrelated process that reused the pid is
    /// never touched. Returns true when something was killed.
    pub fn reap_orphan(&self) -> bool {
        let path = self.pid_file();
        let Ok(content) = std::fs::read_to_string(&path) else {
            return false;
        };
        let _ = std::fs::remove_file(&path);
        let mut lines = content.lines();
        let (Some(pid), Some(program)) = (lines.next(), lines.next()) else {
            return false;
        };
        if pid.parse::<u32>().is_err() || program.is_empty() {
            return false;
        }
        #[cfg(unix)]
        {
            let running = std::process::Command::new("/bin/ps")
                .args(["-p", pid, "-o", "command="])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();
            if running.trim_start().starts_with(program) {
                let killed = std::process::Command::new("/bin/kill")
                    .args(["-9", pid])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                crate::logging::event("-", "sidecar", &format!("{} orphan reaped", self.name));
                return killed;
            }
        }
        false
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    fn fail(&self, details: impl Into<String>) -> AppError {
        AppError::new(self.error_code).with_details(details)
    }

    /// Starts the sidecar, replacing any running instance.
    pub async fn start(&self, spec: SidecarSpec) -> Result<()> {
        self.stop().await;
        if !spec.program.is_file() {
            return Err(self.fail("runtime executable not found"));
        }
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .envs(spec.env.iter().cloned())
            .env(SESSION_TOKEN_ENV, &self.token)
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if spec.stdio_protocol {
            cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null()).stdout(Stdio::null());
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| self.fail(format!("spawn failed: {}", e.kind())))?;

        if let Some(stderr) = child.stderr.take() {
            let path = self.log_dir.join(format!("sidecar-{}.log", self.name));
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                    {
                        let _ = writeln!(f, "{}", sanitize_line(&line));
                    }
                }
            });
        }
        if spec.stdio_protocol {
            if let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) {
                *self.io.lock().await = Some(Io {
                    stdin,
                    stdout: BufReader::new(stdout).lines(),
                });
            }
        }
        if let Some(pid) = child.id() {
            let record = format!("{pid}\n{}\n", spec.program.display());
            let _ = std::fs::write(self.pid_file(), record);
        }
        crate::logging::event("-", "sidecar", &format!("{} started", self.name));
        let mut inner = self.inner.lock().await;
        inner.child = Some(child);
        inner.spec = Some(spec);
        inner.crashed = false;
        Ok(())
    }

    /// Stops the sidecar. Not treated as a crash.
    pub async fn stop(&self) {
        let child = {
            let mut inner = self.inner.lock().await;
            inner.crashed = false;
            inner.child.take()
        };
        if let Some(mut child) = child {
            let _ = child.kill().await;
            let _ = std::fs::remove_file(self.pid_file());
            crate::logging::event("-", "sidecar", &format!("{} stopped", self.name));
        }
        *self.io.lock().await = None;
    }

    /// Restarts with the last spec.
    pub async fn restart(&self) -> Result<()> {
        let spec = self.inner.lock().await.spec.clone();
        match spec {
            Some(spec) => self.start(spec).await,
            None => Err(self.fail("sidecar was never started")),
        }
    }

    /// Current state. Detects a process that died on its own.
    pub async fn state(&self) -> SidecarState {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return SidecarState::Running,
                _ => {
                    inner.child = None;
                    inner.crashed = true;
                    crate::logging::event(
                        "-",
                        "sidecar",
                        &format!("{} exited unexpectedly", self.name),
                    );
                }
            }
        }
        if inner.crashed {
            SidecarState::Crashed
        } else {
            SidecarState::NotStarted
        }
    }

    pub async fn pid(&self) -> Option<u32> {
        self.inner.lock().await.child.as_ref().and_then(|c| c.id())
    }

    /// Sends one JSON line and waits for one JSON line back. Cancelling or
    /// timing out kills the sidecar (the only way to abort a stdio request);
    /// it is started again on the next use.
    pub async fn request_line(
        &self,
        line: &str,
        cancel: &CancellationToken,
        timeout: Duration,
        timeout_code: ErrorCode,
    ) -> Result<String> {
        let mut io_guard = self.io.lock().await;
        let io = io_guard
            .as_mut()
            .ok_or_else(|| self.fail("sidecar is not running"))?;
        let exchange = async {
            io.stdin.write_all(line.as_bytes()).await?;
            io.stdin.write_all(b"\n").await?;
            io.stdin.flush().await?;
            io.stdout.next_line().await
        };
        let outcome = tokio::select! {
            _ = cancel.cancelled() => Err(AppError::canceled()),
            r = tokio::time::timeout(timeout, exchange) => match r {
                Err(_) => Err(AppError::new(timeout_code)),
                Ok(Ok(Some(reply))) => Ok(reply),
                Ok(Ok(None)) => Err(self.fail("sidecar closed its output")),
                Ok(Err(e)) => Err(self.fail(format!("pipe error: {}", e.kind()))),
            },
        };
        if outcome.is_err() {
            drop(io_guard);
            self.stop().await;
        }
        outcome
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn manager(dir: &std::path::Path) -> SidecarManager {
        SidecarManager::new("test", dir.to_path_buf(), ErrorCode::AsrProcessFailed)
    }

    fn spec(program: &str, args: &[&str], stdio: bool) -> SidecarSpec {
        SidecarSpec {
            program: PathBuf::from(program),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: vec![],
            stdio_protocol: stdio,
        }
    }

    #[tokio::test]
    async fn start_stop_restart() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        assert_eq!(m.state().await, SidecarState::NotStarted);
        m.start(spec("/bin/sleep", &["30"], false)).await.unwrap();
        assert_eq!(m.state().await, SidecarState::Running);
        let first = m.pid().await;
        m.restart().await.unwrap();
        assert_eq!(m.state().await, SidecarState::Running);
        assert_ne!(m.pid().await, first);
        m.stop().await;
        // A deliberate stop is not a crash.
        assert_eq!(m.state().await, SidecarState::NotStarted);
    }

    #[tokio::test]
    async fn crash_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        m.start(spec("/bin/sh", &["-c", "exit 3"], false))
            .await
            .unwrap();
        for _ in 0..100 {
            if m.state().await == SidecarState::Crashed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(m.state().await, SidecarState::Crashed);
        // Crash recovery: the same spec can be started again.
        m.start(spec("/bin/sleep", &["30"], false)).await.unwrap();
        assert_eq!(m.state().await, SidecarState::Running);
        m.stop().await;
    }

    #[tokio::test]
    async fn missing_runtime_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        let err = m
            .start(spec("/nonexistent/runtime", &[], false))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::AsrProcessFailed);
    }

    #[tokio::test]
    async fn stdio_request_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        m.start(spec("/bin/cat", &[], true)).await.unwrap();
        let cancel = CancellationToken::new();
        let reply = m
            .request_line(
                "{\"op\":\"ping\"}",
                &cancel,
                Duration::from_secs(5),
                ErrorCode::AsrTimeout,
            )
            .await
            .unwrap();
        assert_eq!(reply, "{\"op\":\"ping\"}");

        // A sidecar that never answers: cancellation aborts and kills it.
        m.start(spec("/bin/sleep", &["30"], true)).await.unwrap();
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel2.cancel();
        });
        let err = m
            .request_line("{}", &cancel, Duration::from_secs(5), ErrorCode::AsrTimeout)
            .await
            .unwrap_err();
        assert!(err.is_canceled());
        assert_eq!(m.state().await, SidecarState::NotStarted);
    }

    #[tokio::test]
    async fn stderr_is_sanitized_into_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        m.start(spec(
            "/bin/sh",
            &[
                "-c",
                "echo 'loaded prompt=\"my secret words\" ok' >&2; sleep 0.2",
            ],
            false,
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let log = std::fs::read_to_string(dir.path().join("sidecar-test.log")).unwrap();
        assert!(log.contains("loaded") && !log.contains("secret"));
    }

    #[tokio::test]
    async fn orphan_from_a_previous_run_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        // A previous run that was killed: its child outlives it.
        let mut orphan = std::process::Command::new("/bin/sleep")
            .arg("60")
            .spawn()
            .unwrap();
        std::fs::write(
            dir.path().join("sidecar-test.pid"),
            format!("{}\n/bin/sleep\n", orphan.id()),
        )
        .unwrap();
        let m = manager(dir.path());
        assert!(m.reap_orphan());
        assert!(!orphan.wait().unwrap().success());
        assert!(!dir.path().join("sidecar-test.pid").exists());

        // A recycled pid that now belongs to another program is left alone.
        std::fs::write(
            dir.path().join("sidecar-test.pid"),
            format!("{}\n/some/other/runtime\n", std::process::id()),
        )
        .unwrap();
        assert!(!m.reap_orphan());

        // A normal start/stop leaves no pid file behind.
        m.start(spec("/bin/sleep", &["30"], false)).await.unwrap();
        assert!(dir.path().join("sidecar-test.pid").exists());
        m.stop().await;
        assert!(!dir.path().join("sidecar-test.pid").exists());
        assert!(!m.reap_orphan());
    }

    #[test]
    fn tokens_are_random_and_ports_are_loopback() {
        assert_ne!(new_session_token(), new_session_token());
        assert_eq!(new_session_token().len(), 64);
        assert!(free_loopback_port().unwrap() > 0);
    }
}
