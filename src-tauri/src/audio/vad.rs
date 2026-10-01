//! Minimal energy-based voice activity detection.

const FRAME_MS: usize = 20;
/// Level that counts as speech for a normal microphone.
const SPEECH_RMS: f32 = 0.008;
/// Below this even the loudest frame is treated as silence.
const NOISE_FLOOR_RMS: f32 = 0.0015;
/// Quiet microphones (Bluetooth headsets in particular): frames within this
/// fraction of the loudest frame count as speech.
const RELATIVE_TO_PEAK: f32 = 0.12;

/// Speech threshold for a recording, adapted to how loud it is overall.
pub fn speech_threshold(samples: &[f32], sample_rate: u32) -> Option<f32> {
    let frame = (sample_rate as usize * FRAME_MS / 1000).max(1);
    let loudest = samples.chunks(frame).map(rms).fold(0.0f32, f32::max);
    (loudest >= NOISE_FLOOR_RMS)
        .then(|| (loudest * RELATIVE_TO_PEAK).clamp(NOISE_FLOOR_RMS, SPEECH_RMS))
}
const PAD_BEFORE_MS: usize = 200;
const PAD_AFTER_MS: usize = 300;

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Removes leading and trailing silence. Returns an empty vector when no
/// frame contains speech-level energy.
pub fn trim_silence(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let frame = (sample_rate as usize * FRAME_MS / 1000).max(1);
    let Some(threshold) = speech_threshold(samples, sample_rate) else {
        return Vec::new();
    };
    let voiced: Vec<bool> = samples.chunks(frame).map(|f| rms(f) >= threshold).collect();
    let (Some(first), Some(last)) = (
        voiced.iter().position(|v| *v),
        voiced.iter().rposition(|v| *v),
    ) else {
        return Vec::new();
    };
    let pad_before = sample_rate as usize * PAD_BEFORE_MS / 1000;
    let pad_after = sample_rate as usize * PAD_AFTER_MS / 1000;
    let start = (first * frame).saturating_sub(pad_before);
    let end = ((last + 1) * frame + pad_after).min(samples.len());
    samples[start..end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_to_voiced_region() {
        let sr = 16_000;
        let mut samples = vec![0.0f32; sr as usize];
        samples.extend((0..sr).map(|i| (i as f32 * 0.2).sin() * 0.5));
        samples.extend(vec![0.0f32; sr as usize]);
        let out = trim_silence(&samples, sr);
        assert!(out.len() < samples.len());
        assert!(out.len() >= sr as usize);
        assert!(trim_silence(&vec![0.0; 16_000], sr).is_empty());
    }

    #[test]
    fn quiet_microphone_speech_is_kept() {
        let sr = 16_000;
        // A quiet headset: speech well below the normal threshold, over a low noise floor.
        let noise = |i: u32| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.0006;
        let mut samples: Vec<f32> = (0..sr).map(noise).collect();
        samples.extend((0..sr).map(|i| (i as f32 * 0.2).sin() * 0.006 + noise(i)));
        samples.extend((0..sr).map(noise));
        let out = trim_silence(&samples, sr);
        assert!(
            out.len() >= sr as usize,
            "quiet speech must not be discarded"
        );
        assert!(
            out.len() < samples.len(),
            "surrounding noise is still trimmed"
        );
        // Pure background noise is still silence.
        let only_noise: Vec<f32> = (0..sr * 2).map(noise).collect();
        assert!(trim_silence(&only_noise, sr).is_empty());
    }
}
