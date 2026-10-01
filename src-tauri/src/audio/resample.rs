fn moving_average(input: &[f32], width: usize) -> Vec<f32> {
    let half = width / 2;
    let mut prefix = Vec::with_capacity(input.len() + 1);
    prefix.push(0.0f64);
    for s in input {
        prefix.push(prefix[prefix.len() - 1] + *s as f64);
    }
    (0..input.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + width - half).min(input.len());
            ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32
        })
        .collect()
}

/// Linear-interpolation resampler. Adequate for speech going into ASR.
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    // Downsampling needs a low-pass first, or frequencies above the new
    // Nyquist limit fold back into the speech band as noise. Two passes of a
    // moving average over one output period (a triangular filter) are cheap
    // and adequate for speech.
    let filtered: Vec<f32>;
    let input: &[f32] = if from_rate > to_rate {
        let width = (from_rate as f64 / to_rate as f64).round().max(1.0) as usize;
        filtered = moving_average(&moving_average(input, width), width);
        &filtered
    } else {
        input
    };
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = input[idx];
        let b = input[(idx + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_length_by_rate_ratio() {
        let input: Vec<f32> = (0..44_100).map(|i| i as f32 / 44_100.0).collect();
        let out = resample_linear(&input, 44_100, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!((out[8000] - 0.5).abs() < 0.001);
        assert_eq!(resample_linear(&input, 16_000, 16_000).len(), input.len());
        assert!(resample_linear(&[], 48_000, 16_000).is_empty());
    }

    #[test]
    fn downsampling_removes_content_above_the_new_nyquist() {
        // 12 kHz tone at 48 kHz: above the 8 kHz limit of 16 kHz audio. Without a
        // low-pass it would alias to a loud 4 kHz tone.
        let tone: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 12_000.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let out = resample_linear(&tone, 48_000, 16_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms < 0.1, "aliased energy {rms}");
        // A 1 kHz tone, well inside the speech band, passes almost unchanged.
        let speech: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 1_000.0 * std::f32::consts::TAU / 48_000.0).sin())
            .collect();
        let out = resample_linear(&speech, 48_000, 16_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.6, "speech band attenuated to {rms}");
    }
}
