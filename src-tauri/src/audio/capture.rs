//! Microphone capture with cpal. The stream lives on a dedicated thread, so
//! capture never blocks the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::{vad, AudioBuffer, AudioSource, CaptureOptions, LevelCallback};
use crate::error::{AppError, ErrorCode, Result};

const LEVEL_INTERVAL: Duration = Duration::from_millis(60);

struct Session {
    sink: Arc<Sink>,
    sample_rate: u32,
    stop_tx: mpsc::Sender<()>,
    join: thread::JoinHandle<Result<AudioBuffer>>,
}

#[derive(Default)]
pub struct CpalAudioSource {
    session: Mutex<Option<Session>>,
}

impl CpalAudioSource {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Collects mono samples and reports the input level.
struct Sink {
    samples: Mutex<Vec<f32>>,
    channels: usize,
    max_samples: usize,
    on_level: LevelCallback,
    last_level: Mutex<Instant>,
}

impl Sink {
    fn push(&self, interleaved: &[f32]) {
        let channels = self.channels.max(1);
        let mono: Vec<f32> = interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect();
        if let Ok(mut last) = self.last_level.try_lock() {
            if last.elapsed() >= LEVEL_INTERVAL {
                *last = Instant::now();
                (self.on_level)((vad::rms(&mono) * 4.0).min(1.0));
            }
        }
        if let Ok(mut samples) = self.samples.lock() {
            let room = self.max_samples.saturating_sub(samples.len());
            samples.extend(mono.into_iter().take(room));
        }
    }
}

fn capture_failed(details: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::AudioCaptureFailed).with_details(details.to_string())
}

fn pick_device(host: &cpal::Host, wanted: Option<&str>) -> Result<cpal::Device> {
    if let Some(name) = wanted {
        if let Ok(mut devices) = host.input_devices() {
            if let Some(d) = devices.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
                return Ok(d);
            }
        }
        // The configured device disappeared: fall back to the system default.
        crate::logging::event(
            "-",
            "audio",
            "configured input device missing; using default",
        );
    }
    host.default_input_device()
        .ok_or_else(|| AppError::new(ErrorCode::MicDeviceNotFound))
}

fn run_capture(
    options: CaptureOptions,
    on_level: LevelCallback,
    ready_tx: mpsc::Sender<Result<(Arc<Sink>, u32)>>,
    stop_rx: mpsc::Receiver<()>,
) -> Result<AudioBuffer> {
    let setup = || -> Result<(cpal::Stream, Arc<Sink>, u32, Arc<AtomicBool>)> {
        let host = cpal::default_host();
        let device = pick_device(&host, options.device.as_deref())?;
        let supported = device
            .default_input_config()
            .map_err(|e| AppError::new(ErrorCode::MicDeviceNotFound).with_details(e.to_string()))?;
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let sink = Arc::new(Sink {
            samples: Mutex::new(Vec::new()),
            channels,
            max_samples: sample_rate as usize * options.max_secs.max(1) as usize,
            on_level,
            last_level: Mutex::new(Instant::now()),
        });
        let failed = Arc::new(AtomicBool::new(false));
        let failed_cb = failed.clone();
        let on_error = move |_err: cpal::StreamError| failed_cb.store(true, Ordering::SeqCst);

        let stream = match format {
            cpal::SampleFormat::F32 => {
                let s = sink.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| s.push(data),
                    on_error,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let s = sink.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let f: Vec<f32> = data.iter().map(|v| *v as f32 / 32768.0).collect();
                        s.push(&f)
                    },
                    on_error,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let s = sink.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let f: Vec<f32> = data
                            .iter()
                            .map(|v| (*v as f32 - 32768.0) / 32768.0)
                            .collect();
                        s.push(&f)
                    },
                    on_error,
                    None,
                )
            }
            other => {
                return Err(capture_failed(format!(
                    "unsupported sample format {other:?}"
                )))
            }
        }
        .map_err(capture_failed)?;
        stream.play().map_err(capture_failed)?;
        Ok((stream, sink, sample_rate, failed))
    };

    let (stream, sink, sample_rate, failed) = match setup() {
        Ok(parts) => {
            let _ = ready_tx.send(Ok((parts.1.clone(), parts.2)));
            parts
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e.clone()));
            return Err(e);
        }
    };

    // Block until asked to stop (or the owner goes away).
    let _ = stop_rx.recv();
    drop(stream);

    let samples = std::mem::take(
        &mut *sink
            .samples
            .lock()
            .map_err(|_| capture_failed("sink poisoned"))?,
    );
    if failed.load(Ordering::SeqCst) && samples.is_empty() {
        return Err(AppError::new(ErrorCode::MicDeviceNotFound).with_details("input stream failed"));
    }
    Ok(AudioBuffer {
        samples,
        sample_rate,
    })
}

impl AudioSource for CpalAudioSource {
    fn start(&self, options: CaptureOptions, on_level: LevelCallback) -> Result<()> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| capture_failed("lock poisoned"))?;
        if guard.is_some() {
            return Err(AppError::new(ErrorCode::Busy).with_details("already recording"));
        }
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("voicebridge-audio".into())
            .spawn(move || run_capture(options, on_level, ready_tx, stop_rx))
            .map_err(capture_failed)?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok((sink, sample_rate))) => {
                *guard = Some(Session {
                    sink,
                    sample_rate,
                    stop_tx,
                    join,
                });
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => {
                let _ = stop_tx.send(());
                Err(capture_failed("audio device did not start in time"))
            }
        }
    }

    fn stop(&self) -> Result<AudioBuffer> {
        let session = self
            .session
            .lock()
            .map_err(|_| capture_failed("lock poisoned"))?
            .take()
            .ok_or_else(|| capture_failed("not recording"))?;
        let _ = session.stop_tx.send(());
        session
            .join
            .join()
            .map_err(|_| capture_failed("capture thread panicked"))?
    }

    fn abort(&self) {
        let session = self.session.lock().ok().and_then(|mut g| g.take());
        if let Some(session) = session {
            let _ = session.stop_tx.send(());
            let _ = session.join.join();
        }
    }

    fn list_devices(&self) -> Vec<String> {
        cpal::default_host()
            .input_devices()
            .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
            .unwrap_or_default()
    }

    fn snapshot(&self) -> Option<AudioBuffer> {
        let guard = self.session.lock().ok()?;
        let session = guard.as_ref()?;
        let samples = session.sink.samples.lock().ok()?.clone();
        Some(AudioBuffer {
            samples,
            sample_rate: session.sample_rate,
        })
    }
}
