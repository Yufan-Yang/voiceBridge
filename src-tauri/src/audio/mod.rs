//! Audio capture, resampling and voice-activity detection.

pub mod capture;
pub mod resample;
pub mod temp;
pub mod vad;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::{AppError, ErrorCode, Result};

/// Sample rate expected by the ASR providers.
pub const ASR_SAMPLE_RATE: u32 = 16_000;

/// Mono PCM audio held in memory. Never serialized and never logged.
#[derive(Clone, PartialEq)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioBuffer {
    pub fn duration_ms(&self) -> u32 {
        if self.sample_rate == 0 {
            return 0;
        }
        (self.samples.len() as u64 * 1000 / self.sample_rate as u64) as u32
    }
}

impl std::fmt::Debug for AudioBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately omits the samples.
        write!(
            f,
            "AudioBuffer({} ms @ {} Hz)",
            self.duration_ms(),
            self.sample_rate
        )
    }
}

pub type LevelCallback = Arc<dyn Fn(f32) + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct CaptureOptions {
    /// Input device name; `None` uses the system default.
    pub device: Option<String>,
    pub max_secs: u32,
}

/// A microphone-like source. Capture runs off the UI thread.
pub trait AudioSource: Send + Sync {
    fn start(&self, options: CaptureOptions, on_level: LevelCallback) -> Result<()>;
    /// Stops capture and returns everything recorded since `start`.
    fn stop(&self) -> Result<AudioBuffer>;
    /// Stops capture and discards the audio.
    fn abort(&self);
    fn list_devices(&self) -> Vec<String>;
    /// Audio captured so far while recording continues, for live interim
    /// transcription. `None` when not recording or not supported.
    fn snapshot(&self) -> Option<AudioBuffer> {
        None
    }
}

/// Converts captured audio to what ASR expects: 16 kHz mono, optionally with
/// leading and trailing silence removed.
pub fn prepare_for_asr(audio: AudioBuffer, vad_enabled: bool) -> Result<AudioBuffer> {
    let resampled = resample::resample_linear(&audio.samples, audio.sample_rate, ASR_SAMPLE_RATE);
    let samples = if vad_enabled {
        vad::trim_silence(&resampled, ASR_SAMPLE_RATE)
    } else {
        resampled
    };
    // Shorter than 150 ms cannot contain a usable utterance.
    if samples.len() < (ASR_SAMPLE_RATE as usize * 150) / 1000 {
        return Err(AppError::new(ErrorCode::NoSpeechDetected));
    }
    let samples = normalize_gain(samples);
    Ok(AudioBuffer {
        samples,
        sample_rate: ASR_SAMPLE_RATE,
    })
}

/// Brings quiet recordings up to a consistent level. Recognition is clearly
/// worse on very quiet input, which is common with headset microphones.
/// Loud recordings are left alone, and the boost is capped so that near
/// silence is not turned into loud noise.
pub fn normalize_gain(mut samples: Vec<f32>) -> Vec<f32> {
    const TARGET_PEAK: f32 = 0.7;
    const MAX_GAIN: f32 = 20.0;
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 && peak < TARGET_PEAK {
        let gain = (TARGET_PEAK / peak).min(MAX_GAIN);
        for s in &mut samples {
            *s *= gain;
        }
    }
    samples
}

/// Loads a WAV file as mono f32. Used by the development-only simulation.
pub fn load_wav(path: &std::path::Path) -> Result<AudioBuffer> {
    let bad = |e: hound::Error| {
        AppError::new(ErrorCode::InvalidInput)
            .with_message("The WAV file could not be read.")
            .with_details(e.to_string())
    };
    let mut reader = hound::WavReader::open(path).map_err(bad)?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()
            .map_err(bad)?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<_, _>>()
                .map_err(bad)?
        }
    };
    let samples = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok(AudioBuffer {
        samples,
        sample_rate: spec.sample_rate,
    })
}

/// Development-only source: every "recording" returns the contents of a WAV
/// file. Wired in only in debug builds when `VOICEBRIDGE_DEV_WAV` is set, so
/// the full Push-to-Talk flow can be driven without speaking.
pub struct WavFileSource {
    path: std::path::PathBuf,
    active: Mutex<bool>,
}

impl WavFileSource {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            active: Mutex::new(false),
        }
    }
}

impl AudioSource for WavFileSource {
    fn start(&self, _options: CaptureOptions, on_level: LevelCallback) -> Result<()> {
        let mut active = self.active.lock().unwrap();
        if *active {
            return Err(AppError::new(ErrorCode::Busy).with_details("already recording"));
        }
        *active = true;
        on_level(0.4);
        Ok(())
    }

    fn stop(&self) -> Result<AudioBuffer> {
        *self.active.lock().unwrap() = false;
        load_wav(&self.path)
    }

    fn abort(&self) {
        *self.active.lock().unwrap() = false;
    }

    fn list_devices(&self) -> Vec<String> {
        vec![format!("WAV file: {}", self.path.display())]
    }

    fn snapshot(&self) -> Option<AudioBuffer> {
        if *self.active.lock().unwrap() {
            load_wav(&self.path).ok()
        } else {
            None
        }
    }
}

/// Synthetic source for tests and headless runs: "records" a short tone.
#[derive(Default)]
pub struct MockAudioSource {
    active: Mutex<bool>,
    pub starts: AtomicU32,
    pub aborts: AtomicU32,
    pub fail_start: Mutex<Option<AppError>>,
}

impl MockAudioSource {
    pub fn tone(ms: u32) -> AudioBuffer {
        let sr = ASR_SAMPLE_RATE;
        let n = (sr as u64 * ms as u64 / 1000) as usize;
        AudioBuffer {
            samples: (0..n)
                .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.3)
                .collect(),
            sample_rate: sr,
        }
    }

    pub fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }
}

impl AudioSource for MockAudioSource {
    fn start(&self, _options: CaptureOptions, on_level: LevelCallback) -> Result<()> {
        if let Some(e) = self.fail_start.lock().unwrap().clone() {
            return Err(e);
        }
        let mut active = self.active.lock().unwrap();
        if *active {
            return Err(AppError::new(ErrorCode::Busy).with_details("already recording"));
        }
        *active = true;
        self.starts.fetch_add(1, Ordering::SeqCst);
        on_level(0.3);
        Ok(())
    }

    fn stop(&self) -> Result<AudioBuffer> {
        let mut active = self.active.lock().unwrap();
        if !*active {
            return Err(AppError::new(ErrorCode::AudioCaptureFailed).with_details("not recording"));
        }
        *active = false;
        Ok(Self::tone(800))
    }

    fn abort(&self) {
        *self.active.lock().unwrap() = false;
        self.aborts.fetch_add(1, Ordering::SeqCst);
    }

    fn list_devices(&self) -> Vec<String> {
        vec!["Mock Microphone".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_resamples_and_rejects_silence() {
        let tone = AudioBuffer {
            samples: (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.4).collect(),
            sample_rate: 48_000,
        };
        let out = prepare_for_asr(tone, true).unwrap();
        assert_eq!(out.sample_rate, ASR_SAMPLE_RATE);
        assert!((out.samples.len() as i64 - 16_000).abs() < 400);

        let silence = AudioBuffer {
            samples: vec![0.0; 48_000],
            sample_rate: 48_000,
        };
        let err = prepare_for_asr(silence, true).unwrap_err();
        assert_eq!(err.code, ErrorCode::NoSpeechDetected);
    }

    #[test]
    fn quiet_recordings_are_boosted_within_limits() {
        let peak = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak(&normalize_gain(vec![0.05, -0.1, 0.02])) - 0.7).abs() < 1e-5);
        assert_eq!(
            normalize_gain(vec![0.9, -0.8]),
            vec![0.9, -0.8],
            "loud audio is untouched"
        );
        assert!(
            (peak(&normalize_gain(vec![0.001, -0.001])) - 0.02).abs() < 1e-5,
            "boost is capped at 20x"
        );
        assert_eq!(normalize_gain(vec![0.0; 4]), vec![0.0; 4]);
    }

    #[test]
    fn debug_output_never_contains_samples() {
        let s = format!("{:?}", MockAudioSource::tone(100));
        assert!(s.starts_with("AudioBuffer(") && s.len() < 40);
    }

    #[test]
    fn wav_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let audio = MockAudioSource::tone(200);
        let file = temp::TempAudioFile::write(dir.path(), &audio).unwrap();
        let loaded = load_wav(file.path()).unwrap();
        assert_eq!(loaded.sample_rate, audio.sample_rate);
        assert_eq!(loaded.samples.len(), audio.samples.len());
    }
}
