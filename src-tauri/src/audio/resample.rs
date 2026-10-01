/// Linear-interpolation resampler. Adequate for speech going into ASR.
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
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
}
