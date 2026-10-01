//! Local debug log. Only identifiers, stages, timings and sanitized provider
//! diagnostics are written — never transcripts, prompts, source code or audio.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

const MAX_LINE: usize = 200;

pub fn init(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("voicebridge.log"))
        .ok();
    let _ = LOG.set(Mutex::new(file));
}

/// Record a pipeline event. `detail` must be an error code, timing, or state
/// name; callers must never pass user content.
pub fn event(utterance_id: &str, stage: &str, detail: &str) {
    let line = format!(
        "{} utterance={} stage={} {}\n",
        chrono::Utc::now().to_rfc3339(),
        utterance_id,
        stage,
        sanitize_line(detail)
    );
    if cfg!(debug_assertions) {
        eprint!("[voicebridge] {line}");
    }
    if let Some(lock) = LOG.get() {
        if let Ok(mut guard) = lock.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

/// Make a provider diagnostic line safe to log: quoted payloads are redacted,
/// control characters are dropped, and the line is truncated.
pub fn sanitize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len().min(MAX_LINE));
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                    out.push_str("<redacted>");
                    out.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '`' {
                    quote = Some(ch);
                    out.push(ch);
                } else if !ch.is_control() {
                    out.push(ch);
                }
            }
        }
        if out.chars().count() >= MAX_LINE {
            break;
        }
    }
    if quote.is_some() {
        out.push_str("<redacted>");
    }
    out.chars().take(MAX_LINE + 12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_quoted_payloads_and_truncates() {
        let s = sanitize_line("request failed body=\"secret transcript text\" code=500");
        assert!(!s.contains("secret"));
        assert!(s.contains("code=500"));
        let long = "x".repeat(1000);
        assert!(sanitize_line(&long).len() <= MAX_LINE + 12);
        let open = sanitize_line("prompt: \"never closed secret");
        assert!(!open.contains("secret"));
    }
}
