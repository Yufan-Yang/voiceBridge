//! Restricted temporary audio files for runtimes that only accept file input.
//! Files are deleted on drop, and leftovers from a crash are removed at startup.

use std::path::{Path, PathBuf};

use super::AudioBuffer;
use crate::config::store::restrict_permissions;
use crate::error::{AppError, ErrorCode, Result};

const PREFIX: &str = "vb-utterance-";

pub struct TempAudioFile {
    path: PathBuf,
}

impl TempAudioFile {
    /// Writes `audio` as 16-bit PCM WAV readable only by the current user.
    pub fn write(dir: &Path, audio: &AudioBuffer) -> Result<Self> {
        let fail = |d: String| AppError::new(ErrorCode::AudioCaptureFailed).with_details(d);
        std::fs::create_dir_all(dir).map_err(|e| fail(e.kind().to_string()))?;
        let path = dir.join(format!("{PREFIX}{}.wav", uuid::Uuid::new_v4()));
        // Create the guard first so a failed write is still cleaned up.
        let guard = Self { path };
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: audio.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::create(&guard.path, spec).map_err(|e| fail(e.to_string()))?;
        restrict_permissions(&guard.path);
        for s in &audio.samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(v).map_err(|e| fail(e.to_string()))?;
        }
        writer.finalize().map_err(|e| fail(e.to_string()))?;
        Ok(guard)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempAudioFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Deletes temporary audio left behind by a previous crash. Returns the count.
pub fn cleanup_stale(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(PREFIX)
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MockAudioSource;

    #[test]
    fn temp_file_is_deleted_on_drop_and_stale_files_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let path = {
            let f = TempAudioFile::write(dir.path(), &MockAudioSource::tone(50)).unwrap();
            assert!(f.path().exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(f.path()).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
            f.path().to_path_buf()
        };
        assert!(!path.exists());

        std::fs::write(dir.path().join("vb-utterance-crashed.wav"), b"x").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), b"x").unwrap();
        assert_eq!(cleanup_stale(dir.path()), 1);
        assert!(dir.path().join("unrelated.txt").exists());
    }
}
