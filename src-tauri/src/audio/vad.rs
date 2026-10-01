//! Minimal energy-based voice activity detection.

const FRAME_MS: usize = 20;
const SPEECH_RMS: f32 = 0.008;
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
    let voiced: Vec<bool> = samples
        .chunks(frame)
        .map(|f| rms(f) >= SPEECH_RMS)
        .collect();
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
}
